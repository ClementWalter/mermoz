//! End-to-end: real mermozd + real `mermoz` CLI against a stub Gmail. Covers
//! sign-in, import, routing, label write-back, reopen rule and the send gate.

mod stub_gmail;

use rstest::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use stub_gmail::Shared;

const NOW: i64 = 1_791_700_000_000;

fn raw(from: &str, subject: &str, body: &str, msg_id: &str) -> String {
    format!(
        "From: {from}\r\nTo: Me <me@example.com>\r\nSubject: {subject}\r\nMessage-ID: <{msg_id}@example.com>\r\nDate: Sun, 11 Oct 2026 08:12:00 +0000\r\n\r\n{body}\r\n"
    )
}

fn raw_with_pdf(from: &str, subject: &str) -> String {
    format!(
        "From: {from}\r\nTo: me@example.com\r\nSubject: {subject}\r\nMessage-ID: <pdf@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n--B\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"report.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQ=\r\n--B--\r\n"
    )
}

struct World {
    home: tempfile::TempDir,
    mailbox: Shared,
    daemon: Child,
    port: u16,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

fn bin_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mermozd"))
        .parent()
        .unwrap()
        .to_path_buf()
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

    fn cli(&self, args: &[&str]) -> std::process::Output {
        let bin = bin_dir().join("mermoz");
        assert!(bin.exists(), "build the CLI first: cargo build --release");
        Command::new(bin)
            .args(args)
            .env("MERMOZ_HOME", self.home.path())
            .output()
            .unwrap()
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

    fn thread_by_subject(&self, subject: &str) -> Value {
        let v = self.ok("threads.list", json!({ "limit": 100 }));
        v["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["subject"] == subject)
            .cloned()
            .unwrap_or_else(|| panic!("no thread {subject}"))
    }
}

/// Signs in through the real OAuth loopback flow, pretending to be the browser.
fn sign_in(w: &World) {
    let v = w.ok("accounts.add_gmail", json!({}));
    let url = url_parse(v["auth_url"].as_str().unwrap());
    let redirect = url
        .iter()
        .find(|(k, _)| k == "redirect_uri")
        .unwrap()
        .1
        .clone();
    let state = url.iter().find(|(k, _)| k == "state").unwrap().1.clone();
    let hostport = redirect.trim_start_matches("http://");
    let mut s = std::net::TcpStream::connect(hostport).unwrap();
    write!(
        s,
        "GET /?code=stub-code&state={} HTTP/1.1\r\nHost: {hostport}\r\n\r\n",
        urlencode(&state)
    )
    .unwrap();
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    w.wait_until("account to be added", |w| {
        w.ok("accounts.list", json!({}))
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
}

fn url_parse(u: &str) -> Vec<(String, String)> {
    let q = u.split_once('?').map(|x| x.1).unwrap_or("");
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), urldecode(v)))
        .collect()
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|c| {
            if c.is_ascii_alphanumeric() || b"-_.~".contains(&c) {
                (c as char).to_string()
            } else {
                format!("%{c:02X}")
            }
        })
        .collect()
}

#[fixture]
fn world() -> World {
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
                "Marc Petit <marc@example.com>",
                "Audit report v2",
                "Please sign off the remediation timeline",
                "m1",
            ),
            NOW,
        );
        m.add_message(
            "m2",
            "t2",
            &[],
            &raw(
                "Bank <noreply@bank.example>",
                "Your statement",
                "Statement ready",
                "m2",
            ),
            NOW - 1000,
        );
        m.add_message(
            "m3",
            "t3",
            &["INBOX"],
            &raw_with_pdf("Jonas <jonas@example.com>", "Budget sheet"),
            NOW - 2000,
        );
    }
    let port = stub_gmail::start(mailbox.clone());
    std::fs::write(
        home.path().join("oauth_client.json"),
        json!({ "client_id": "cid", "client_secret": "secret", "auth_url": format!("http://127.0.0.1:{port}/auth"), "token_url": format!("http://127.0.0.1:{port}/token") }).to_string(),
    )
    .unwrap();
    // Stand-in for Anthropic's `ant` CLI: login succeeds, credentials print a fixed token.
    let fake_ant = home.path().join("fake-ant");
    std::fs::write(
        &fake_ant,
        "#!/usr/bin/env python3\nimport sys\na = sys.argv[1:]\nif a[:2] == ['auth', 'print-credentials']:\n    print('oauth-tok')\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_ant, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let daemon = Command::new(env!("CARGO_BIN_EXE_mermozd"))
        .env("MERMOZ_ANT", &fake_ant)
        .env(
            "MERMOZ_CHATGPT_AUTH_BASE",
            format!("http://127.0.0.1:{port}/oa"),
        )
        .env(
            "MERMOZ_OPENAI_API_BASE",
            format!("http://127.0.0.1:{port}/oa/v1"),
        )
        .env("MERMOZ_HOME", home.path())
        .env("MERMOZ_TOKEN_STORE", "file")
        .env(
            "MERMOZ_GMAIL_BASE",
            format!("http://127.0.0.1:{port}/gmail"),
        )
        .env("MERMOZ_POLL_SECS", "1")
        .env("MERMOZ_JEV_BASE", format!("http://127.0.0.1:{port}/jev"))
        .env("MERMOZ_TRUSTED_EXE", std::env::current_exe().unwrap())
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
        mailbox,
        daemon,
        port,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(w.sock()).is_err() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    w
}

#[fixture]
fn synced(world: World) -> World {
    sign_in(&world);
    world.wait_until("initial import", |w| {
        w.ok("threads.list", json!({}))["total"] == 3
    });
    world
}

#[rstest]
fn sign_in_adds_the_account(world: World) {
    sign_in(&world);
    assert_eq!(
        world.ok("accounts.list", json!({}))[0]["email"],
        "me@example.com"
    );
}

#[rstest]
fn import_brings_every_thread(synced: World) {
    assert_eq!(synced.ok("threads.list", json!({}))["total"], 3);
}

#[rstest]
fn transient_fetch_failure_loses_no_mail(world: World) {
    world.mailbox.lock().unwrap().fail_once.insert("m2".into());
    sign_in(&world);
    world.wait_until("import with retry", |w| {
        w.ok("threads.list", json!({}))["total"] == 3
    });
}

