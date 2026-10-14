//! End-to-end Google Drive flows: real mermozd against stub Gmail and stub Drive.
//! Covers connecting Drive (and declining it), previews and text of linked documents,
//! the files shared with a thread's people, saving an attachment into the project's
//! folder, picking a file for a reply and sharing it.

#[allow(dead_code)]
mod stub_drive;
#[allow(dead_code)]
mod stub_gmail;

use rstest::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use stub_drive::{DOC, SHEET, SharedDrive, StubFile};
use stub_gmail::Shared;

const NOW: i64 = 1_791_700_000_000;
const READ_ALL: &str = "https://www.googleapis.com/auth/drive.readonly";
const FILE_ONLY: &str = "https://www.googleapis.com/auth/drive.file";
const GMAIL: &str = "https://www.googleapis.com/auth/gmail.modify";
/// A Google Doc Marc shares in his mail.
const PLAN: &str = "1PlanRemediationDocAbCdEfGhIjKlMn";
/// A sheet the user shared with Marc, never linked in mail.
const FORECAST: &str = "1ForecastSheetAbCdEfGhIjKlMnOpQ";
/// A file of the user's that has nothing to do with Marc.
const HOLIDAY: &str = "1HolidayPhotosAbCdEfGhIjKlMnOpQ";

fn raw(from: &str, subject: &str, body: &str, msg_id: &str) -> String {
    format!(
        "From: {from}\r\nTo: Me <me@example.com>\r\nSubject: {subject}\r\nMessage-ID: <{msg_id}@example.com>\r\nDate: Sun, 11 Oct 2026 08:12:00 +0000\r\n\r\n{body}\r\n"
    )
}

fn raw_with_pdf(from: &str, subject: &str, msg_id: &str) -> String {
    format!(
        "From: {from}\r\nTo: me@example.com\r\nSubject: {subject}\r\nMessage-ID: <{msg_id}@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n--B\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"report.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQ=\r\n--B--\r\n"
    )
}

struct World {
    home: tempfile::TempDir,
    mailbox: Shared,
    drive: SharedDrive,
    daemon: Child,
    port: u16,
    dport: u16,
    trusted: bool,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

impl World {
    fn call(&self, method: &str, params: Value) -> Result<Value, Value> {
        let mut s = UnixStream::connect(self.home.path().join("mermozd.sock")).unwrap();
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
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if f(self) {
                return;
            }
            let _ = self.call("sync.now", json!({}));
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("timed out waiting for {what}");
    }

