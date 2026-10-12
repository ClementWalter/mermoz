//! End-to-end: real mermozd + real `mermoz` CLI against a stub Gmail. Covers
//! sign-in, import, triage, label write-back, reopen rule and the send gate.

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
fn ai_triage_moves_a_column(synced: World) {
    enable_agent(&synced);
    let v = synced.ok("agent.triage", json!({ "status": "todo" }));
    assert_eq!(v["moved"], 1);
}

#[rstest]
fn ai_triage_applies_the_moves(synced: World) {
    enable_agent(&synced);
    synced.ok("agent.triage", json!({ "status": "todo" }));
    assert_eq!(synced.thread_by_subject("Budget sheet")["status"], "done");
}

#[rstest]
fn ai_triage_needs_an_allowed_provider(synced: World) {
    assert!(
        synced
            .call("agent.triage", json!({ "status": "unread" }))
            .is_err()
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

#[rstest]
fn ai_triage_does_not_count_threads_it_leaves_as_they_are(synced: World) {
    enable_agent(&synced);
    let t = synced.thread_by_subject("Budget sheet");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "status": "done" }),
    );
    // The stub AI sends everything to Done, where this thread already is.
    assert_eq!(
        synced.ok("agent.triage", json!({ "status": "done" }))["moved"],
        0
    );
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