#[rstest]
fn filed_mail_is_listed_under_its_label(world: World) {
    {
        let mut m = world.mailbox.lock().unwrap();
        let id = m.label_id_for("Clients/Acme");
        m.add_message(
            "f1",
            "tf1",
            &[&id],
            &raw("Acme <a@acme.example>", "Invoice 42", "Filed away", "f1"),
            NOW - 9000,
        );
    }
    sign_in(&world);
    world.wait_until("import", |w| w.ok("threads.list", json!({}))["total"] == 4);
    let out = world.cli(&["--json", "list", "--label", "clients/acme"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["threads"][0]["subject"], "Invoice 42");
}

/// Configures a provider pointed at the stub and turns the agent on for the account.
fn enable_agent(w: &World) {
    let pr = w.ok(
        "providers.save",
        json!({ "name": "Work AI", "kind": "anthropic", "model": "claude-opus-5-5",
        "base_url": format!("http://127.0.0.1:{}", w.port), "api_key": "sk-test" }),
    );
    let acc = w.ok("accounts.list", json!({}))[0]["id"].clone();
    w.ok(
        "accounts.agent.set",
        json!({ "account_id": acc, "provider_ids": [pr["id"]], "digest_enabled": true }),
    );
}

#[rstest]
fn agent_builds_directory_from_signatures(synced: World) {
    enable_agent(&synced);
    synced.wait_until("contact", |w| {
        w.ok("contacts.list", json!({}))
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
    assert_eq!(
        synced.ok("contacts.get", json!({ "email": "marc@example.com" }))["contact"]["title"],
        "CFO"
    );
}

#[rstest]
fn forced_clean_runs_without_auto_cleaning(synced: World) {
    let pr = synced.ok(
        "providers.save",
        json!({ "name": "Work AI", "kind": "anthropic", "model": "claude-opus-5-5",
        "base_url": format!("http://127.0.0.1:{}", synced.port), "api_key": "sk-test" }),
    );
    let acc = synced.ok("accounts.list", json!({}))[0]["id"].clone();
    synced.ok(
        "accounts.agent.set",
        json!({ "account_id": acc, "provider_ids": [pr["id"]] }),
    );
    let t = synced.thread_by_subject("Audit report v2");
    assert_eq!(
        synced.ok("agent.clean", json!({ "id": t["id"] }))["cleaned"],
        1
    );
}

/// "Audit report v2" with my reply sent, so the thread holds two messages to clean.
fn two_message_thread(w: &World) -> Value {
    let t = w.thread_by_subject("Audit report v2");
    let d = w.ok(
        "drafts.create",
        json!({ "thread_id": t["id"], "body": "ok" }),
    );
    w.ok("drafts.send", json!({ "id": d["id"] }));
    w.wait_until("reply in thread", |w| {
        w.ok("threads.get", json!({ "id": t["id"] }))["messages"]
            .as_array()
            .is_some_and(|m| m.len() == 2)
    });
    w.ok("threads.get", json!({ "id": t["id"] }))
}

#[rstest]
fn forced_clean_covers_every_message_of_the_thread(synced: World) {
    allow_ai(&synced);
    let d = two_message_thread(&synced);
    assert_eq!(
        synced.ok("agent.clean", json!({ "id": d["thread"]["id"] }))["cleaned"],
        2
    );
}

#[rstest]
fn clean_with_a_message_id_rewrites_only_that_message(synced: World) {
    allow_ai(&synced);
    let d = two_message_thread(&synced);
    assert_eq!(
        synced.ok(
            "agent.clean",
            json!({ "id": d["thread"]["id"], "message_id": d["messages"][0]["id"] })
        )["cleaned"],
        1
    );
}

#[rstest]
fn ai_people_clean_renames_the_directory(synced: World) {
    enable_agent(&synced);
    // Make Marc a correspondent so the directory has someone in it.
    let t = synced.thread_by_subject("Audit report v2");
    let d = synced.ok(
        "drafts.create",
        json!({ "thread_id": t["id"], "body": "ok" }),
    );
    synced.ok("drafts.send", json!({ "id": d["id"] }));
    synced.ok("agent.people_clean", json!({}));
    synced.wait_until("people clean", |w| {
        w.ok("agent.people_status", json!({}))["running"] == false
    });
    assert_eq!(
        synced.ok("contacts.list", json!({}))[0]["name"],
        "Tidy Name"
    );
}

#[rstest]
fn agent_clean_body_is_served(synced: World) {
    enable_agent(&synced);
    let t = synced.thread_by_subject("Audit report v2");
    synced.wait_until("clean body", |w| {
        w.ok("threads.get", json!({ "id": t["id"] }))["messages"][0]["body_clean"] == "Cleaned"
    });
}

#[rstest]
fn agent_stays_off_without_consent(synced: World) {
    let pr = synced.ok(
        "providers.save",
        json!({ "name": "Work AI", "kind": "anthropic", "model": "claude-opus-5-5",
        "base_url": format!("http://127.0.0.1:{}", synced.port), "api_key": "sk-test" }),
    );
    let acc = synced.ok("accounts.list", json!({}))[0]["id"].clone();
    synced.ok(
        "accounts.agent.set",
        json!({ "account_id": acc, "provider_ids": [pr["id"]] }),
    );
    std::thread::sleep(Duration::from_millis(1500));
    assert!(synced.mailbox.lock().unwrap().llm_keys.is_empty());
}

#[rstest]
fn agent_uses_the_stored_key(synced: World) {
    enable_agent(&synced);
    synced.wait_until("llm call", |w| {
        !w.mailbox.lock().unwrap().llm_keys.is_empty()
    });
    assert_eq!(synced.mailbox.lock().unwrap().llm_keys[0], "sk-test");
}

#[rstest]
fn anthropic_sign_in_adds_a_connected_provider(synced: World) {
    synced.ok("providers.login_anthropic", json!({}));
    synced.wait_until("provider", |w| {
        w.ok("providers.list", json!({}))
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
    assert_eq!(synced.ok("providers.list", json!({}))[0]["auth"], "oauth");
}

#[rstest]
fn signed_in_provider_calls_with_bearer_token(synced: World) {
    synced.ok("providers.login_anthropic", json!({}));
    synced.wait_until("provider", |w| {
        w.ok("providers.list", json!({}))
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
    let pr = synced.ok("providers.list", json!({}))[0]["id"].clone();
    // Point the signed-in provider at the stub instead of the real API.
    synced.ok(
        "providers.save",
        json!({ "id": pr, "name": "Claude", "kind": "anthropic", "model": "claude-opus-5-5",
        "base_url": format!("http://127.0.0.1:{}", synced.port) }),
    );
    let acc = synced.ok("accounts.list", json!({}))[0]["id"].clone();
    synced.ok(
        "accounts.agent.set",
        json!({ "account_id": acc, "provider_ids": ["*"], "digest_enabled": true }),
    );
    synced.wait_until("llm call", |w| {
        !w.mailbox.lock().unwrap().llm_keys.is_empty()
    });
    assert_eq!(
        synced.mailbox.lock().unwrap().llm_keys[0],
        "Bearer oauth-tok"
    );
}

#[rstest]
fn agents_cannot_start_a_sign_in(synced: World) {
    let script = r#"
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
s.sendall((json.dumps({"jsonrpc": "2.0", "id": 1, "method": "providers.login_anthropic", "params": {}}) + "\n").encode())
print(s.makefile().readline())
"#;
    let out = Command::new("python3")
        .args(["-c", script, synced.sock().to_str().unwrap()])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], -32011);
}

/// Plays the browser side of "Continue with ChatGPT": OpenAI redirects back with the issued client id.
fn chatgpt_sign_in(w: &World) -> String {
    let v = w.ok("providers.login_chatgpt", json!({}));
    let url = url_parse(v["auth_url"].as_str().unwrap());
    let get = |k: &str| {
        url.iter()
            .find(|(kk, _)| kk == k)
            .map(|x| x.1.clone())
            .unwrap_or_default()
    };
    w.mailbox.lock().unwrap().oidc_nonce = get("nonce");
    let redirect = get("redirect_uri");
    let hostport = redirect
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    let mut s = std::net::TcpStream::connect(&hostport).unwrap();
    write!(s, "GET /auth/callback?code=c1&state={}&client_id=oaiapp_test HTTP/1.1\r\nHost: {hostport}\r\n\r\n", urlencode(&get("state"))).unwrap();
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    w.wait_until("chatgpt provider", |w| {
        w.ok("providers.list", json!({}))
            .as_array()
            .is_some_and(|a| a.iter().any(|p| p["kind"] == "chatgpt"))
    });
    get("client_id")
}

#[rstest]
fn chatgpt_rejects_identity_with_wrong_nonce(synced: World) {
    let v = synced.ok("providers.login_chatgpt", json!({}));
    let url = url_parse(v["auth_url"].as_str().unwrap());
    let get = |k: &str| {
        url.iter()
            .find(|(kk, _)| kk == k)
            .map(|x| x.1.clone())
            .unwrap_or_default()
    };
    synced.mailbox.lock().unwrap().oidc_nonce = "forged".into();
    let redirect = get("redirect_uri");
    let hostport = redirect
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    let mut s = std::net::TcpStream::connect(&hostport).unwrap();
    write!(s, "GET /auth/callback?code=c1&state={}&client_id=oaiapp_test HTTP/1.1\r\nHost: {hostport}\r\n\r\n", urlencode(&get("state"))).unwrap();
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    synced.wait_until("login to finish", |w| {
        w.ok("providers.login_status", json!({}))["running"] == false
    });
    assert!(
        synced
            .ok("providers.list", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[rstest]
fn chatgpt_retry_replaces_a_stuck_attempt(synced: World) {
    // First attempt is abandoned in the browser; the second one completes.
    synced.ok("providers.login_chatgpt", json!({}));
    chatgpt_sign_in(&synced);
    assert_eq!(
        synced.ok("providers.login_status", json!({}))["running"],
        false
    );
}

#[rstest]
fn chatgpt_sign_in_registers_dynamically(synced: World) {
    assert_eq!(chatgpt_sign_in(&synced), "dynamic_agent_client");
}

#[rstest]
fn chatgpt_provider_uses_first_listed_model(synced: World) {
    chatgpt_sign_in(&synced);
    let list = synced.ok("providers.list", json!({}));
    assert_eq!(
        list.as_array()
            .unwrap()
            .iter()
            .find(|p| p["kind"] == "chatgpt")
            .unwrap()["model"],
        "gpt-test-mini"
    );
}

#[rstest]
fn chatgpt_cleans_mail_with_plan_token(synced: World) {
    chatgpt_sign_in(&synced);
    let acc = synced.ok("accounts.list", json!({}))[0]["id"].clone();
    synced.ok(
        "accounts.agent.set",
        json!({ "account_id": acc, "provider_ids": ["*"], "digest_enabled": true }),
    );
    let t = synced.thread_by_subject("Audit report v2");
    synced.wait_until("clean body", |w| {
        w.ok("threads.get", json!({ "id": t["id"] }))["messages"][0]["body_clean"]
            == "Cleaned by ChatGPT"
    });
}

#[rstest]
fn chatgpt_calls_send_the_access_token(synced: World) {
    chatgpt_sign_in(&synced);
    let acc = synced.ok("accounts.list", json!({}))[0]["id"].clone();
    synced.ok(
        "accounts.agent.set",
        json!({ "account_id": acc, "provider_ids": ["*"], "digest_enabled": true }),
    );
    synced.wait_until("responses call", |w| {
        !w.mailbox.lock().unwrap().responses_auth.is_empty()
    });
    assert_eq!(
        synced.mailbox.lock().unwrap().responses_auth[0],
        "Bearer chatgpt-access"
    );
}

#[rstest]
fn deleting_a_project_removes_its_gmail_label(synced: World) {
    synced.ok("projects.create", json!({ "name": "test" }));
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "add_projects": ["test"] }),
    );
    synced.wait_until("label added", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("test")
    });
    synced.ok("projects.delete", json!({ "id": "test" }));
    synced.wait_until("label deleted", |w| {
        !w.mailbox
            .lock()
            .unwrap()
            .labels
            .values()
            .any(|n| n == "test")
    });
}

#[rstest]
fn adding_a_project_files_the_thread_in_gmail(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "add_projects": ["Clients/Acme"] }),
    );
    synced.wait_until("label added", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("Clients/Acme")
    });
}