    fn thread(&self, subject: &str) -> Value {
        self.ok("threads.list", json!({ "limit": 100 }))["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["subject"] == subject)
            .cloned()
            .unwrap_or_else(|| panic!("no thread {subject}"))
    }

    fn account(&self) -> Value {
        self.ok("accounts.list", json!({}))[0]["id"].clone()
    }

    fn attachment(&self, subject: &str) -> Value {
        let t = self.thread(subject);
        self.ok("threads.get", json!({ "id": t["id"] }))["messages"][0]["attachments"][0]["id"]
            .clone()
    }

    fn cli(&self, args: &[&str]) -> String {
        let bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_mermozd"))
            .parent()
            .unwrap()
            .join("mermoz");
        assert!(bin.exists(), "build the CLI first: cargo build --release");
        let out = Command::new(bin)
            .args(args)
            .env("MERMOZ_HOME", self.home.path())
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    fn wait_for_socket(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(self.home.path().join("mermozd.sock")).is_err() {
            assert!(Instant::now() < deadline, "daemon did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Quits and relaunches mermozd on the same home, as after a Mac restart.
    fn restart(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_file(self.home.path().join("mermozd.sock"));
        self.daemon = spawn_daemon(self.home.path(), self.port, self.dport, self.trusted);
        self.wait_for_socket();
    }

    fn gmail_grant(&self) -> String {
        std::fs::read_to_string(
            self.home
                .path()
                .join("tokens/ai.mermoz.oauth/me@example.com"),
        )
        .unwrap()
    }
}

fn query(u: &str) -> Vec<(String, String)> {
    url::Url::parse(u)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn param(u: &str, k: &str) -> String {
    query(u)
        .into_iter()
        .find(|(kk, _)| kk == k)
        .map(|(_, v)| v)
        .unwrap_or_default()
}

/// Plays the browser: Google redirects back to Mermoz with `answer` as the query.
fn browser_returns(auth_url: &str, answer: &str) {
    let redirect = param(auth_url, "redirect_uri");
    let hostport = redirect.trim_start_matches("http://").to_string();
    let state: String =
        url::form_urlencoded::byte_serialize(param(auth_url, "state").as_bytes()).collect();
    let mut s = std::net::TcpStream::connect(&hostport).unwrap();
    write!(
        s,
        "GET /?{answer}&state={state} HTTP/1.1\r\nHost: {hostport}\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
}

fn sign_in(w: &World) {
    let v = w.ok("accounts.add_gmail", json!({}));
    browser_returns(v["auth_url"].as_str().unwrap(), "code=stub-code");
    w.wait_until("account to be added", |w| {
        w.ok("accounts.list", json!({}))
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
}

fn start_world(trusted: bool) -> World {
    let home = tempfile::tempdir().unwrap();
    let mailbox: Shared = Default::default();
    {
        let mut m = mailbox.lock().unwrap();
        m.email = "me@example.com".into();
        m.history_id = 100;
        m.add_message(
            "m1",
            "t1",
            &["INBOX", "UNREAD"],
            &raw(
                "Marc Petit <marc@acme.example>",
                "Audit report v2",
                &format!(
                    "Please sign off the remediation timeline: https://docs.google.com/document/d/{PLAN}/edit?usp=sharing"
                ),
                "m1",
            ),
            NOW,
        );
        let acme = m.label_id_for("Clients/Acme");
        m.add_message(
            "m2",
            "t2",
            &["INBOX", &acme],
            &raw_with_pdf("Jonas <jonas@acme.example>", "Budget sheet", "m2"),
            NOW - 1000,
        );
        m.add_message(
            "m3",
            "t3",
            &["INBOX"],
            &raw_with_pdf("Léa <lea@example.org>", "Scan", "m3"),
            NOW - 2000,
        );
    }
    let drive: SharedDrive = Default::default();
    {
        let mut d = drive.lock().unwrap();
        d.email = "me@example.com".into();
        d.add_file(
            PLAN,
            StubFile {
                name: "Remediation plan".into(),
                mime: DOC.into(),
                modified: "2026-10-10T16:00:00Z".into(),
                owner: "marc@acme.example".into(),
                readers: vec!["me@example.com".into()],
                content: b"# Remediation plan\n\nPhase 1 ends Oct 30.".to_vec(),
                thumbnail: true,
                ..Default::default()
            },
        );
        d.add_file(
            FORECAST,
            StubFile {
                name: "Q4 forecast".into(),
                mime: SHEET.into(),
                modified: "2026-10-09T10:00:00Z".into(),
                owner: "me@example.com".into(),
                readers: vec!["marc@acme.example".into()],
                content: b"month,revenue\noct,12".to_vec(),
                ..Default::default()
            },
        );
        d.add_file(
            HOLIDAY,
            StubFile {
                name: "Holiday photos".into(),
                mime: "image/jpeg".into(),
                modified: "2026-10-01T10:00:00Z".into(),
                owner: "me@example.com".into(),
                ..Default::default()
            },
        );
    }
    let port = stub_gmail::start(mailbox.clone());
    let dport = stub_drive::start(drive.clone());
    std::fs::write(
        home.path().join("oauth_client.json"),
        json!({ "client_id": "cid", "client_secret": "secret", "auth_url": format!("http://127.0.0.1:{port}/auth"), "token_url": format!("http://127.0.0.1:{port}/token") }).to_string(),
    )
    .unwrap();
    let daemon = spawn_daemon(home.path(), port, dport, trusted);
    let w = World {
        home,
        mailbox,
        drive,
        daemon,
        port,
        dport,
        trusted,
    };
    w.wait_for_socket();
    sign_in(&w);
    w.wait_until("initial import", |w| {
        w.ok("threads.list", json!({}))["total"] == 3
    });
    w
}

fn spawn_daemon(home: &std::path::Path, port: u16, dport: u16, trusted: bool) -> Child {
    let trusted_exe = if trusted {
        std::env::current_exe().unwrap()
    } else {
        "/nonexistent/Mermoz".into()
    };
    Command::new(env!("CARGO_BIN_EXE_mermozd"))
        .env("MERMOZ_HOME", home)
        .env("MERMOZ_TOKEN_STORE", "file")
        .env("MERMOZ_EMBEDDER", "off")
        .env(
            "MERMOZ_GMAIL_BASE",
            format!("http://127.0.0.1:{port}/gmail"),
        )
        .env(
            "MERMOZ_DRIVE_BASE",
            format!("http://127.0.0.1:{dport}/drive"),
        )
        .env(
            "MERMOZ_DRIVE_UPLOAD_BASE",
            format!("http://127.0.0.1:{dport}/upload"),
        )
        .env("MERMOZ_POLL_SECS", "1")
        // Clients the user brings carry no token endpoint: they get Google's, i.e. the stub.
        .env(
            "MERMOZ_OAUTH_TOKEN_URL",
            format!("http://127.0.0.1:{port}/token"),
        )
        .env("MERMOZ_TRUSTED_EXE", trusted_exe)
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
        .unwrap()
}

/// Signed in to Gmail, Drive not connected yet.
#[fixture]
fn mail() -> World {
    start_world(true)
}

/// Starts "Connect Google Drive" and answers Google's consent screen with `granted`
/// (empty: the user said no). Returns the consent URL Mermoz opened.
fn connect_drive(w: &World, granted: &[&str]) -> String {
    let url = w.ok("drive.connect", json!({ "account_id": w.account() }))["auth_url"]
        .as_str()
        .unwrap()
        .to_string();
    w.drive.lock().unwrap().read_all = granted.contains(&READ_ALL);
    if granted.is_empty() {
        browser_returns(&url, "error=access_denied");
    } else {
        w.mailbox.lock().unwrap().granted_scope = Some(granted.join(" "));
        browser_returns(&url, "code=drive-code");
    }
    w.wait_until("the Drive consent to finish", |w| {
        w.ok("drive.status", json!({ "account_id": w.account() }))["attempt"]["state"] != "waiting"
    });
    url
}

/// Signed in to Gmail with Drive connected (both permissions granted).
#[fixture]
fn connected(mail: World) -> World {
    connect_drive(&mail, &[READ_ALL, FILE_ONLY]);
    mail
}

fn status(w: &World) -> Value {
    w.ok("drive.status", json!({ "account_id": w.account() }))
}

fn links(w: &World) -> Value {
    let t = w.thread("Audit report v2");
    w.ok("drive.links", json!({ "thread_id": t["id"] }))["links"][0].clone()
}

// ---- connecting -----------------------------------------------------------

#[rstest]
fn connecting_drive_shows_it_connected(connected: World) {
    assert_eq!(status(&connected)["connected"], true);
}

#[rstest]
fn connecting_asks_for_drive_permissions_only(mail: World) {
    let url = connect_drive(&mail, &[READ_ALL, FILE_ONLY]);
    assert_eq!(param(&url, "scope"), format!("{READ_ALL} {FILE_ONLY}"));
}

#[rstest]
fn connecting_preselects_the_account(mail: World) {
    let url = connect_drive(&mail, &[READ_ALL, FILE_ONLY]);
    assert_eq!(param(&url, "login_hint"), "me@example.com");
}

#[rstest]
fn connecting_keeps_the_mail_sign_in(mail: World) {
    let before = mail.gmail_grant();
    connect_drive(&mail, &[READ_ALL, FILE_ONLY]);
    assert_eq!(mail.gmail_grant(), before);
}

#[rstest]
fn declining_leaves_drive_disconnected(mail: World) {
    connect_drive(&mail, &[]);
    assert_eq!(status(&mail)["connected"], false);
}

#[rstest]
fn declining_is_reported_as_declined(mail: World) {
    connect_drive(&mail, &[]);
    assert_eq!(status(&mail)["attempt"]["state"], "declined");
}

#[rstest]
fn declining_keeps_the_mail_sign_in(mail: World) {
    let before = mail.gmail_grant();
    connect_drive(&mail, &[]);
    assert_eq!(mail.gmail_grant(), before);
}

#[rstest]
fn mail_keeps_syncing_after_declining(mail: World) {
    connect_drive(&mail, &[]);
    mail.mailbox.lock().unwrap().add_message(
        "m9",
        "t9",
        &["INBOX", "UNREAD"],
        &raw("Ann <ann@example.org>", "After the no", "Still here", "m9"),
        NOW + 1000,
    );
    mail.wait_until("new mail", |w| {
        w.ok("threads.list", json!({}))["total"] == 4
    });
}

#[rstest]
fn unticking_every_drive_box_counts_as_declining(mail: World) {
    connect_drive(&mail, &[GMAIL]);
    assert_eq!(status(&mail)["attempt"]["state"], "declined");
}

#[rstest]
fn another_google_account_is_refused(mail: World) {
    mail.drive.lock().unwrap().email = "someone.else@example.org".into();
    connect_drive(&mail, &[READ_ALL, FILE_ONLY]);
    assert_eq!(status(&mail)["connected"], false);
}

#[rstest]
fn a_partial_grant_says_what_is_missing(mail: World) {
    connect_drive(&mail, &[FILE_ONLY]);
    assert_eq!(
        (
            status(&mail)["can_read_all"].clone(),
            status(&mail)["can_save"].clone()
        ),
        (json!(false), json!(true))
    );
}

#[rstest]
fn disconnecting_forgets_drive(connected: World) {
    connected.ok(
        "drive.disconnect",
        json!({ "account_id": connected.account() }),
    );
    assert_eq!(status(&connected)["connected"], false);
}

#[rstest]
fn disconnecting_keeps_the_mail_sign_in(connected: World) {
    let before = connected.gmail_grant();
    connected.ok(
        "drive.disconnect",
        json!({ "account_id": connected.account() }),
    );
    assert_eq!(connected.gmail_grant(), before);
}

#[rstest]
fn agents_cannot_connect_drive() {
    let w = start_world(false);
    let e = w
        .call("drive.connect", json!({ "account_id": w.account() }))
        .unwrap_err();
    assert_eq!(e["code"], -32011);
}

// ---- linked documents -----------------------------------------------------

#[rstest]
fn a_linked_doc_is_found_before_drive_is_connected(mail: World) {
    assert_eq!(links(&mail)["access"], "not_connected");
}

#[rstest]
fn a_linked_doc_shows_its_title(connected: World) {
    assert_eq!(links(&connected)["file"]["name"], "Remediation plan");
}

#[rstest]
fn a_linked_doc_shows_its_owner(connected: World) {
    assert_eq!(
        links(&connected)["file"]["owner"]["email"],
        "marc@acme.example"
    );
}

#[rstest]
fn a_linked_doc_shows_its_type(connected: World) {
    assert_eq!(links(&connected)["file"]["kind"], "Google Doc");
}

#[rstest]
fn a_linked_doc_shows_when_it_changed(connected: World) {
    assert_eq!(links(&connected)["file"]["modified"], 1_791_648_000_000i64);
}

#[rstest]
fn a_linked_doc_shows_in_every_thread_of_its_conversation(connected: World) {
    let (audit, scan) = (
        connected.thread("Audit report v2"),
        connected.thread("Scan"),
    );
    connected.ok(
        "conversations.merge",
        json!({ "ids": [audit["id"], scan["id"]] }),
    );
    let v = connected.ok("drive.links", json!({ "thread_id": scan["id"] }));
    assert_eq!(v["links"][0]["file"]["name"], "Remediation plan");
}

#[rstest]
fn a_linked_doc_is_out_of_reach_with_chosen_files_only(mail: World) {
    connect_drive(&mail, &[FILE_ONLY]);
    assert_eq!(links(&mail)["access"], "no_access");
}

#[rstest]
fn a_linked_doc_has_a_thumbnail(connected: World) {
    let v = connected.ok(
        "drive.thumbnail",
        json!({ "account_id": connected.account(), "file_id": PLAN }),
    );
    assert_eq!(v["mime"], "image/png");
}

#[rstest]
fn a_linked_doc_is_read_as_markdown(connected: World) {
    let v = connected.ok(
        "drive.content",
        json!({ "account_id": connected.account(), "file_id": PLAN }),
    );
    assert_eq!(v["text"], "# Remediation plan\n\nPhase 1 ends Oct 30.");
}

#[rstest]
fn a_doc_is_exported_once_then_read_from_the_cache(connected: World) {
    for _ in 0..2 {
        connected.ok(
            "drive.content",
            json!({ "account_id": connected.account(), "file_id": PLAN }),
        );
    }
    assert_eq!(
        connected.drive.lock().unwrap().exports,
        vec![(PLAN.to_string(), "text/markdown".to_string())]
    );
}

#[rstest]
fn chat_about_this_reads_the_linked_doc(connected: World) {
    let pr = connected.ok(
        "providers.save",
        json!({ "name": "Work AI", "kind": "anthropic", "model": "claude-opus-5-5",
        "base_url": format!("http://127.0.0.1:{}", connected.port), "api_key": "sk-test" }),
    );
    connected.ok(
        "accounts.agent.set",
        json!({ "account_id": connected.account(), "provider_ids": [pr["id"]] }),
    );
    let t = connected.thread("Audit report v2");
    connected.ok(
        "chat.send",
        json!({ "thread_id": t["id"], "text": "Is the plan realistic?", "research": false }),
    );
    let m = connected.mailbox.lock().unwrap();
    assert!(
        m.chat_requests[0]["system"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("Phase 1 ends Oct 30.")
    );
}

// ---- the thread's documents ------------------------------------------------

fn files_for_thread(w: &World, subject: &str) -> Value {
    let t = w.thread(subject);
    w.ok("drive.files_for_thread", json!({ "thread_id": t["id"] }))
}

fn names(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect()
}

#[rstest]
fn thread_documents_list_files_shared_with_its_people(connected: World) {
    assert_eq!(
        names(&files_for_thread(&connected, "Audit report v2")["shared"]),
        vec!["Q4 forecast"]
    );
}

#[rstest]
fn thread_documents_list_linked_files(connected: World) {
    assert_eq!(
        files_for_thread(&connected, "Audit report v2")["linked"][0]["file"]["name"],
        "Remediation plan"
    );
}

#[rstest]
fn files_for_people_finds_what_marc_can_open(connected: World) {
    let v = connected.ok(
        "drive.files_for_people",
        json!({ "account_id": connected.account(), "emails": ["marc@acme.example"] }),
    );
    assert_eq!(names(&v["files"]), vec!["Remediation plan", "Q4 forecast"]);
}

#[rstest]
fn people_search_needs_the_read_permission(mail: World) {
    connect_drive(&mail, &[FILE_ONLY]);
    let e = mail
        .call(
            "drive.files_for_people",
            json!({ "account_id": mail.account(), "emails": ["marc@acme.example"] }),
        )
        .unwrap_err();
    assert_eq!(e["data"]["reason"], "read_all");
}

// ---- save to Drive -----------------------------------------------------------

fn save(w: &World, subject: &str) -> Value {
    let att = w.attachment(subject);
    w.ok("drive.save_attachment", json!({ "attachment_id": att }))
}

#[rstest]
fn saving_files_it_under_the_project(connected: World) {
    assert_eq!(
        save(&connected, "Budget sheet")["folder"],
        "Mermoz/Clients/Acme"
    );
}

#[rstest]
fn saving_puts_the_file_in_the_project_folder(connected: World) {
    let saved = save(&connected, "Budget sheet");
    let d = connected.drive.lock().unwrap();
    let acme = d.folders_named("Acme")[0].0.clone();
    assert_eq!(
        d.files[saved["file_id"].as_str().unwrap()].parents,
        vec![acme]
    );
}

#[rstest]
fn saving_keeps_the_file_content(connected: World) {
    let saved = save(&connected, "Budget sheet");
    assert_eq!(
        connected.drive.lock().unwrap().files[saved["file_id"].as_str().unwrap()].content,
        b"%PDF-1.4".to_vec()
    );
}

#[rstest]
fn saving_gives_an_open_in_drive_link(connected: World) {
    let saved = save(&connected, "Budget sheet");
    assert_eq!(
        saved["web_link"],
        format!(
            "https://drive.example/{}",
            saved["file_id"].as_str().unwrap()
        )
    );
}

#[rstest]
fn saving_without_a_project_uses_no_project(connected: World) {
    assert_eq!(save(&connected, "Scan")["folder"], "Mermoz/No project");
}

#[rstest]
fn saving_twice_uploads_once(connected: World) {
    save(&connected, "Budget sheet");
    save(&connected, "Budget sheet");
    assert_eq!(connected.drive.lock().unwrap().uploads, 1);
}

#[rstest]
fn two_saves_share_one_mermoz_folder(connected: World) {
    save(&connected, "Budget sheet");
    save(&connected, "Scan");
    assert_eq!(
        connected
            .drive
            .lock()
            .unwrap()
            .folders_named("Mermoz")
            .len(),
        1
    );
}

#[rstest]
fn a_deleted_folder_is_made_again(connected: World) {
    save(&connected, "Budget sheet");
    {
        let mut d = connected.drive.lock().unwrap();
        let ids: Vec<String> = d.folders_named("Mermoz").into_iter().map(|f| f.0).collect();
        for id in ids {
            d.files.remove(&id);
        }
    }
    assert_eq!(save(&connected, "Scan")["folder"], "Mermoz/No project");
}

#[rstest]
fn saved_attachments_are_listed_with_the_thread(connected: World) {
    save(&connected, "Budget sheet");
    let t = connected.thread("Budget sheet");
    assert_eq!(
        connected.ok("drive.saved", json!({ "thread_id": t["id"] }))[0]["folder"],
        "Mermoz/Clients/Acme"
    );
}

#[rstest]
fn saving_without_drive_asks_to_connect(mail: World) {
    let att = mail.attachment("Budget sheet");
    let e = mail
        .call("drive.save_attachment", json!({ "attachment_id": att }))
        .unwrap_err();
    assert_eq!(e["code"], -32030);
}

// ---- attach from Drive -------------------------------------------------------

#[rstest]
fn recent_files_come_first_without_words(connected: World) {
    let v = connected.ok("drive.search", json!({ "account_id": connected.account() }));
    assert_eq!(names(&v["files"]).len(), 3);
}

#[rstest]
fn search_finds_a_file_by_name(connected: World) {
    let v = connected.ok(
        "drive.search",
        json!({ "account_id": connected.account(), "query": "forecast" }),
    );
    assert_eq!(names(&v["files"]), vec!["Q4 forecast"]);
}

/// Opens Google's picker and plays the user choosing `ids`.
fn pick(w: &World, ids: &[&str]) -> (String, Value) {
    let v = w.ok("drive.pick", json!({ "account_id": w.account() }));
    let url = v["auth_url"].as_str().unwrap().to_string();
    {
        let mut d = w.drive.lock().unwrap();
        for id in ids {
            d.app_files.insert(id.to_string());
        }
    }
    w.mailbox.lock().unwrap().granted_scope = Some(FILE_ONLY.into());
    browser_returns(
        &url,
        &format!("code=pick-code&picked_file_ids={}", ids.join("%2C")),
    );
    let mut result = Value::Null;
    w.wait_until("the pick to finish", |w| {
        result = w.ok("drive.pick_result", json!({ "pick_id": v["pick_id"] }));
        result["state"] != "waiting"
    });
    (url, result)
}

#[rstest]
fn the_picker_opens_googles_file_chooser(connected: World) {
    assert_eq!(
        param(&pick(&connected, &[HOLIDAY]).0, "trigger_onepick"),
        "true"
    );
}

#[rstest]
fn the_picker_asks_for_chosen_files_only(connected: World) {
    assert_eq!(param(&pick(&connected, &[HOLIDAY]).0, "scope"), FILE_ONLY);
}

#[rstest]
fn the_picker_asks_for_no_long_lived_token_once_connected(connected: World) {
    assert_eq!(
        param(&pick(&connected, &[HOLIDAY]).0, "access_type"),
        "online"
    );
}

#[rstest]
fn picked_files_come_back_with_their_details(connected: World) {
    assert_eq!(
        names(&pick(&connected, &[HOLIDAY]).1["files"]),
        vec!["Holiday photos"]
    );
}

#[rstest]
fn picking_without_drive_connects_chosen_files_only(mail: World) {
    pick(&mail, &[HOLIDAY]);
    assert_eq!(
        (
            status(&mail)["connected"].clone(),
            status(&mail)["can_read_all"].clone()
        ),
        (json!(true), json!(false))
    );
}

#[rstest]
fn a_cancelled_pick_is_declined(connected: World) {
    let v = connected.ok("drive.pick", json!({ "account_id": connected.account() }));
    browser_returns(v["auth_url"].as_str().unwrap(), "error=access_denied");
    let mut result = Value::Null;
    connected.wait_until("the pick to finish", |w| {
        result = w.ok("drive.pick_result", json!({ "pick_id": v["pick_id"] }));
        result["state"] != "waiting"
    });
    assert_eq!(result["state"], "declined");
}

#[rstest]
fn a_picked_file_can_be_shared_with_recipients(connected: World) {
    pick(&connected, &[HOLIDAY]);
    connected.ok(
        "drive.share",
        json!({ "account_id": connected.account(), "file_id": HOLIDAY,
                "emails": ["marc@acme.example"], "role": "commenter" }),
    );
    assert_eq!(
        connected.drive.lock().unwrap().permissions,
        vec![(
            HOLIDAY.to_string(),
            "marc@acme.example".to_string(),
            "commenter".to_string()
        )]
    );
}

#[rstest]
fn a_file_not_picked_must_be_picked_before_sharing(connected: World) {
    let e = connected
        .call(
            "drive.share",
            json!({ "account_id": connected.account(), "file_id": FORECAST,
                    "emails": ["marc@acme.example"], "role": "reader" }),
        )
        .unwrap_err();
    assert_eq!(e["data"]["reason"], "pick");
}

#[rstest]
fn sharing_never_happens_without_asking(connected: World) {
    let _ = connected.call(
        "drive.share",
        json!({ "account_id": connected.account(), "file_id": FORECAST,
                "emails": ["marc@acme.example"], "role": "reader" }),
    );
    assert!(connected.drive.lock().unwrap().permissions.is_empty());
}

#[rstest]
fn sharing_accepts_only_view_or_comment(connected: World) {
    let e = connected
        .call(
            "drive.share",
            json!({ "account_id": connected.account(), "file_id": HOLIDAY,
                    "emails": ["marc@acme.example"], "role": "writer" }),
        )
        .unwrap_err();
    assert_eq!(e["code"], -32602);
}

#[rstest]
fn agents_cannot_change_sharing() {
    let w = start_world(false);
    let e = w
        .call(
            "drive.share",
            json!({ "account_id": w.account(), "file_id": HOLIDAY,
                    "emails": ["marc@acme.example"], "role": "reader" }),
        )
        .unwrap_err();
    assert_eq!(e["code"], -32011);
}

// ---- attaching a Drive file as a regular file ------------------------------

/// A scanned notice stored in Drive as a PDF.
const NOTICE: &str = "1NoticeScanPdfAbCdEfGhIjKlMnOpQ";
/// A certificate written as a Google Doc.
const CERTIFICATE: &str = "1CertificateDocAbCdEfGhIjKlMnOp";
/// A video Drive says is 40 MB.
const VIDEO: &str = "1SiteVisitVideoAbCdEfGhIjKlMnOp";
/// A Google Doc whose PDF is bigger than Gmail allows.
const HUGE_DOC: &str = "1HugeMinutesDocAbCdEfGhIjKlMnOp";
/// A Google Form: no file form exists.
const SURVEY: &str = "1SurveyFormAbCdEfGhIjKlMnOpQrSt";
/// Ten-MB PDFs: three of them don't fit in one email.
const SCANS: [&str; 3] = [
    "1ScanOnePdfAbCdEfGhIjKlMnOpQrSt",
    "1ScanTwoPdfAbCdEfGhIjKlMnOpQrSt",
    "1ScanThreePdfAbCdEfGhIjKlMnOpQr",
];
const NOTICE_BYTES: &[u8] = b"%PDF-1.4 fictional notice of assessment";
const CERTIFICATE_PDF: &[u8] = b"%PDF-1.7 fictional certificate";
const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// Drive connected, with the files of a fictional filing to a public office.
#[fixture]
fn filing(connected: World) -> World {
    {
        let mut d = connected.drive.lock().unwrap();
        let file = |name: &str, mime: &str, content: Vec<u8>| StubFile {
            name: name.into(),
            mime: mime.into(),
            modified: "2026-10-12T08:00:00Z".into(),
            owner: "me@example.com".into(),
            content,
            ..Default::default()
        };
        d.add_file(
            NOTICE,
            file("Avis 2026.pdf", "application/pdf", NOTICE_BYTES.to_vec()),
        );
        d.add_file(
            CERTIFICATE,
            file("Attestation", DOC, CERTIFICATE_PDF.to_vec()),
        );
        d.add_file(
            VIDEO,
            StubFile {
                declared_size: Some(40 * 1024 * 1024),
                ..file("Visit.mov", "video/quicktime", b"fictional".to_vec())
            },
        );
        d.add_file(HUGE_DOC, file("Minutes", DOC, vec![b'x'; 26 * 1024 * 1024]));
        d.add_file(
            SURVEY,
            file("Survey", "application/vnd.google-apps.form", vec![]),
        );
        for (i, id) in SCANS.iter().enumerate() {
            d.add_file(
                id,
                file(
                    &format!("Scan {i}.pdf"),
                    "application/pdf",
                    vec![b'%'; 10 * 1024 * 1024],
                ),
            );
        }
    }
    connected
}

fn download(w: &World, file_id: &str, format: Option<&str>) -> Result<Value, Value> {
    let mut p = json!({ "account_id": w.account(), "file_id": file_id });
    if let Some(f) = format {
        p["format"] = json!(f);
    }
    w.call("drive.download", p)
}

fn downloaded_bytes(v: &Value) -> Vec<u8> {
    std::fs::read(v["path"].as_str().unwrap()).unwrap()
}

/// A new draft to Marc carrying the downloaded files.
fn draft_with(w: &World, downloads: &[Value]) -> Result<Value, Value> {
    let attachments: Vec<Value> = downloads
        .iter()
        .map(|d| json!({ "path": d["path"], "filename": d["filename"] }))
        .collect();
    w.call(
        "drafts.create",
        json!({ "account_id": w.account(), "to": ["marc@acme.example"], "subject": "Dossier",
                "body": "Please find the documents attached.", "attachments": attachments }),
    )
}

fn last_sent(w: &World) -> mail_parser::Message<'static> {
    let raw = w.mailbox.lock().unwrap().sent_raw.last().cloned().unwrap();
    mail_parser::MessageParser::default()
        .parse(&raw)
        .unwrap()
        .into_owned()
}

/// The notice and the certificate (as PDF) sent to Marc as files.
fn send_both(w: &World) -> mail_parser::Message<'static> {
    let notice = download(w, NOTICE, None).unwrap();
    let certificate = download(w, CERTIFICATE, None).unwrap();
    let d = draft_with(w, &[notice, certificate]).unwrap();
    w.ok("drafts.send", json!({ "id": d["id"] }));
    last_sent(w)
}

#[rstest]
fn a_stored_pdf_downloads_with_its_bytes(filing: World) {
    let v = download(&filing, NOTICE, None).unwrap();
    assert_eq!(downloaded_bytes(&v), NOTICE_BYTES);
}

#[rstest]
fn a_stored_pdf_is_fetched_as_media(filing: World) {
    download(&filing, NOTICE, None).unwrap();
    assert_eq!(filing.drive.lock().unwrap().downloads, vec![NOTICE]);
}

#[rstest]
fn a_stored_pdf_keeps_its_name(filing: World) {
    assert_eq!(
        download(&filing, NOTICE, None).unwrap()["filename"],
        "Avis 2026.pdf"
    );
}

#[rstest]
fn a_stored_pdf_reports_its_size(filing: World) {
    assert_eq!(
        download(&filing, NOTICE, None).unwrap()["size"],
        NOTICE_BYTES.len()
    );
}

#[rstest]
fn a_google_doc_is_exported_to_pdf(filing: World) {
    download(&filing, CERTIFICATE, None).unwrap();
    assert_eq!(
        filing.drive.lock().unwrap().exports,
        vec![(CERTIFICATE.to_string(), "application/pdf".to_string())]
    );
}

#[rstest]
fn an_exported_doc_is_named_as_a_pdf(filing: World) {
    assert_eq!(
        download(&filing, CERTIFICATE, None).unwrap()["filename"],
        "Attestation.pdf"
    );
}

#[rstest]
fn an_exported_doc_carries_the_export(filing: World) {
    let v = download(&filing, CERTIFICATE, None).unwrap();
    assert_eq!(downloaded_bytes(&v), CERTIFICATE_PDF);
}

#[rstest]
fn a_google_doc_can_be_exported_to_word(filing: World) {
    assert_eq!(
        download(&filing, CERTIFICATE, Some("docx")).unwrap()["mime"],
        DOCX_MIME
    );
}

#[rstest]
fn the_draft_holds_the_downloaded_bytes(filing: World) {
    let v = download(&filing, NOTICE, None).unwrap();
    let d = draft_with(&filing, &[v]).unwrap();
    assert_eq!(
        std::fs::read(d["attachments"][0]["path"].as_str().unwrap()).unwrap(),
        NOTICE_BYTES
    );
}

#[rstest]
fn the_draft_holds_a_file_not_a_link(filing: World) {
    let v = download(&filing, CERTIFICATE, None).unwrap();
    let d = draft_with(&filing, &[v]).unwrap();
    assert_eq!(d["attachments"][0]["kind"], "file");
}

#[rstest]
fn the_draft_holds_the_pdf_type_of_an_export(filing: World) {
    let v = download(&filing, CERTIFICATE, None).unwrap();
    let d = draft_with(&filing, &[v]).unwrap();
    assert_eq!(d["attachments"][0]["mime"], "application/pdf");
}

#[rstest]
fn the_draft_holds_the_word_type_of_an_export(filing: World) {
    let v = download(&filing, CERTIFICATE, Some("docx")).unwrap();
    let d = draft_with(&filing, &[v]).unwrap();
    assert_eq!(d["attachments"][0]["mime"], DOCX_MIME);
}

#[rstest]
fn gmail_receives_the_stored_pdf_bytes(filing: World) {
    let m = send_both(&filing);
    assert_eq!(m.attachment(0).unwrap().contents(), NOTICE_BYTES);
}

#[rstest]
fn gmail_receives_the_exported_pdf_bytes(filing: World) {
    let m = send_both(&filing);
    assert_eq!(m.attachment(1).unwrap().contents(), CERTIFICATE_PDF);
}

#[rstest]
fn gmail_receives_the_export_as_a_pdf(filing: World) {
    use mail_parser::MimeHeaders;
    let m = send_both(&filing);
    assert_eq!(
        m.attachment(1).unwrap().content_type().unwrap().subtype(),
        Some("pdf")
    );
}

#[rstest]
fn gmail_receives_a_multipart_message(filing: World) {
    let m = send_both(&filing);
    assert_eq!(m.attachment_count(), 2);
}

#[rstest]
fn sending_files_shares_nothing(filing: World) {
    send_both(&filing);
    assert!(filing.drive.lock().unwrap().permissions.is_empty());
}

#[rstest]
fn a_file_over_the_limit_is_refused(filing: World) {
    assert_eq!(
        download(&filing, VIDEO, None).unwrap_err()["data"]["reason"],
        "too_large"
    );
}

#[rstest]
fn a_file_over_the_limit_is_not_downloaded(filing: World) {
    let _ = download(&filing, VIDEO, None);
    assert!(filing.drive.lock().unwrap().downloads.is_empty());
}

#[rstest]
fn a_file_over_the_limit_is_explained(filing: World) {
    assert_eq!(
        download(&filing, VIDEO, None).unwrap_err()["message"],
        "“Visit.mov” is 40.0 MB, more than the 25.0 MB Gmail can send in one email. Keep it as a link."
    );
}

#[rstest]
fn an_export_over_the_limit_is_refused(filing: World) {
    assert_eq!(
        download(&filing, HUGE_DOC, None).unwrap_err()["data"]["reason"],
        "too_large"
    );
}

#[rstest]
fn a_form_cannot_become_a_file(filing: World) {
    assert_eq!(
        download(&filing, SURVEY, None).unwrap_err()["message"],
        "A Google Form can't be attached as a file. Keep it as a link."
    );
}

#[rstest]
fn a_file_mermoz_cannot_open_is_explained(filing: World) {
    assert_eq!(
        download(&filing, "1MissingFileAbCdEfGhIjKlMnOpQr", None).unwrap_err()["data"]["reason"],
        "no_access"
    );
}

#[rstest]
fn downloaded_files_count_toward_the_email_limit(filing: World) {
    let scans: Vec<Value> = SCANS
        .iter()
        .map(|id| download(&filing, id, None).unwrap())
        .collect();
    assert!(
        draft_with(&filing, &scans).unwrap_err()["message"]
            .as_str()
            .unwrap()
            .starts_with("These files add up to 30.0 MB")
    );
}

// ---- agents (CLI) ----------------------------------------------------------

#[rstest]
fn cli_reads_a_linked_doc(connected: World) {
    assert_eq!(
        connected.cli(&["drive", "read", PLAN]),
        "# Remediation plan\n\nPhase 1 ends Oct 30.\n"
    );
}

#[rstest]
fn cli_lists_a_threads_drive_files(connected: World) {
    let t = connected.thread("Audit report v2");
    let v: Value = serde_json::from_str(&connected.cli(&[
        "--json",
        "drive",
        "files",
        t["number"].as_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(v["shared"][0]["name"], "Q4 forecast");
}

#[rstest]
fn cli_saves_an_attachment(connected: World) {
    let att = connected.attachment("Budget sheet");
    assert!(
        connected
            .cli(&["drive", "save", att.as_str().unwrap()])
            .starts_with("saved in Mermoz/Clients/Acme: https://drive.example/")
    );
}

// ---- an account signed in through another OAuth client ---------------------

/// Fictional client of another Google Cloud project, borrowed during development.
const OTHER_CLIENT: &str = "other-project-cid";

fn grant_client(raw: &str) -> String {
    serde_json::from_str::<Value>(raw).unwrap()["client_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn refreshed_with(w: &World, client_id: &str) -> bool {
    w.mailbox
        .lock()
        .unwrap()
        .token_requests
        .iter()
        .any(|(grant, id)| grant == "refresh_token" && id == client_id)
}

fn needs_reconnect(w: &World) -> Value {
    w.ok("accounts.list", json!({}))[0]["needs_reconnect"].clone()
}

/// The Gmail token was issued by another client than the app's ("cid"), then
/// Mermoz restarted.
#[fixture]
fn legacy(mail: World) -> World {
    let mut w = mail;
    let _ = w.daemon.kill();
    let _ = w.daemon.wait();
    std::fs::write(
        w.home.path().join("tokens/ai.mermoz.oauth/me@example.com"),
        json!({
            "refresh_token": "other-refresh",
            "client_id": OTHER_CLIENT,
            "client_secret": "other-secret",
            "token_url": format!("http://127.0.0.1:{}/token", w.port),
        })
        .to_string(),
    )
    .unwrap();
    w.mailbox.lock().unwrap().token_requests.clear();
    w.restart();
    w.wait_until("the old token to refresh", |w| {
        refreshed_with(w, OTHER_CLIENT)
    });
    w
}

#[rstest]
fn fresh_sign_in_does_not_need_reconnect(mail: World) {
    assert_eq!(needs_reconnect(&mail), false);
}

#[rstest]
fn legacy_token_is_reported_as_needing_reconnect(legacy: World) {
    assert_eq!(needs_reconnect(&legacy), true);
}

#[rstest]
fn legacy_token_never_refreshes_with_the_app_client(legacy: World) {
    assert!(!refreshed_with(&legacy, "cid"));
}

#[rstest]
fn legacy_token_keeps_mail_syncing(legacy: World) {
    legacy.mailbox.lock().unwrap().add_message(
        "m9",
        "t9",
        &["INBOX", "UNREAD"],
        &raw("Ann <ann@example.org>", "Still syncing", "Hi", "m9"),
        NOW + 1000,
    );
    legacy.wait_until("new mail", |w| {
        w.ok("threads.list", json!({}))["total"] == 4
    });
}

#[rstest]
fn cli_asks_to_sign_in_again(legacy: World) {
    assert!(legacy.cli(&["accounts"]).contains("sign in again"));
}

#[rstest]
fn drive_consent_uses_the_app_client_with_a_legacy_token(legacy: World) {
    let url = connect_drive(&legacy, &[READ_ALL, FILE_ONLY]);
    assert_eq!(param(&url, "client_id"), "cid");
}

#[rstest]
fn drive_grant_records_the_app_client(legacy: World) {
    connect_drive(&legacy, &[READ_ALL, FILE_ONLY]);
    let raw = std::fs::read_to_string(
        legacy
            .home
            .path()
            .join("tokens/ai.mermoz.drive/me@example.com"),
    )
    .unwrap();
    assert_eq!(grant_client(&raw), "cid");
}

#[rstest]
fn drive_token_refreshes_with_its_own_client(legacy: World) {
    let mut w = legacy;
    connect_drive(&w, &[READ_ALL, FILE_ONLY]);
    w.restart();
    w.mailbox.lock().unwrap().token_requests.clear();
    w.ok("drive.search", json!({ "account_id": w.account() }));
    assert!(refreshed_with(&w, "cid"));
}

#[rstest]
fn file_pick_uses_the_app_client_with_a_legacy_token(legacy: World) {
    let v = legacy.ok("drive.pick", json!({ "account_id": legacy.account() }));
    assert_eq!(param(v["auth_url"].as_str().unwrap(), "client_id"), "cid");
}

/// Starts "Sign in again" and answers Google's consent screen.
fn reconnect(w: &World) -> String {
    let url = w.ok("accounts.reconnect", json!({ "account_id": w.account() }))["auth_url"]
        .as_str()
        .unwrap()
        .to_string();
    browser_returns(&url, "code=reconnect-code");
    w.wait_until("the account to be signed in again", |w| {
        needs_reconnect(w) == false
    });
    url
}

#[rstest]
fn reconnect_consent_uses_the_app_client(legacy: World) {
    let url = reconnect(&legacy);
    assert_eq!(param(&url, "client_id"), "cid");
}

#[rstest]
fn reconnect_preselects_the_account(legacy: World) {
    let url = reconnect(&legacy);
    assert_eq!(param(&url, "login_hint"), "me@example.com");
}

#[rstest]
fn reconnect_replaces_the_token(legacy: World) {
    reconnect(&legacy);
    assert_eq!(grant_client(&legacy.gmail_grant()), "cid");
}

#[rstest]
fn reconnect_keeps_local_mail(legacy: World) {
    reconnect(&legacy);
    assert_eq!(legacy.ok("threads.list", json!({}))["total"], 3);
}

#[rstest]
fn mail_keeps_syncing_after_reconnect(legacy: World) {
    reconnect(&legacy);
    legacy.mailbox.lock().unwrap().add_message(
        "m9",
        "t9",
        &["INBOX", "UNREAD"],
        &raw("Ann <ann@example.org>", "After reconnect", "Hi", "m9"),
        NOW + 1000,
    );
    legacy.wait_until("new mail", |w| {
        w.ok("threads.list", json!({}))["total"] == 4
    });
}

// ---- an account on the user's own sign-in client ----------------------------

/// Fictional client of a company's own Google Cloud project.
const COMPANY_CLIENT: &str = "company-project-cid.apps.googleusercontent.com";

/// The client_secret_*.json Google Cloud downloads for a Desktop app client.
fn company_client_file() -> String {
    json!({ "installed": {
        "client_id": COMPANY_CLIENT,
        "project_id": "company-project",
        "auth_uri": "https://accounts.google.com/o/oauth2/auth",
        "token_uri": "https://oauth2.googleapis.com/token",
        "client_secret": "company-secret",
        "redirect_uris": ["http://localhost"],
    }})
    .to_string()
}

/// Adds the account through `params`' client and answers Google's consent screen.
/// Returns the consent URL Mermoz opened.
fn add_with_own_client(w: &World, params: Value) -> String {
    let url = w.ok("accounts.add_gmail", params)["auth_url"]
        .as_str()
        .unwrap()
        .to_string();
    browser_returns(&url, "code=company-code");
    w.wait_until("the account to be on the company client", |w| {
        grant_client(&w.gmail_grant()) == COMPANY_CLIENT
    });
    url
}

/// Signed in through the company's client, then Mermoz restarted.
#[fixture]
fn own_client(mail: World) -> World {
    let mut w = mail;
    add_with_own_client(&w, json!({ "client_json": company_client_file() }));
    let _ = w.daemon.kill();
    let _ = w.daemon.wait();
    w.mailbox.lock().unwrap().token_requests.clear();
    w.restart();
    w.wait_until("the token to refresh", |w| {
        refreshed_with(w, COMPANY_CLIENT)
    });
    w
}

#[rstest]
fn own_client_consent_uses_that_client(mail: World) {
    let url = add_with_own_client(&mail, json!({ "client_json": company_client_file() }));
    assert_eq!(param(&url, "client_id"), COMPANY_CLIENT);
}

#[rstest]
fn pasted_client_id_and_secret_are_used(mail: World) {
    let url = add_with_own_client(
        &mail,
        json!({ "client_id": COMPANY_CLIENT, "client_secret": "company-secret" }),
    );
    assert_eq!(param(&url, "client_id"), COMPANY_CLIENT);
}

#[rstest]
fn own_client_is_stored_with_the_token(own_client: World) {
    assert_eq!(grant_client(&own_client.gmail_grant()), COMPANY_CLIENT);
}

#[rstest]
fn own_client_is_marked_as_chosen(own_client: World) {
    let g: Value = serde_json::from_str(&own_client.gmail_grant()).unwrap();
    assert_eq!(g["custom_client"], true);
}

#[rstest]
fn own_client_token_never_refreshes_with_the_app_client(own_client: World) {
    assert!(!refreshed_with(&own_client, "cid"));
}

#[rstest]
fn own_client_does_not_need_reconnect(own_client: World) {
    assert_eq!(needs_reconnect(&own_client), false);
}

#[rstest]
fn own_client_keeps_mail_syncing(own_client: World) {
    own_client.mailbox.lock().unwrap().add_message(
        "m9",
        "t9",
        &["INBOX", "UNREAD"],
        &raw("Ann <ann@example.org>", "Company mail", "Hi", "m9"),
        NOW + 1000,
    );
    own_client.wait_until("new mail", |w| {
        w.ok("threads.list", json!({}))["total"] == 4
    });
}

#[rstest]
fn drive_consent_uses_the_own_client(own_client: World) {
    let url = connect_drive(&own_client, &[READ_ALL, FILE_ONLY]);
    assert_eq!(param(&url, "client_id"), COMPANY_CLIENT);
}

#[rstest]
fn drive_grant_records_the_own_client(own_client: World) {
    connect_drive(&own_client, &[READ_ALL, FILE_ONLY]);
    let raw = std::fs::read_to_string(
        own_client
            .home
            .path()
            .join("tokens/ai.mermoz.drive/me@example.com"),
    )
    .unwrap();
    assert_eq!(grant_client(&raw), COMPANY_CLIENT);
}

#[rstest]
fn drive_token_refreshes_with_the_own_client(own_client: World) {
    let mut w = own_client;
    connect_drive(&w, &[READ_ALL, FILE_ONLY]);
    w.restart();
    w.mailbox.lock().unwrap().token_requests.clear();
    w.ok("drive.search", json!({ "account_id": w.account() }));
    assert!(refreshed_with(&w, COMPANY_CLIENT));
}

#[rstest]
fn file_pick_uses_the_own_client(own_client: World) {
    let v = own_client.ok("drive.pick", json!({ "account_id": own_client.account() }));
    assert_eq!(
        param(v["auth_url"].as_str().unwrap(), "client_id"),
        COMPANY_CLIENT
    );
}

#[rstest]
fn signing_in_again_keeps_the_own_client(own_client: World) {
    let url = own_client.ok(
        "accounts.reconnect",
        json!({ "account_id": own_client.account() }),
    )["auth_url"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(param(&url, "client_id"), COMPANY_CLIENT);
}

#[rstest]
fn a_file_that_is_not_a_client_is_refused(mail: World) {
    let err = mail
        .call("accounts.add_gmail", json!({ "client_json": "not json" }))
        .unwrap_err();
    assert_eq!(err["code"], -32602);
}

// ---- Google refusing a company account ---------------------------------------

/// Starts a sign-in and plays Google redirecting back with `answer`; returns
/// `accounts.sign_in_status` once it settles.
fn refused(w: &World, params: Value, answer: &str) -> Value {
    let url = w.ok("accounts.add_gmail", params)["auth_url"]
        .as_str()
        .unwrap()
        .to_string();
    browser_returns(&url, answer);
    let mut status = Value::Null;
    w.wait_until("the sign-in to settle", |w| {
        status = w.ok("accounts.sign_in_status", json!({}));
        status["running"] == false
    });
    status
}

#[rstest]
fn admin_policy_is_explained(mail: World) {
    let s = refused(&mail, json!({}), "error=admin_policy_enforced");
    assert_eq!(
        s["error"],
        "Your organization only allows apps it trusts. Ask your Google Workspace admin to trust Mermoz (client ID below), or use your own sign-in client (Settings › Developer)."
    );
}

#[rstest]
fn admin_policy_shows_the_client_to_trust(mail: World) {
    let s = refused(&mail, json!({}), "error=admin_policy_enforced");
    assert_eq!(s["problem"]["client_id"], "cid");
}

#[rstest]
fn app_access_control_block_is_recognised(mail: World) {
    let s = refused(
        &mail,
        json!({}),
        "error=access_denied&error_description=Your+admin+has+blocked+access",
    );
    assert_eq!(s["problem"]["kind"], "app_blocked");
}

#[rstest]
fn own_client_refusal_names_the_own_client(mail: World) {
    let s = refused(
        &mail,
        json!({ "client_json": company_client_file() }),
        "error=org_internal",
    );
    assert_eq!(s["problem"]["client_id"], COMPANY_CLIENT);
}

#[rstest]
fn the_app_client_is_shown_before_any_refusal(mail: World) {
    assert_eq!(
        mail.ok("accounts.sign_in_status", json!({}))["app_client_id"],
        "cid"
    );
}

#[rstest]
fn a_successful_sign_in_clears_the_problem(mail: World) {
    refused(&mail, json!({}), "error=admin_policy_enforced");
    let s = refused(&mail, json!({}), "code=stub-code");
    assert_eq!(s["problem"], Value::Null);
}
