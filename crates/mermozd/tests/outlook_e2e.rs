//! End-to-end: a real mermozd with an Outlook account against the stub Microsoft Graph
//! and sign-in: add the account through the browser flow, import, a change made in
//! Outlook, mark read, archive (a move), and a reply that lands in its thread.

use mermoz_core::outlook::stub::{Graph, Shared};
use rstest::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ME: &str = "ana@outlook.example";

fn mime(from: &str, subject: &str, id: &str) -> String {
    format!(
        "From: {from}\r\nTo: Ana <{ME}>\r\nSubject: {subject}\r\nMessage-ID: <{id}@example.com>\r\nDate: Wed, 21 Oct 2026 08:00:00 +0000\r\n\r\nAbout {subject}\r\n"
    )
}

struct World {
    home: tempfile::TempDir,
    graph: Shared,
    daemon: Child,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

impl World {
    fn sock(&self) -> PathBuf {
        self.home.path().join("mermozd.sock")
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, Value> {
        let mut s = UnixStream::connect(self.sock()).unwrap();
        writeln!(
            s,
            "{}",
            json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
        )
        .unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        match v.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(v["result"].clone()),
        }
    }

    fn ok(&self, method: &str, params: Value) -> Value {
        self.call(method, params)
            .unwrap_or_else(|e| panic!("{method} failed: {e}"))
    }

    fn wait_until(&self, what: &str, mut f: impl FnMut(&World) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if f(self) {
                return;
            }
            let _ = self.call("sync.now", json!({}));
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("timed out waiting for {what}");
    }