#[rstest]
fn deleted_moves_the_thread_to_gmail_trash(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "status": "deleted" }),
    );
    synced.wait_until("trash", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("TRASH")
    });
}

#[rstest]
fn restoring_a_deleted_thread_untrashes_it(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "status": "deleted" }),
    );
    synced.wait_until("trash", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("TRASH")
    });
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "status": "todo" }),
    );
    synced.wait_until("untrash", |w| {
        !w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("TRASH")
    });
}

#[rstest]
fn unread_inbox_mail_lands_in_unread(synced: World) {
    assert_eq!(
        synced.thread_by_subject("Audit report v2")["status"],
        "unread"
    );
}

#[rstest]
fn archived_mail_is_archive(synced: World) {
    assert_eq!(
        synced.thread_by_subject("Your statement")["status"],
        "archive"
    );
}

#[rstest]
fn search_finds_body_text(synced: World) {
    assert_eq!(
        synced.ok("search", json!({ "query": "remediation" }))["threads"][0]["subject"],
        "Audit report v2"
    );
}

#[rstest]
fn attachment_is_a_readable_file(synced: World) {
    let t = synced.thread_by_subject("Budget sheet");
    let full = synced.ok("threads.get", json!({ "id": t["id"] }));
    let att = &full["messages"][0]["attachments"][0];
    let info = synced.ok("attachments.get", json!({ "id": att["id"] }));
    assert_eq!(
        std::fs::read(info["path"].as_str().unwrap()).unwrap(),
        b"%PDF-1.4"
    );
}

#[rstest]
fn status_change_pushes_label_to_gmail(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "status": "todo" }),
    );
    synced.wait_until("label push", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("Mermoz/Todo")
    });
}

#[rstest]
fn done_archives_in_gmail(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "status": "done" }),
    );
    synced.wait_until("archive", |w| {
        !w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("INBOX")
    });
}

#[rstest]
fn own_label_echo_keeps_local_status(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "status": "waiting" }),
    );
    synced.wait_until("label push", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("Mermoz/Waiting")
    });
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        synced.thread_by_subject("Audit report v2")["status"],
        "waiting"
    );
}

#[rstest]
fn reply_reopens_done_thread(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "status": "done" }),
    );
    synced.wait_until("archive", |w| {
        !w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("INBOX")
    });
    synced.mailbox.lock().unwrap().add_message(
        "m9",
        "t1",
        &["INBOX", "UNREAD"],
        &raw(
            "Marc Petit <marc@example.com>",
            "Re: Audit report v2",
            "Any news?",
            "m9",
        ),
        NOW + 5000,
    );
    synced.wait_until("reopen", |w| {
        w.thread_by_subject("Audit report v2")["status"] == "unread"
    });
}

#[rstest]
fn remote_status_label_is_adopted(synced: World) {
    let id = synced
        .mailbox
        .lock()
        .unwrap()
        .label_id_for("Mermoz/Waiting");
    synced
        .mailbox
        .lock()
        .unwrap()
        .relabel_thread("t3", &[&id], &[]);
    synced.wait_until("adopt", |w| {
        w.thread_by_subject("Budget sheet")["status"] == "waiting"
    });
}

#[rstest]
fn agent_send_exits_with_approval_code(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    let d = synced.cli(&[
        "--json",
        "draft",
        "reply",
        t["number"].as_str().unwrap(),
        "--body",
        "Signed off.",
    ]);
    let draft: Value = serde_json::from_slice(&d.stdout).unwrap();
    let out = synced.cli(&["send", draft["id"].as_str().unwrap()]);
    assert_eq!(out.status.code(), Some(3));
}

#[rstest]
fn agent_send_moves_thread_to_review(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    let d = synced.cli(&[
        "--json",
        "draft",
        "reply",
        t["number"].as_str().unwrap(),
        "--body",
        "Signed off.",
    ]);
    let draft: Value = serde_json::from_slice(&d.stdout).unwrap();
    synced.cli(&["send", draft["id"].as_str().unwrap()]);
    assert_eq!(
        synced.thread_by_subject("Audit report v2")["status"],
        "review"
    );
}

#[rstest]
fn agent_send_does_not_reach_gmail(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    let d = synced.cli(&[
        "--json",
        "draft",
        "reply",
        t["number"].as_str().unwrap(),
        "--body",
        "Signed off.",
    ]);
    let draft: Value = serde_json::from_slice(&d.stdout).unwrap();
    synced.cli(&["send", draft["id"].as_str().unwrap()]);
    assert!(synced.mailbox.lock().unwrap().sent.is_empty());
}

#[rstest]
fn agents_cannot_flip_the_send_gate(synced: World) {
    // Any process other than Mermoz.app (here: python) is refused app-only methods.
    let script = r#"
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
s.sendall((json.dumps({"jsonrpc": "2.0", "id": 1, "method": "settings.set", "params": {"agents_may_send": True}}) + "\n").encode())
print(s.makefile().readline())
"#;
    let out = Command::new("python3")
        .args(["-c", script, synced.sock().to_str().unwrap()])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], -32011);
}

#[rstest]
fn app_approval_sends_threaded_reply(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    let d = synced.cli(&[
        "--json",
        "draft",
        "reply",
        t["number"].as_str().unwrap(),
        "--body",
        "Signed off.",
    ]);
    let draft: Value = serde_json::from_slice(&d.stdout).unwrap();
    synced.cli(&["send", draft["id"].as_str().unwrap()]);
    synced.ok("review.approve", json!({ "id": draft["id"] }));
    assert!(
        synced.mailbox.lock().unwrap().sent[0]
            .0
            .contains("In-Reply-To: <m1@example.com>")
    );
}

#[rstest]
fn approved_thread_waits_on_the_other_side(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    let d = synced.cli(&[
        "--json",
        "draft",
        "reply",
        t["number"].as_str().unwrap(),
        "--body",
        "Signed off.",
    ]);
    let draft: Value = serde_json::from_slice(&d.stdout).unwrap();
    synced.cli(&["send", draft["id"].as_str().unwrap()]);
    synced.ok("review.approve", json!({ "id": draft["id"] }));
    assert_eq!(
        synced.thread_by_subject("Audit report v2")["status"],
        "waiting"
    );
}

#[rstest]
fn rejection_restores_status(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    let d = synced.cli(&[
        "--json",
        "draft",
        "reply",
        t["number"].as_str().unwrap(),
        "--body",
        "Signed off.",
    ]);
    let draft: Value = serde_json::from_slice(&d.stdout).unwrap();
    synced.cli(&["send", draft["id"].as_str().unwrap()]);
    synced.ok("review.reject", json!({ "id": draft["id"] }));
    assert_eq!(
        synced.thread_by_subject("Audit report v2")["status"],
        "unread"
    );
}

#[rstest]
fn mark_read_pushes_to_gmail(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok("threads.mark_read", json!({ "id": t["id"] }));
    synced.wait_until("read push", |w| {
        !w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("UNREAD")
    });
}

#[rstest]
fn cli_search_json_lists_thread(synced: World) {
    let out = synced.cli(&["--json", "search", "remediation"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["threads"][0]["subject"], "Audit report v2");
}

#[rstest]
fn cli_unknown_status_lists_valid_ones(synced: World) {
    let out = synced.cli(&["status", "M-1", "nope"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("valid: unread"));
}

#[rstest]
fn mcp_lists_tools(synced: World) {
    let mut child = Command::new(bin_dir().join("mermoz"))
        .arg("mcp")
        .env("MERMOZ_HOME", synced.home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } })).unwrap();
    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })
    )
    .unwrap();
    drop(stdin);
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let _ = child.wait();
    let last: Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
    assert!(
        last["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "send_draft")
    );
}

#[rstest]
fn mcp_send_reports_approval(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    let d = synced.cli(&[
        "--json",
        "draft",
        "reply",
        t["number"].as_str().unwrap(),
        "--body",
        "ok",
    ]);
    let draft: Value = serde_json::from_slice(&d.stdout).unwrap();
    let mut child = Command::new(bin_dir().join("mermoz"))
        .arg("mcp")
        .env("MERMOZ_HOME", synced.home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "send_draft", "arguments": { "id": draft["id"] } } })).unwrap();
    drop(stdin);
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let _ = child.wait();
    let v: Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
    assert!(
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("approval_required")
    );
}

/// Turns the router on with a "Receipts" project available to file into.
fn enable_router(w: &World) {
    w.ok("projects.create", json!({ "name": "Receipts" }));
    w.ok("router.set", json!({ "api_key": "jev-test" }));
    w.wait_until("routing", |w| {
        !w.thread_by_subject("Audit report v2")["suggestion"].is_null()
    });
}

#[rstest]
fn router_suggests_a_status_for_unread_mail(synced: World) {
    enable_router(&synced);
    assert_eq!(
        synced.thread_by_subject("Audit report v2")["suggestion"]["status"],
        "archive"
    );
}

#[rstest]
fn router_suggests_a_project(synced: World) {
    enable_router(&synced);
    assert_eq!(
        synced.thread_by_subject("Audit report v2")["suggestion"]["projects"],
        json!(["Receipts"])
    );
}

#[rstest]
fn router_sends_the_typesafe_key(synced: World) {
    enable_router(&synced);
    assert!(
        synced
            .mailbox
            .lock()
            .unwrap()
            .jev_auth
            .contains(&"Bearer jev-test".to_string())
    );
}

#[rstest]
fn reading_applies_the_router_suggestion(synced: World) {
    enable_router(&synced);
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok("threads.mark_read", json!({ "id": t["id"] }));
    assert_eq!(
        synced.thread_by_subject("Audit report v2")["status"],
        "archive"
    );
}

#[rstest]
fn accepting_files_the_thread_in_gmail(synced: World) {
    enable_router(&synced);
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok("threads.accept_suggestions", json!({ "ids": [t["id"]] }));
    synced.wait_until("label push", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("Receipts")
    });
}

#[rstest]
fn router_reports_its_key(synced: World) {
    synced.ok("router.set", json!({ "api_key": "jev-test" }));
    assert_eq!(synced.ok("router.get", json!({}))["configured"], true);
}

#[rstest]
fn router_test_checks_the_key(synced: World) {
    assert_eq!(
        synced.ok("router.test", json!({ "api_key": "jev-test" }))["ok"],
        true
    );
}

#[rstest]
fn agents_cannot_set_the_typesafe_key(synced: World) {
    let script = r#"
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
s.sendall((json.dumps({"jsonrpc": "2.0", "id": 1, "method": "router.set", "params": {"api_key": "x"}}) + "\n").encode())
print(s.makefile().readline())
"#;
    let out = Command::new("python3")
        .args(["-c", script, synced.sock().to_str().unwrap()])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], -32011);
}

/// An HTML mail whose quote lookahead ends inside "à" (as in a real 2012 thread).
fn raw_html_accent_at_edge() -> String {
    let marker = r#"<div class="gmail_quote">"#;
    let tail = format!("{}à la suite", "x".repeat(799 - marker.len()));
    format!(
        "From: Romain <romain@example.com>\r\nTo: me@example.com\r\nSubject: Accent edge\r\nMessage-ID: <acc@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Bonjour</p>{marker}{tail}</div>\r\n"
    )
}