    fn threads(&self) -> Vec<Value> {
        self.ok("threads.list", json!({ "limit": 100 }))["threads"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    fn thread_by_subject(&self, subject: &str) -> Value {
        self.threads()
            .into_iter()
            .find(|t| t["subject"] == subject)
            .unwrap_or_else(|| panic!("no thread {subject}"))
    }

    fn graph(&self) -> std::sync::MutexGuard<'_, Graph> {
        self.graph.lock().unwrap()
    }
}

/// A daemon on a fresh home talking to a stub Graph holding an Inbox conversation, an
/// archived bill and a "Clients" folder. `outlook_client`: whether this build has a
/// Microsoft sign-in client.
fn start(outlook_client: bool) -> World {
    let now = chrono_now_ms();
    let mut g = Graph::new(ME);
    g.add_folder("f-clients", "Clients", "root");
    g.add_message(
        "i1",
        "c1",
        "f-inbox",
        false,
        &mime("Marc <marc@example.com>", "Audit report", "i1"),
        now - 60_000,
    );
    g.add_message(
        "a1",
        "c2",
        "f-archive",
        true,
        &mime("Bank <no-reply@bank.example>", "Statement", "a1"),
        now - 120_000,
    );
    g.add_message(
        "u1",
        "c3",
        "f-clients",
        true,
        &mime("Bo <bo@example.com>", "Contract", "u1"),
        now - 180_000,
    );
    let graph: Shared = Arc::new(Mutex::new(g));
    let port = mermoz_core::outlook::stub::start(graph.clone());
    let home = tempfile::tempdir().unwrap();
    if outlook_client {
        std::fs::write(
            home.path().join("microsoft_client.json"),
            json!({ "client_id": "ms-cid", "auth_url": format!("http://127.0.0.1:{port}/ms/authorize"), "token_url": format!("http://127.0.0.1:{port}/ms/token") }).to_string(),
        )
        .unwrap();
    }
    let daemon = Command::new(env!("CARGO_BIN_EXE_mermozd"))
        .env("MERMOZ_HOME", home.path())
        .env("MERMOZ_TOKEN_STORE", "file")
        .env(
            "MERMOZ_GRAPH_BASE",
            format!("http://127.0.0.1:{port}/graph"),
        )
        .env("MERMOZ_POLL_SECS", "1")
        .env("MERMOZ_TRUSTED_EXE", std::env::current_exe().unwrap())
        .env("MERMOZ_EMBEDDER", "stub")
        .env_remove("MERMOZ_MICROSOFT_CLIENT_ID")
        .env(
            "RUST_LOG",
            std::env::var("E2E_LOG").unwrap_or("warn".into()),
        )
        .stderr(if std::env::var("E2E_LOG").is_ok() {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .spawn()
        .unwrap();
    let w = World {
        home,
        graph,
        daemon,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(w.sock()).is_err() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    w
}

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn query(url: &str, key: &str) -> String {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| panic!("no {key} in {url}"))
}

/// Plays the browser: Microsoft sends the user back to the loopback redirect.
fn sign_in(w: &World) {
    let url = w.ok("accounts.add_outlook", json!({}))["auth_url"]
        .as_str()
        .unwrap()
        .to_string();
    come_back(&url);
}

fn account_row(w: &World) -> Value {
    w.ok("accounts.list", json!({}))[0].clone()
}

/// Microsoft stopped accepting the account's sign-in (revoked, expired).
fn refused(w: &World) {
    {
        let mut g = w.graph();
        g.refuse_refresh = true;
        g.unauthorized = true;
    }
    w.wait_until("sign-in refused", |w| {
        account_row(w)["sign_in_expired"] == true
    });
}

#[rstest]
fn a_refused_microsoft_sign_in_asks_to_sign_in_again(synced: World) {
    refused(&synced);
    assert_eq!(account_row(&synced)["sign_in_expired"], true);
}

#[rstest]
fn signing_in_again_resumes_the_account(synced: World) {
    refused(&synced);
    {
        let mut g = synced.graph();
        g.refuse_refresh = false;
        g.unauthorized = false;
    }
    let id = account_row(&synced)["id"].clone();
    let url = synced.ok("accounts.reconnect", json!({ "account_id": id }))["auth_url"]
        .as_str()
        .unwrap()
        .to_string();
    come_back(&url);
    synced.wait_until("signed in again", |w| {
        account_row(w)["sign_in_expired"] == false
    });
}

/// The browser's return to the loopback redirect of the consent at `url`.
fn come_back(url: &str) {
    let redirect = query(url, "redirect_uri");
    let port = redirect.rsplit(':').next().unwrap().trim_end_matches('/');
    let mut s = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    write!(
        s,
        "GET /?code=stub-code&state={} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n",
        url::form_urlencoded::byte_serialize(query(url, "state").as_bytes()).collect::<String>()
    )
    .unwrap();
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
}

#[fixture]
fn world() -> World {
    start(true)
}

/// Signed in and imported.
#[fixture]
fn synced(world: World) -> World {
    sign_in(&world);
    world.wait_until("import", |w| w.threads().len() == 3);
    world
}

#[rstest]
fn sign_in_adds_an_outlook_account(synced: World) {
    assert_eq!(
        synced.ok("accounts.list", json!({}))[0]["provider"],
        "outlook"
    );
}

#[rstest]
fn the_account_is_its_sign_in_address(synced: World) {
    assert_eq!(synced.ok("accounts.list", json!({}))[0]["email"], ME);
}

#[rstest]
fn the_consent_asks_for_mail_access(world: World) {
    let url = world.ok("accounts.add_outlook", json!({}))["auth_url"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        query(&url, "scope"),
        "offline_access User.Read Mail.ReadWrite Mail.Send MailboxSettings.Read"
    );
}

#[rstest]
fn the_sign_in_never_sends_a_secret(synced: World) {
    assert!(
        synced
            .graph()
            .token_forms
            .iter()
            .all(|f| !f.contains_key("client_secret"))
    );
}

#[rstest]
fn import_brings_every_thread(synced: World) {
    assert_eq!(synced.threads().len(), 3);
}

#[rstest]
fn unread_inbox_mail_lands_in_unread(synced: World) {
    assert_eq!(synced.thread_by_subject("Audit report")["status"], "unread");
}

#[rstest]
fn a_folder_is_a_project(synced: World) {
    assert_eq!(
        synced.thread_by_subject("Contract")["projects"],
        json!(["Clients"])
    );
}

#[rstest]
fn new_mail_in_outlook_arrives(synced: World) {
    let now = chrono_now_ms();
    synced.graph().add_message(
        "i2",
        "c9",
        "f-inbox",
        false,
        &mime("Di <di@example.com>", "Trip", "i2"),
        now,
    );
    synced.wait_until("new mail", |w| {
        w.threads().iter().any(|t| t["subject"] == "Trip")
    });
}

#[rstest]
fn read_in_outlook_reads_here(synced: World) {
    synced.graph().edit("i1", |m| m.is_read = true);
    synced.wait_until("read state", |w| {
        w.thread_by_subject("Audit report")["unread"] == false
    });
}

#[rstest]
fn mark_read_reaches_outlook(synced: World) {
    let t = synced.thread_by_subject("Audit report");
    synced.ok("threads.mark_read", json!({ "id": t["id"] }));
    synced.wait_until("read pushed", |w| w.graph().messages["i1"].is_read);
}

#[rstest]
fn archive_moves_the_mail_in_outlook(synced: World) {
    let t = synced.thread_by_subject("Audit report");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "status": "archive" }),
    );
    synced.wait_until("archive move", |w| {
        w.graph().messages["i1"].folder == "f-archive"
    });
}

#[rstest]
fn a_reply_lands_in_its_thread(synced: World) {
    let t = synced.thread_by_subject("Audit report");
    let d = synced.ok(
        "drafts.create",
        json!({ "thread_id": t["id"], "body": "Signed off." }),
    );
    synced.ok("drafts.send", json!({ "id": d["id"] }));
    synced.wait_until("reply in thread", |w| {
        w.ok("threads.get", json!({ "id": t["id"] }))["messages"]
            .as_array()
            .is_some_and(|m| m.len() == 2)
    });
}

#[rstest]
fn a_reply_answers_the_message(synced: World) {
    let t = synced.thread_by_subject("Audit report");
    let d = synced.ok(
        "drafts.create",
        json!({ "thread_id": t["id"], "body": "Signed off." }),
    );
    synced.ok("drafts.send", json!({ "id": d["id"] }));
    assert!(synced.graph().sent[0].contains("In-Reply-To: <i1@example.com>"));
}

#[rstest]
fn drive_is_refused_for_outlook(synced: World) {
    let acc = synced.ok("accounts.list", json!({}))[0]["id"].clone();
    assert_eq!(
        synced
            .call("drive.search", json!({ "account_id": acc }))
            .unwrap_err()["message"],
        "Google Drive works with Google accounts only."
    );
}

#[rstest]
fn the_app_knows_outlook_sign_in_is_set_up(world: World) {
    assert_eq!(
        world.ok("accounts.sign_in_status", json!({}))["outlook_available"],
        true
    );
}

#[rstest]
fn without_a_sign_in_client_outlook_says_so() {
    let w = start(false);
    assert_eq!(
        w.call("accounts.add_outlook", json!({})).unwrap_err()["message"],
        "Outlook sign-in isn't set up in this version of Mermoz yet. It will come with an update."
    );
}

#[rstest]
fn without_a_sign_in_client_outlook_is_unavailable() {
    let w = start(false);
    assert_eq!(
        w.ok("accounts.sign_in_status", json!({}))["outlook_available"],
        false
    );
}