#[rstest]
fn html_with_accent_at_the_quote_edge_opens(synced: World) {
    synced.mailbox.lock().unwrap().add_message(
        "m7",
        "t7",
        &["INBOX"],
        &raw_html_accent_at_edge(),
        NOW + 7000,
    );
    synced.wait_until("sync", |w| {
        w.ok("search", json!({ "query": "Accent edge" }))["threads"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
    let t = synced.thread_by_subject("Accent edge");
    let full = synced.ok("threads.get", json!({ "id": t["id"] }));
    let html = synced.ok("messages.html", json!({ "id": full["messages"][0]["id"] }));
    assert!(html["html"].as_str().unwrap().contains("Bonjour"));
}

#[rstest]
fn statuses_can_be_reordered(synced: World) {
    let mut keys: Vec<Value> = synced
        .ok("statuses.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].clone())
        .collect();
    keys.swap(1, 4);
    synced.ok("statuses.reorder", json!({ "keys": keys }));
    let after: Vec<Value> = synced
        .ok("statuses.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].clone())
        .collect();
    assert_eq!(after, keys);
}

#[rstest]
fn reversed_order_is_the_list_backwards(synced: World) {
    let ids = |reverse: bool| -> Vec<Value> {
        synced.ok("threads.list", json!({ "reverse": reverse }))["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].clone())
            .collect()
    };
    let mut backwards = ids(false);
    backwards.reverse();
    assert_eq!(ids(true), backwards);
}

/// An AI allowed for the account, without automatic cleaning.
fn allow_ai(w: &World) {
    let pr = w.ok(
        "providers.save",
        json!({ "name": "Work AI", "kind": "anthropic", "model": "claude-opus-5-5",
        "base_url": format!("http://127.0.0.1:{}", w.port), "api_key": "sk-test" }),
    );
    let acc = w.ok("accounts.list", json!({}))[0]["id"].clone();
    w.ok(
        "accounts.agent.set",
        json!({ "account_id": acc, "provider_ids": [pr["id"]] }),
    );
}

#[rstest]
fn optimized_view_cleans_a_message_once(synced: World) {
    allow_ai(&synced);
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "agent.clean",
        json!({ "id": t["id"], "missing_only": true }),
    );
    assert_eq!(
        synced.ok(
            "agent.clean",
            json!({ "id": t["id"], "missing_only": true })
        )["cleaned"],
        0
    );
}

#[rstest]
fn optimized_view_does_not_call_the_ai_again(synced: World) {
    allow_ai(&synced);
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "agent.clean",
        json!({ "id": t["id"], "missing_only": true }),
    );
    let calls = synced.mailbox.lock().unwrap().llm_keys.len();
    synced.ok(
        "agent.clean",
        json!({ "id": t["id"], "missing_only": true }),
    );
    assert_eq!(synced.mailbox.lock().unwrap().llm_keys.len(), calls);
}

#[rstest]
fn a_slow_ai_call_does_not_hold_back_other_requests(synced: World) {
    allow_ai(&synced);
    synced.mailbox.lock().unwrap().llm_delay_ms = 1500;
    let t = synced.thread_by_subject("Audit report v2");
    // Same connection, like the app: the quick list must answer before the slow clean.
    let script = r#"
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f = s.makefile()
s.sendall((json.dumps({"jsonrpc": "2.0", "id": 1, "method": "agent.clean", "params": {"id": sys.argv[2]}}) + "\n").encode())
s.sendall((json.dumps({"jsonrpc": "2.0", "id": 2, "method": "threads.list", "params": {}}) + "\n").encode())
print(json.loads(f.readline())["id"])
"#;
    let out = Command::new("python3")
        .args([
            "-c",
            script,
            synced.sock().to_str().unwrap(),
            t["id"].as_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "2");
}

const GMAIL_PASS: &str = "Authentication-Results: mx.google.com;\r\n       dkim=pass header.i=@amazonaws.com;\r\n       dmarc=pass (p=QUARANTINE) header.from=amazonaws.com";

/// A message from AWS carrying Gmail's authentication verdict.
fn raw_signed(id: &str) -> String {
    format!(
        "{GMAIL_PASS}\r\nFrom: Amazon Web Services <no-reply@amazonaws.com>\r\nTo: me@example.com\r\nSubject: Billing alert\r\nMessage-ID: <{id}@example.com>\r\n\r\nYour bill is ready.\r\n"
    )
}

fn first_message_auth(w: &World, subject: &str) -> Value {
    let t = w.thread_by_subject(subject);
    w.ok("threads.get", json!({ "id": t["id"] }))["messages"][0]["auth"].clone()
}

#[rstest]
fn new_mail_from_a_verified_sender_is_marked_pass(synced: World) {
    synced.mailbox.lock().unwrap().add_message(
        "m8",
        "t8",
        &["INBOX", "UNREAD"],
        &raw_signed("m8"),
        NOW + 8000,
    );
    synced.wait_until("sync", |w| {
        w.ok("search", json!({ "query": "Billing alert" }))["threads"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
    assert_eq!(first_message_auth(&synced, "Billing alert"), "pass");
}

#[rstest]
fn older_mail_gets_its_sender_checked_when_opened(synced: World) {
    synced.mailbox.lock().unwrap().add_message(
        "m8",
        "t8",
        &["INBOX", "UNREAD"],
        &raw_signed("m8"),
        NOW + 8000,
    );
    synced.wait_until("sync", |w| {
        w.ok("search", json!({ "query": "Billing alert" }))["threads"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
    // As if imported before verification existed.
    let db = synced.home.path().join("mermoz.sqlite");
    let out = Command::new("sqlite3")
        .args([
            db.to_str().unwrap(),
            "UPDATE messages SET auth = NULL WHERE subject = 'Billing alert'",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    first_message_auth(&synced, "Billing alert");
    synced.wait_until("sender check", |w| {
        first_message_auth(w, "Billing alert") == "pass"
    });
}

/// Ann's mail as Gmail received it, given Gmail's verdict line.
fn raw_from_ann(id: &str, results: &str) -> String {
    format!(
        "Authentication-Results: mx.google.com;\r\n       {results}\r\nFrom: Ann Example <ann@mail.example>\r\nTo: me@example.com\r\nSubject: Sunday lunch\r\nMessage-ID: <{id}@mail.example>\r\n\r\nSee you there.\r\n"
    )
}

fn ann_auth(w: &World, results: &str) -> Value {
    w.mailbox.lock().unwrap().add_message(
        "m8a",
        "t8a",
        &["INBOX", "UNREAD"],
        &raw_from_ann("m8a", results),
        NOW + 8000,
    );
    w.wait_until("sync", |w| {
        w.ok("search", json!({ "query": "Sunday lunch" }))["threads"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
    first_message_auth(w, "Sunday lunch")
}

#[rstest]
fn a_forwarded_copy_gets_no_warning(synced: World) {
    let forwarded = "spf=softfail smtp.mailfrom=ann@mail.example;\r\n       dkim=neutral (body hash did not verify) header.i=@;\r\n       dmarc=fail (p=NONE dis=NONE) header.from=mail.example";
    assert_eq!(ann_auth(&synced, forwarded), "none");
}

#[rstest]
fn a_failure_the_domain_rejects_is_a_warning(synced: World) {
    let spoofed = "spf=fail smtp.mailfrom=x@evil.example;\r\n       dmarc=fail (p=REJECT dis=QUARANTINE) header.from=mail.example";
    assert_eq!(ann_auth(&synced, spoofed), "fail");
}

/// A newsletter whose logo is a remote image.
fn raw_with_remote_image() -> String {
    "From: News <news@example.com>\r\nTo: me@example.com\r\nSubject: Weekly news\r\nMessage-ID: <img@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hello</p><img src=\"https://cdn.example/logo.png\">\r\n".into()
}

fn news_html(w: &World, images: bool) -> Value {
    w.mailbox.lock().unwrap().add_message(
        "m9i",
        "t9i",
        &["INBOX"],
        &raw_with_remote_image(),
        NOW + 9000,
    );
    w.wait_until("sync", |w| {
        w.ok("search", json!({ "query": "Weekly news" }))["threads"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
    let t = w.thread_by_subject("Weekly news");
    let m = w.ok("threads.get", json!({ "id": t["id"] }))["messages"][0]["id"].clone();
    w.ok("messages.html", json!({ "id": m, "images": images }))
}

#[rstest]
fn remote_images_are_hidden_by_default(synced: World) {
    assert!(
        !news_html(&synced, false)["html"]
            .as_str()
            .unwrap()
            .contains("logo.png")
    );
}

#[rstest]
fn hidden_remote_images_are_reported(synced: World) {
    assert_eq!(news_html(&synced, false)["remote_images"], true);
}

#[rstest]
fn remote_images_load_when_asked(synced: World) {
    assert!(
        news_html(&synced, true)["html"]
            .as_str()
            .unwrap()
            .contains("logo.png")
    );
}

// ---- background sorting of open columns ---------------------------------

/// Turns the router on and waits until the read "Budget sheet" (To do) has its
/// background suggestion: the stub files everything in Receipts, to archive.
fn sorted_todo(w: &World) {
    enable_router(w);
    w.wait_until("background sorting", |w| {
        !w.thread_by_subject("Budget sheet")["suggestion"].is_null()
    });
}

fn todo_column(w: &World) -> Value {
    w.ok("suggestions.summary", json!({ "statuses": ["todo"] }))["columns"][0].clone()
}

#[rstest]
fn background_sorting_suggests_for_an_open_column(synced: World) {
    sorted_todo(&synced);
    assert_eq!(
        synced.thread_by_subject("Budget sheet")["suggestion"]["status"],
        "archive"
    );
}

#[rstest]
fn background_sorting_leaves_the_thread_in_place(synced: World) {
    sorted_todo(&synced);
    assert_eq!(synced.thread_by_subject("Budget sheet")["status"], "todo");
}

#[rstest]
fn column_summary_groups_by_target(synced: World) {
    sorted_todo(&synced);
    assert_eq!(
        todo_column(&synced)["groups"],
        json!([{ "status": "archive", "projects": ["Receipts"], "count": 1 }])
    );
}

#[rstest]
fn column_summary_reports_sorting_progress(synced: World) {
    sorted_todo(&synced);
    let col = todo_column(&synced);
    assert_eq!(
        (col["classified"].clone(), col["total"].clone()),
        (json!(1), json!(1))
    );
}

#[rstest]
fn background_calls_are_counted(synced: World) {
    sorted_todo(&synced);
    assert!(
        synced.ok("suggestions.summary", json!({ "statuses": ["todo"] }))["calls_today"]
            .as_i64()
            .unwrap()
            >= 1
    );
}

#[rstest]
fn accepting_a_column_moves_its_threads(synced: World) {
    sorted_todo(&synced);
    synced.ok("suggestions.accept", json!({ "status": "todo" }));
    assert_eq!(
        synced.thread_by_subject("Budget sheet")["status"],
        "archive"
    );
}

#[rstest]
fn accepting_another_group_moves_nothing(synced: World) {
    sorted_todo(&synced);
    let v = synced.ok(
        "suggestions.accept",
        json!({ "status": "todo", "target": { "status": "done", "projects": [] } }),
    );
    assert_eq!(v["accepted"], 0);
}

#[rstest]
fn accepting_a_column_files_in_gmail(synced: World) {
    sorted_todo(&synced);
    synced.ok("suggestions.accept", json!({ "status": "todo" }));
    synced.wait_until("label push", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t3")
            .contains("Receipts")
    });
}

#[rstest]
fn restoring_undoes_a_column_accept(synced: World) {
    sorted_todo(&synced);
    let v = synced.ok("suggestions.accept", json!({ "status": "todo" }));
    synced.ok("threads.restore", json!({ "threads": v["before"] }));
    assert_eq!(synced.thread_by_subject("Budget sheet")["status"], "todo");
}

/// A read thread nobody wrote to for ten years, sorted by a Jev that answers Waiting.
fn sorted_stale_thread(w: &World) -> Value {
    {
        let mut m = w.mailbox.lock().unwrap();
        m.jev_status = Some("waiting".into());
        m.add_message(
            "m10",
            "t10",
            &["INBOX"],
            &raw(
                "Old Friend <old@example.com>",
                "Offsite plans",
                "Shall we?",
                "m10",
            ),
            NOW - 3650 * 86_400_000,
        );
    }
    w.wait_until("sync", |w| {
        w.ok("search", json!({ "query": "Offsite plans" }))["threads"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    });
    enable_router(w);
    w.wait_until("background sorting", |w| {
        !w.thread_by_subject("Offsite plans")["suggestion"].is_null()
    });
    w.thread_by_subject("Offsite plans")["suggestion"].clone()
}

#[rstest]
fn stale_thread_gets_no_waiting_suggestion(synced: World) {
    assert!(sorted_stale_thread(&synced)["status"].is_null());
}

#[rstest]
fn stale_thread_still_gets_its_project(synced: World) {
    assert_eq!(
        sorted_stale_thread(&synced)["projects"],
        json!(["Receipts"])
    );
}

#[rstest]
fn background_sorting_stops_at_the_daily_cap(synced: World) {
    synced.ok("router.set", json!({ "daily_cap": 0 }));
    enable_router(&synced);
    synced.wait_until("cap reached", |w| {
        w.ok("suggestions.summary", json!({ "statuses": ["todo"] }))["paused"] == "limit"
    });
    assert!(synced.thread_by_subject("Budget sheet")["suggestion"].is_null());
}

#[rstest]
fn background_sorting_recovers_from_rate_limits(synced: World) {
    synced.mailbox.lock().unwrap().jev_rate_limited = 3;
    sorted_todo(&synced);
}

#[rstest]
fn cli_accepts_a_column(synced: World) {
    sorted_todo(&synced);
    synced.cli(&["accept", "--column", "todo"]);
    assert_eq!(
        synced.thread_by_subject("Budget sheet")["status"],
        "archive"
    );
}

#[rstest]
fn cli_lists_column_suggestions(synced: World) {
    sorted_todo(&synced);
    let out = synced.cli(&["suggestions", "todo"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("→ archive +Receipts"));
}

// ---- sorting the whole mailbox again ---------------------------------------

fn archive_column(w: &World) -> Value {
    w.ok("suggestions.summary", json!({ "statuses": ["archive"] }))["columns"][0].clone()
}

/// Open columns sorted, then `mermoz resort --all`, then the archived "Your statement"
/// sorted too.
fn resorted_all(w: &World) {
    sorted_todo(w);
    let out = w.cli(&["resort", "--all"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    w.wait_until("whole-mailbox sort", |w| {
        !w.thread_by_subject("Your statement")["suggestion"].is_null()
    });
}

/// `mermoz resort --all` while TypeSafe keeps answering "slow down".
fn resort_stalled(w: &World) {
    sorted_todo(w);
    w.mailbox.lock().unwrap().jev_rate_limited = 1_000_000;
    w.cli(&["resort", "--all"]);
}

#[rstest]
fn resort_sorts_archived_mail(synced: World) {
    resorted_all(&synced);
    assert_eq!(
        synced.thread_by_subject("Your statement")["suggestion"]["projects"],
        json!(["Receipts"])
    );
}

#[rstest]
fn archived_mail_is_not_sorted_without_resort(synced: World) {
    sorted_todo(&synced);
    assert!(synced.thread_by_subject("Your statement")["suggestion"].is_null());
}

#[rstest]
fn resort_ends_once_everything_is_sorted(synced: World) {
    resorted_all(&synced);
    synced.wait_until("resort end", |w| {
        w.ok("router.get", json!({}))["resort"].is_null()
    });
}

#[rstest]
fn archive_bar_shows_sorting_during_resort(synced: World) {
    resort_stalled(&synced);
    assert_eq!(archive_column(&synced)["sorted"], true);
}

#[rstest]
fn archive_bar_progress_starts_from_zero(synced: World) {
    resort_stalled(&synced);
    let col = archive_column(&synced);
    assert_eq!(
        (col["classified"].clone(), col["total"].clone()),
        (json!(0), json!(1))
    );
}

#[rstest]
fn resort_puts_open_columns_back_in_the_queue(synced: World) {
    resort_stalled(&synced);
    assert_eq!(todo_column(&synced)["classified"], 0);
}

#[rstest]
fn cli_resort_reports_the_columns(synced: World) {
    sorted_todo(&synced);
    let out = synced.cli(&["resort", "--column", "archive"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("(archive)"));
}

#[rstest]
fn resort_needs_a_typesafe_key(synced: World) {
    assert!(synced.call("suggestions.resort", json!({})).is_err());
}

#[rstest]
fn mcp_resorts_the_mailbox(synced: World) {
    sorted_todo(&synced);
    let mut child = Command::new(bin_dir().join("mermoz"))
        .arg("mcp")
        .env("MERMOZ_HOME", synced.home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "resort_mailbox", "arguments": { "statuses": ["done"] } } })).unwrap();
    drop(stdin);
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let _ = child.wait();
    let v: Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
    assert!(
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"done\"")
    );
}

// ---- chat about a thread ---------------------------------------------------

/// A thread with an AI allowed for its account, ready to chat about.
#[fixture]
fn chatting(synced: World) -> (World, String) {
    allow_ai(&synced);
    let id = synced.thread_by_subject("Audit report v2")["id"]
        .as_str()
        .unwrap()
        .to_string();
    (synced, id)
}

fn ask(w: &World, thread: &str, text: &str) -> Value {
    w.ok("chat.send", json!({ "thread_id": thread, "text": text }))
}

fn chat_system(w: &World) -> String {
    let m = w.mailbox.lock().unwrap();
    m.chat_requests[0]["system"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[rstest]
fn chat_answer_hides_the_actions_block(chatting: (World, String)) {
    let (w, t) = chatting;
    assert_eq!(
        ask(&w, &t, "What should I do?")["text"],
        "Marc wants your sign-off. I'd reply today."
    );
}

#[rstest]
fn chat_keeps_only_applicable_proposals(chatting: (World, String)) {
    let (w, t) = chatting;
    let kinds: Vec<Value> = ask(&w, &t, "What should I do?")["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].clone())
        .collect();
    assert_eq!(kinds, vec![json!("status"), json!("draft_reply")]);
}

#[rstest]
fn chat_never_creates_a_draft_by_itself(chatting: (World, String)) {
    let (w, t) = chatting;
    ask(&w, &t, "Write him back");
    assert_eq!(w.ok("drafts.list", json!({})), json!([]));
}

#[rstest]
fn chat_is_kept_with_the_thread(chatting: (World, String)) {
    let (w, t) = chatting;
    ask(&w, &t, "What should I do?");
    assert_eq!(
        w.ok("chat.get", json!({ "thread_id": t }))["messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[rstest]
fn chat_accepts_the_thread_number(chatting: (World, String)) {
    let (w, t) = chatting;
    ask(&w, &t, "What should I do?");
    let number = w.ok("threads.get", json!({ "id": t }))["thread"]["number"].clone();
    assert_eq!(
        w.ok("chat.get", json!({ "thread_id": number }))["messages"][0]["text"],
        "What should I do?"
    );
}

#[rstest]
fn follow_up_question_carries_the_conversation(chatting: (World, String)) {
    let (w, t) = chatting;
    ask(&w, &t, "What should I do?");
    ask(&w, &t, "And if I say no?");
    let roles: Vec<Value> = w.mailbox.lock().unwrap().chat_requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].clone())
        .collect();
    assert_eq!(
        roles,
        vec![json!("user"), json!("assistant"), json!("user")]
    );
}

#[rstest]
fn chat_context_carries_the_thread_body(chatting: (World, String)) {
    let (w, t) = chatting;
    ask(&w, &t, "What should I do?");
    assert!(chat_system(&w).contains("Please sign off the remediation timeline"));
}

#[rstest]
fn chat_context_states_today(chatting: (World, String)) {
    let (w, t) = chatting;
    ask(&w, &t, "What should I do?");
    assert!(chat_system(&w).contains("Today: 20"));
}

#[rstest]
fn chat_context_names_the_participants(chatting: (World, String)) {
    let (w, t) = chatting;
    ask(&w, &t, "What should I do?");
    assert!(chat_system(&w).contains("Participants: Marc Petit <marc@example.com>"));
}

#[rstest]
fn chat_lets_the_ai_search_the_web(chatting: (World, String)) {
    let (w, t) = chatting;
    ask(&w, &t, "Who is Marc?");
    assert_eq!(
        w.mailbox.lock().unwrap().chat_requests[0]["tools"][0]["type"],
        "web_search_20260209"
    );
}

#[rstest]
fn chat_answer_says_when_the_ai_searched(chatting: (World, String)) {
    let (w, t) = chatting;
    assert_eq!(ask(&w, &t, "Who is Marc?")["searched"], true);
}

#[rstest]
fn chat_without_research_sends_no_tools(chatting: (World, String)) {
    let (w, t) = chatting;
    w.ok(
        "chat.send",
        json!({ "thread_id": t, "text": "Quick one", "research": false }),
    );
    assert!(
        w.mailbox.lock().unwrap().chat_requests[0]
            .get("tools")
            .is_none()
    );
}

#[rstest]
fn chat_reports_research_is_available(chatting: (World, String)) {
    let (w, t) = chatting;
    assert_eq!(
        w.ok("chat.get", json!({ "thread_id": t }))["research_available"],
        true
    );
}

#[rstest]
fn chat_clear_forgets_the_conversation(chatting: (World, String)) {
    let (w, t) = chatting;
    ask(&w, &t, "What should I do?");
    w.ok("chat.clear", json!({ "thread_id": t }));
    assert_eq!(
        w.ok("chat.get", json!({ "thread_id": t }))["messages"],
        json!([])
    );
}

#[rstest]
fn chat_needs_an_allowed_ai(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    let e = synced
        .call(
            "chat.send",
            json!({ "thread_id": t["id"], "text": "What should I do?" }),
        )
        .unwrap_err();
    assert!(e["message"].as_str().unwrap().contains("Settings"));
}

#[rstest]
fn chat_streams_the_answer_as_it_comes(chatting: (World, String)) {
    let (w, t) = chatting;
    let s = UnixStream::connect(w.sock()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut events = BufReader::new(s.try_clone().unwrap());
    writeln!(
        &s,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 1, "method": "subscribe", "params": {} })
    )
    .unwrap();
    let mut line = String::new();
    events.read_line(&mut line).unwrap();
    ask(&w, &t, "What should I do?");
    let mut first = Value::Null;
    loop {
        line.clear();
        if events.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["params"]["kind"] == "chat_delta" {
            first = v["params"].clone();
            break;
        }
    }
    assert_eq!(first["text"], "Marc wants your sign-off. ");
}

/// The chat after the stub Claude paused `pauses` times before answering.
fn ask_after_pauses(w: &World, t: &str, pauses: usize) -> Value {
    w.mailbox.lock().unwrap().pause_turns = pauses;
    ask(w, t, "Who are they?")
}

#[rstest]
fn paused_turn_resumes_into_one_answer(chatting: (World, String)) {
    let (w, t) = chatting;
    assert_eq!(
        ask_after_pauses(&w, &t, 1)["text"],
        "Part one. Marc wants your sign-off. I'd reply today."
    );
}

#[rstest]
fn paused_turn_is_stored_as_one_answer(chatting: (World, String)) {
    let (w, t) = chatting;
    ask_after_pauses(&w, &t, 1);
    assert_eq!(
        w.ok("chat.get", json!({ "thread_id": t }))["messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[rstest]
fn paused_turn_is_sent_back_as_the_assistant_message(chatting: (World, String)) {
    let (w, t) = chatting;
    ask_after_pauses(&w, &t, 1);
    let m = w.mailbox.lock().unwrap();
    let last = m.chat_requests[1]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(
        last,
        json!({ "role": "assistant", "content": [
            { "type": "thinking", "thinking": "", "signature": "stub-sig" },
            { "type": "server_tool_use", "id": "srvtoolu_stub", "name": "web_search", "input": { "query": "Acme audit" } },
            { "type": "text", "text": "Part one. " }
        ] })
    );
}

#[rstest]
fn resuming_adds_no_user_message(chatting: (World, String)) {
    let (w, t) = chatting;
    ask_after_pauses(&w, &t, 1);
    let m = w.mailbox.lock().unwrap();
    assert_eq!(
        m.chat_requests[1]["messages"].as_array().unwrap().len(),
        m.chat_requests[0]["messages"].as_array().unwrap().len() + 1
    );
}

#[rstest]
fn resuming_stops_after_three_continuations(chatting: (World, String)) {
    let (w, t) = chatting;
    ask_after_pauses(&w, &t, 10);
    assert_eq!(w.mailbox.lock().unwrap().chat_requests.len(), 4);
}

#[rstest]
fn capped_turn_keeps_the_answer_so_far(chatting: (World, String)) {
    let (w, t) = chatting;
    assert_eq!(
        ask_after_pauses(&w, &t, 10)["text"],
        "Part one. Part one. Part one. Part one."
    );
}

#[rstest]
fn chatgpt_without_web_search_still_answers(synced: World) {
    chatgpt_sign_in(&synced);
    synced.mailbox.lock().unwrap().reject_tools = true;
    let acc = synced.ok("accounts.list", json!({}))[0]["id"].clone();
    synced.ok(
        "accounts.agent.set",
        json!({ "account_id": acc, "provider_ids": ["*"] }),
    );
    let t = synced.thread_by_subject("Audit report v2");
    assert_eq!(
        ask(&synced, t["id"].as_str().unwrap(), "What should I do?")["research_unavailable"],
        true
    );
}

#[rstest]
fn chatgpt_chat_asks_for_web_search(synced: World) {
    chatgpt_sign_in(&synced);
    let acc = synced.ok("accounts.list", json!({}))[0]["id"].clone();
    synced.ok(
        "accounts.agent.set",
        json!({ "account_id": acc, "provider_ids": ["*"] }),
    );
    let t = synced.thread_by_subject("Audit report v2");
    ask(&synced, t["id"].as_str().unwrap(), "Who is Marc?");
    assert_eq!(
        synced.mailbox.lock().unwrap().chat_requests[0]["tools"][0]["type"],
        "web_search"
    );
}

#[rstest]
fn cli_chat_prints_the_answer(chatting: (World, String)) {
    let (w, t) = chatting;
    let out = w.cli(&["chat", &t, "What", "should", "I", "do?"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("I'd reply today."));
}

#[rstest]
fn cli_chat_shows_proposals_as_commands(chatting: (World, String)) {
    let (w, t) = chatting;
    let out = w.cli(&["chat", &t, "What", "should", "I", "do?"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("mermoz status <thread> waiting"));
}

#[rstest]
fn cli_chat_json_lists_the_conversation(chatting: (World, String)) {
    let (w, t) = chatting;
    ask(&w, &t, "What should I do?");
    let out = w.cli(&["--json", "chat", &t]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["messages"][1]["role"], "assistant");
}

#[rstest]
fn mcp_chat_answers(chatting: (World, String)) {
    let (w, t) = chatting;
    let mut child = Command::new(bin_dir().join("mermoz"))
        .arg("mcp")
        .env("MERMOZ_HOME", w.home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "chat_about_thread", "arguments": { "id": t, "question": "What should I do?" } } })).unwrap();
    drop(stdin);
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let _ = child.wait();
    let v: Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
    assert!(
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Move to Waiting")
    );
}

// ---- reviewing suggestions before accepting -----------------------------

fn subjects(v: &Value) -> Vec<String> {
    v["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["subject"].as_str().unwrap().to_string())
        .collect()
}

#[rstest]
fn review_lists_the_threads_of_one_destination(synced: World) {
    sorted_todo(&synced);
    let v = synced.ok(
        "threads.list",
        json!({ "status": ["todo"], "suggested": { "status": "archive", "projects": ["Receipts"] } }),
    );
    assert_eq!(subjects(&v), vec!["Budget sheet"]);
}

#[rstest]
fn review_of_another_destination_is_empty(synced: World) {
    sorted_todo(&synced);
    let v = synced.ok(
        "threads.list",
        json!({ "status": ["todo"], "suggested": { "status": "done" } }),
    );
    assert_eq!(v["total"], 0);
}

#[rstest]
fn review_of_everything_lists_suggested_threads(synced: World) {
    sorted_todo(&synced);
    let v = synced.ok(
        "threads.list",
        json!({ "status": ["todo"], "suggested": true }),
    );
    assert_eq!(subjects(&v), vec!["Budget sheet"]);
}

#[rstest]
fn rejecting_drops_the_thread_from_its_group(synced: World) {
    sorted_todo(&synced);
    let t = synced.thread_by_subject("Budget sheet");
    synced.ok("threads.reject_suggestions", json!({ "ids": [t["id"]] }));
    assert_eq!(todo_column(&synced)["pending"], 0);
}

#[rstest]
fn rejecting_keeps_the_thread_in_place(synced: World) {
    sorted_todo(&synced);
    let t = synced.thread_by_subject("Budget sheet");
    synced.ok("threads.reject_suggestions", json!({ "ids": [t["id"]] }));
    assert_eq!(synced.thread_by_subject("Budget sheet")["status"], "todo");
}

#[rstest]
fn undoing_a_rejection_brings_the_suggestion_back(synced: World) {
    sorted_todo(&synced);
    let t = synced.thread_by_subject("Budget sheet");
    synced.ok("threads.reject_suggestions", json!({ "ids": [t["id"]] }));
    synced.ok("threads.reopen_suggestions", json!({ "ids": [t["id"]] }));
    assert_eq!(todo_column(&synced)["pending"], 1);
}

#[rstest]
fn cli_lists_the_threads_of_a_destination(synced: World) {
    sorted_todo(&synced);
    let out = synced.cli(&[
        "--json",
        "suggestions",
        "todo",
        "--to",
        "archive",
        "--to-project",
        "Receipts",
    ]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(subjects(&v), vec!["Budget sheet"]);
}

#[rstest]
fn cli_rejects_a_suggestion(synced: World) {
    sorted_todo(&synced);
    let t = synced.thread_by_subject("Budget sheet");
    synced.cli(&["reject", t["number"].as_str().unwrap()]);
    assert!(synced.thread_by_subject("Budget sheet")["suggestion"].is_null());
}

// ---- conversations ---------------------------------------------------------

const DAY_MS: i64 = 86_400_000;

fn sent_raw(to: &str, subject: &str, body: &str, msg_id: &str) -> String {
    format!(
        "From: Me <me@example.com>\r\nTo: {to}\r\nSubject: {subject}\r\nMessage-ID: <{msg_id}@example.com>\r\nDate: Sun, 4 Oct 2026 08:12:00 +0000\r\n\r\n{body}\r\n"
    )
}

/// The user wrote to Nora about the flat five days ago; she now starts a new thread.
fn with_nora(world: World, later_subject: &str) -> World {
    {
        let mut m = world.mailbox.lock().unwrap();
        m.add_message(
            "n1",
            "ga",
            &["SENT"],
            &sent_raw(
                "Nora Lind <nora@agency.example>",
                "Flat sale, next steps",
                "Here is the cellar plan, see https://notary.example/rdv",
                "n1",
            ),
            NOW - 5 * DAY_MS,
        );
        m.add_message(
            "n2",
            "gb",
            &["INBOX"],
            &raw(
                "Nora Lind <nora@agency.example>",
                later_subject,
                "The signing moves to the 20th.",
                "n2",
            ),
            NOW - DAY_MS,
        );
    }
    sign_in(&world);
    world.wait_until("import with Nora's threads", |w| {
        w.ok("threads.list", json!({}))["total"] == 5
    });
    world
}

fn conversation_of(w: &World, subject: &str) -> Value {
    w.thread_by_subject(subject)["conversation_id"].clone()
}

fn wait_joined(w: &World, later: &str) {
    w.wait_until("the threads to join", |w| {
        conversation_of(w, later) == conversation_of(w, "Flat sale, next steps")
    });
}

/// Waits until grouping has settled every thread.
fn wait_grouped(w: &World) {
    w.wait_until("grouping to finish", |w| {
        let p = w.ok("conversations.progress", json!({}));
        p["pending"] == 0 && p["waiting_for_ai"] == 0
    });
}

#[fixture]
fn nora(world: World) -> World {
    let w = with_nora(world, "Signing postponed");
    w.ok("router.set", json!({ "api_key": "jev-test" }));
    wait_joined(&w, "Signing postponed");
    w
}

#[rstest]
fn a_continuation_joins_the_earlier_conversation(nora: World) {
    assert_eq!(
        conversation_of(&nora, "Signing postponed"),
        conversation_of(&nora, "Flat sale, next steps")
    );
}

#[rstest]
fn conversation_lists_show_one_row(nora: World) {
    let v = nora.ok(
        "threads.list",
        json!({ "conversations": true, "participant": ["nora@agency.example"] }),
    );
    assert_eq!(v["total"], 1);
}

#[rstest]
fn the_row_counts_its_threads(nora: World) {
    assert_eq!(
        nora.thread_by_subject("Signing postponed")["conversation_threads"],
        2
    );
}

#[rstest]
fn opening_a_conversation_reads_every_thread(nora: World) {
    let id = nora.thread_by_subject("Signing postponed")["id"].clone();
    let v = nora.ok("threads.get", json!({ "id": id, "conversation": true }));
    assert_eq!(v["messages"].as_array().unwrap().len(), 2);
}

#[rstest]
fn the_conversation_lists_its_links(nora: World) {
    let id = nora.thread_by_subject("Signing postponed")["id"].clone();
    let v = nora.ok("threads.get", json!({ "id": id, "conversation": true }));
    assert_eq!(
        v["conversation"]["links"][0]["url"],
        "https://notary.example/rdv"
    );
}

#[rstest]
fn the_conversation_lists_its_people(nora: World) {
    let id = nora.thread_by_subject("Signing postponed")["id"].clone();
    let v = nora.ok("threads.get", json!({ "id": id, "conversation": true }));
    assert_eq!(
        v["conversation"]["people"][0]["email"],
        "nora@agency.example"
    );
}

#[rstest]
fn a_status_change_moves_the_whole_conversation(nora: World) {
    // The earlier thread is filed (sent mail); the new one is open and follows.
    let id = nora.thread_by_subject("Flat sale, next steps")["id"].clone();
    nora.ok("threads.update", json!({ "ids": [id], "status": "todo" }));
    assert_eq!(
        nora.thread_by_subject("Signing postponed")["status"],
        "todo"
    );
}

#[rstest]
fn reading_a_conversation_reads_every_thread(nora: World) {
    let id = nora.thread_by_subject("Signing postponed")["id"].clone();
    nora.ok("threads.mark_read", json!({ "id": id }));
    assert_eq!(nora.thread_by_subject("Signing postponed")["unread"], false);
}

#[rstest]
fn splitting_takes_a_thread_out(nora: World) {
    let id = nora.thread_by_subject("Signing postponed")["id"].clone();
    nora.ok("conversations.split", json!({ "id": id }));
    assert_ne!(
        conversation_of(&nora, "Signing postponed"),
        conversation_of(&nora, "Flat sale, next steps")
    );
}

#[rstest]
fn merging_joins_conversations_by_hand(nora: World) {
    let a = nora.thread_by_subject("Audit report v2")["id"].clone();
    let b = nora.thread_by_subject("Budget sheet")["id"].clone();
    nora.ok("conversations.merge", json!({ "ids": [a, b] }));
    assert_eq!(
        conversation_of(&nora, "Audit report v2"),
        conversation_of(&nora, "Budget sheet")
    );
}

#[rstest]
fn chat_about_a_conversation_reads_every_thread(nora: World) {
    allow_ai(&nora);
    let id = nora.thread_by_subject("Signing postponed")["id"].clone();
    nora.ok(
        "chat.send",
        json!({ "thread_id": id, "text": "What's next?" }),
    );
    let asked = nora.mailbox.lock().unwrap().chat_requests[0].to_string();
    assert!(asked.contains("cellar plan"));
}

#[rstest]
fn an_unrelated_thread_stays_apart(world: World) {
    let w = with_nora(world, "Unrelated party");
    w.ok("router.set", json!({ "api_key": "jev-test" }));
    wait_grouped(&w);
    assert_ne!(
        conversation_of(&w, "Unrelated party"),
        conversation_of(&w, "Flat sale, next steps")
    );
}

#[rstest]
fn the_ai_settles_an_uncertain_pair(world: World) {
    let w = with_nora(world, "Maybe the signing");
    allow_ai(&w);
    w.ok("router.set", json!({ "api_key": "jev-test" }));
    wait_joined(&w, "Maybe the signing");
    assert_eq!(w.mailbox.lock().unwrap().grouping_asks, 1);
}

#[rstest]
fn without_an_ai_an_uncertain_pair_stays_apart(world: World) {
    let w = with_nora(world, "Maybe the signing");
    w.ok("router.set", json!({ "api_key": "jev-test" }));
    wait_grouped(&w);
    assert_ne!(
        conversation_of(&w, "Maybe the signing"),
        conversation_of(&w, "Flat sale, next steps")
    );
}

#[rstest]
fn automated_mail_from_one_sender_is_one_feed(synced: World) {
    synced.mailbox.lock().unwrap().add_message(
        "b2",
        "tb2",
        &["INBOX"],
        &raw(
            "Bank <noreply@bank.example>",
            "Card payment",
            "A payment was made",
            "b2",
        ),
        NOW + 1000,
    );
    synced.wait_until("the second bank mail to join the feed", |w| {
        let listed = w.ok("threads.list", json!({}))["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["subject"] == "Card payment");
        listed && conversation_of(w, "Card payment") == conversation_of(w, "Your statement")
    });
}

#[rstest]
fn feeds_can_be_listed_alone(synced: World) {
    wait_grouped(&synced);
    let v = synced.ok(
        "threads.list",
        json!({ "conversations": true, "kind": ["feed", "list"] }),
    );
    assert_eq!(v["threads"][0]["subject"], "Your statement");
}

#[rstest]
fn shared_files_are_listed(synced: World) {
    let v = synced.ok("files.list", json!({}));
    assert_eq!(v[0]["filename"], "report.pdf");
}
