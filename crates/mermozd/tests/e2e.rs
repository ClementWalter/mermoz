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

    fn wait_for_socket(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(self.sock()).is_err() {
            assert!(Instant::now() < deadline, "daemon did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Stops the daemon and starts a fresh one on the same home, as after a reboot.
    fn restart(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_file(self.sock());
        self.daemon = spawn_daemon(self.home.path(), self.port, &[]);
        self.wait_for_socket();
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
    start_world(&[])
}

/// Daemon started with extra environment (e.g. to force a code path in tests).
fn start_world(extra_env: &[(&str, &str)]) -> World {
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
    launch(home, mailbox, extra_env)
}

/// Starts the stub servers and a daemon on `home` with extra environment.
fn launch(home: tempfile::TempDir, mailbox: Shared, extra_env: &[(&str, &str)]) -> World {
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
    let daemon = spawn_daemon(home.path(), port, extra_env);
    let w = World {
        home,
        mailbox,
        daemon,
        port,
    };
    w.wait_for_socket();
    w
}

/// A daemon on `home`, talking to the stubs on `port`.
fn spawn_daemon(home: &std::path::Path, port: u16, extra_env: &[(&str, &str)]) -> Child {
    let fake_ant = home.join("fake-ant");
    Command::new(env!("CARGO_BIN_EXE_mermozd"))
        .env("MERMOZ_ANT", &fake_ant)
        .env(
            "MERMOZ_CHATGPT_AUTH_BASE",
            format!("http://127.0.0.1:{port}/oa"),
        )
        .env(
            "MERMOZ_OPENAI_API_BASE",
            format!("http://127.0.0.1:{port}/oa/v1"),
        )
        .env("MERMOZ_HOME", home)
        .env("MERMOZ_TOKEN_STORE", "file")
        .env(
            "MERMOZ_GMAIL_BASE",
            format!("http://127.0.0.1:{port}/gmail"),
        )
        .env("MERMOZ_POLL_SECS", "1")
        .env("MERMOZ_JEV_BASE", format!("http://127.0.0.1:{port}/jev"))
        .env("MERMOZ_SUBQ_BASE", format!("http://127.0.0.1:{port}/subq"))
        .env("MERMOZ_TRUSTED_EXE", std::env::current_exe().unwrap())
        .envs(extra_env.iter().copied())
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

/// A folder holding "Audit report v2" is described to Jev by its people alone.
#[rstest]
fn router_describes_a_folder_without_its_subjects(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    synced.ok(
        "threads.update",
        json!({ "ids": [t["id"]], "add_projects": ["Receipts"] }),
    );
    synced.ok("router.set", json!({ "api_key": "jev-test" }));
    synced.wait_until("routing", |w| {
        !w.mailbox.lock().unwrap().folder_criteria.is_empty()
    });
    assert_eq!(
        synced.mailbox.lock().unwrap().folder_criteria[0]["Receipts"],
        "Folder 'Receipts'. Frequent correspondents: Marc Petit."
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
fn mark_read_answers_with_the_thread_read(synced: World) {
    let t = synced.thread_by_subject("Audit report v2");
    let r = synced.ok("threads.mark_read", json!({ "id": t["id"] }));
    assert_eq!(r["thread"]["unread"], false);
}

#[rstest]
fn mark_read_answers_with_the_status_reading_applied(synced: World) {
    enable_router(&synced);
    let t = synced.thread_by_subject("Audit report v2");
    let r = synced.ok("threads.mark_read", json!({ "id": t["id"] }));
    assert_eq!(r["thread"]["status"], "archive");
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

// ---- suggestions on the Feed's cards -----------------------------------------

/// Asks for the suggestions of the To do threads, as the Feed does for its cards.
fn suggest_todo(w: &World) -> Value {
    let ids: Vec<Value> = w.ok("threads.list", json!({ "status": ["todo"] }))["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].clone())
        .collect();
    w.ok("threads.suggest", json!({ "ids": ids }))
}

/// The threads `threads.suggest` returned a suggestion for.
fn suggested(v: &Value) -> Vec<Value> {
    v["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].clone())
        .collect()
}

/// Turns the router on and asks for To do's suggestions until the read
/// "Budget sheet" has its suggestion: the stub files everything in Receipts, to archive.
fn sorted_todo(w: &World) {
    enable_router(w);
    w.wait_until("suggestions", |w| {
        suggest_todo(w);
        !w.thread_by_subject("Budget sheet")["suggestion"].is_null()
    });
}

fn todo_column(w: &World) -> Value {
    w.ok("suggestions.summary", json!({ "statuses": ["todo"] }))["columns"][0].clone()
}

#[rstest]
#[case::queue("sort.next")]
#[case::counts("sort.state")]
fn the_sort_queue_is_gone(synced: World, #[case] method: &str) {
    assert!(synced.call(method, json!({})).is_err());
}

#[rstest]
fn nothing_is_suggested_on_read_threads_before_the_feed_asks(synced: World) {
    enable_router(&synced);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(synced.thread_by_subject("Budget sheet")["suggestion"].is_null());
}

#[rstest]
fn the_card_asked_about_gets_its_suggestion(synced: World) {
    enable_router(&synced);
    assert_eq!(
        suggested(&suggest_todo(&synced)),
        vec![synced.thread_by_subject("Budget sheet")["id"].clone()]
    );
}

#[rstest]
fn a_suggestion_says_where_it_goes(synced: World) {
    enable_router(&synced);
    assert_eq!(
        suggest_todo(&synced)["suggestions"][0]["suggestion"]["status"],
        "archive"
    );
}

#[rstest]
fn threads_not_asked_about_get_no_suggestion(synced: World) {
    more_from_jonas(&synced, 1);
    enable_router(&synced);
    let id = synced.thread_by_subject("Budget sheet")["id"].clone();
    synced.ok("threads.suggest", json!({ "ids": [id] }));
    assert!(synced.thread_by_subject("Sheet 0")["suggestion"].is_null());
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
    w.wait_until("suggestions", |w| {
        suggest_todo(w);
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
fn suggestions_stop_at_the_daily_cap(synced: World) {
    synced.ok("router.set", json!({ "daily_cap": 0 }));
    enable_router(&synced);
    assert_eq!(suggest_todo(&synced)["suggestions"], json!([]));
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

// ---- stopping the flood ---------------------------------------------------

#[rstest]
fn stopping_a_resort_clears_unreviewed_suggestions(synced: World) {
    sorted_todo(&synced);
    synced.cli(&["resort", "--stop"]);
    assert!(synced.thread_by_subject("Budget sheet")["suggestion"].is_null());
}

#[rstest]
fn cleared_thread_is_suggested_again(synced: World) {
    sorted_todo(&synced);
    synced.cli(&["suggestions", "--clear"]);
    assert_eq!(suggested(&suggest_todo(&synced)).len(), 1);
}

#[rstest]
fn clearing_keeps_unread_suggestions(synced: World) {
    sorted_todo(&synced);
    synced.cli(&["suggestions", "--clear"]);
    assert!(!synced.thread_by_subject("Audit report v2")["suggestion"].is_null());
}

#[rstest]
fn starting_a_whole_mailbox_sort_is_refused(synced: World) {
    enable_router(&synced);
    assert!(synced.call("suggestions.resort", json!({})).is_err());
}

/// Router on, then TypeSafe runs out of credits.
fn out_of_credits(w: &World) {
    enable_router(w);
    w.mailbox.lock().unwrap().jev_out_of_credits = true;
}

#[rstest]
fn out_of_credits_pauses_sorting(synced: World) {
    out_of_credits(&synced);
    assert_eq!(suggest_todo(&synced)["paused"], "credits");
}

#[rstest]
fn out_of_credits_asks_again_once_topped_up(synced: World) {
    out_of_credits(&synced);
    suggest_todo(&synced);
    // Topped up: the thread was never judged, so it is asked about again.
    synced.mailbox.lock().unwrap().jev_out_of_credits = false;
    synced.ok("router.set", json!({ "api_key": "jev-topped-up" }));
    assert_eq!(suggested(&suggest_todo(&synced)).len(), 1);
}

#[rstest]
fn out_of_credits_suggests_nothing(synced: World) {
    out_of_credits(&synced);
    assert_eq!(suggest_todo(&synced)["suggestions"], json!([]));
}

#[rstest]
fn out_of_credits_stops_asking(synced: World) {
    out_of_credits(&synced);
    suggest_todo(&synced);
    let before = synced.mailbox.lock().unwrap().jev_auth.len();
    suggest_todo(&synced);
    assert_eq!(synced.mailbox.lock().unwrap().jev_auth.len(), before);
}

#[rstest]
fn out_of_credits_is_said_in_plain_words(synced: World) {
    out_of_credits(&synced);
    suggest_todo(&synced);
    assert_eq!(
        synced.ok("router.get", json!({}))["error"],
        "Sorting paused: your TypeSafe credits ran out"
    );
}

#[rstest]
fn a_new_key_resumes_sorting(synced: World) {
    out_of_credits(&synced);
    suggest_todo(&synced);
    synced.mailbox.lock().unwrap().jev_out_of_credits = false;
    synced.ok("router.set", json!({ "api_key": "jev-topped-up" }));
    assert!(suggest_todo(&synced)["paused"].is_null());
}

// ---- instant sorting providers ---------------------------------------------

/// SubQ picked with its own key; new mail is routed through it.
fn subq_router(w: &World) {
    w.ok("projects.create", json!({ "name": "Receipts" }));
    w.ok(
        "router.set",
        json!({ "provider": "subq", "api_key": "subq-test" }),
    );
    w.wait_until("routing", |w| {
        !w.thread_by_subject("Audit report v2")["suggestion"].is_null()
    });
}

/// Models asked of one stub path, without repeats.
fn models_sent(w: &World, which: &str) -> Vec<String> {
    let mut m: Vec<String> = w
        .mailbox
        .lock()
        .unwrap()
        .system_one_models
        .iter()
        .filter(|(p, _)| *p == which)
        .map(|(_, m)| m.clone())
        .collect();
    m.dedup();
    m
}

#[rstest]
fn router_reports_typesafe_by_default(synced: World) {
    assert_eq!(synced.ok("router.get", json!({}))["provider"], "typesafe");
}

#[rstest]
fn router_reports_the_picked_provider(synced: World) {
    synced.ok("router.set", json!({ "provider": "subq" }));
    assert_eq!(synced.ok("router.get", json!({}))["provider"], "subq");
}

#[rstest]
fn router_refuses_an_unknown_provider(synced: World) {
    assert!(
        synced
            .call("router.set", json!({ "provider": "openai" }))
            .is_err()
    );
}

#[rstest]
fn subq_is_sent_its_own_key(synced: World) {
    subq_router(&synced);
    assert!(
        synced
            .mailbox
            .lock()
            .unwrap()
            .subq_auth
            .contains(&"Bearer subq-test".to_string())
    );
}

#[rstest]
fn subq_is_asked_for_its_model(synced: World) {
    subq_router(&synced);
    assert_eq!(models_sent(&synced, "subq"), vec!["subq-snap-preview"]);
}

#[rstest]
fn typesafe_is_asked_for_jev(synced: World) {
    enable_router(&synced);
    assert_eq!(models_sent(&synced, "jev"), vec!["jev-latest"]);
}

#[rstest]
fn picking_subq_sends_nothing_to_typesafe(synced: World) {
    subq_router(&synced);
    synced.ok(
        "router.set",
        json!({ "api_key": "jev-test", "key_provider": "typesafe" }),
    );
    suggest_todo(&synced);
    assert!(synced.mailbox.lock().unwrap().jev_auth.is_empty());
}

#[rstest]
fn router_reports_the_model_of_the_picked_provider(synced: World) {
    synced.ok("router.set", json!({ "provider": "subq" }));
    assert_eq!(
        synced.ok("router.get", json!({}))["model"],
        "subq-snap-preview"
    );
}

/// A key saved for each provider, TypeSafe in use.
fn both_keys(w: &World) {
    w.ok("router.set", json!({ "api_key": "jev-test" }));
    w.ok(
        "router.set",
        json!({ "api_key": "subq-test", "key_provider": "subq" }),
    );
}

fn keys_held(w: &World) -> Vec<bool> {
    w.ok("router.get", json!({}))["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["configured"].as_bool().unwrap())
        .collect()
}

#[rstest]
fn each_provider_keeps_its_key(synced: World) {
    both_keys(&synced);
    assert_eq!(keys_held(&synced), vec![true, true]);
}

#[rstest]
fn a_key_for_the_other_provider_does_not_switch(synced: World) {
    both_keys(&synced);
    assert_eq!(synced.ok("router.get", json!({}))["provider"], "typesafe");
}

#[rstest]
fn removing_one_key_keeps_the_other(synced: World) {
    both_keys(&synced);
    synced.ok(
        "router.set",
        json!({ "api_key": null, "key_provider": "subq" }),
    );
    assert_eq!(keys_held(&synced), vec![true, false]);
}

#[rstest]
fn switching_back_finds_the_kept_key(synced: World) {
    both_keys(&synced);
    synced.ok("router.set", json!({ "provider": "subq" }));
    synced.ok("router.set", json!({ "provider": "typesafe" }));
    assert_eq!(synced.ok("router.get", json!({}))["configured"], true);
}

#[rstest]
fn provider_calls_are_counted_per_provider(synced: World) {
    subq_router(&synced);
    let v = synced.ok("router.get", json!({}));
    assert!(v["providers"][1]["calls_today"].as_i64().unwrap() > 0);
}

#[rstest]
fn router_test_uses_the_named_provider(synced: World) {
    synced.ok(
        "router.test",
        json!({ "provider": "subq", "api_key": "subq-probe" }),
    );
    assert!(
        synced
            .mailbox
            .lock()
            .unwrap()
            .subq_auth
            .contains(&"Bearer subq-probe".to_string())
    );
}

#[rstest]
fn agents_cannot_pick_the_provider(synced: World) {
    let script = r#"
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
s.sendall((json.dumps({"jsonrpc": "2.0", "id": 1, "method": "router.set", "params": {"provider": "subq"}}) + "\n").encode())
print(s.makefile().readline())
"#;
    let out = Command::new("python3")
        .args(["-c", script, synced.sock().to_str().unwrap()])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], -32011);
}

/// A SubQ key saved through `router.set`, TypeSafe still in use.
fn subq_key_saved(w: &World) {
    w.ok(
        "router.set",
        json!({ "api_key": "subq-test", "key_provider": "subq" }),
    );
}

/// The CLI's line for SubQ in `mermoz router`.
fn cli_subq_line(w: &World) -> String {
    let out = w.cli(&["router"]);
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.trim_start().starts_with("SubQ"))
        .unwrap_or_default()
        .to_string()
}

#[rstest]
fn a_saved_subq_key_is_reported(synced: World) {
    subq_key_saved(&synced);
    assert_eq!(
        synced.ok("router.get", json!({}))["providers"][1]["configured"],
        true
    );
}

#[rstest]
fn the_cli_reports_a_saved_subq_key(synced: World) {
    subq_key_saved(&synced);
    assert!(cli_subq_line(&synced).contains(" key "));
}

#[rstest]
fn a_saved_subq_key_is_written_to_the_secret_store(synced: World) {
    subq_key_saved(&synced);
    assert!(
        synced
            .home
            .path()
            .join("tokens/ai.mermoz.subq/api_key")
            .exists()
    );
}

#[rstest]
fn a_saved_subq_key_survives_a_restart(mut synced: World) {
    subq_key_saved(&synced);
    synced.restart();
    assert_eq!(
        synced.ok("router.get", json!({}))["providers"][1]["configured"],
        true
    );
}

#[rstest]
fn the_cli_reports_the_subq_key_after_a_restart(mut synced: World) {
    subq_key_saved(&synced);
    synced.restart();
    assert!(cli_subq_line(&synced).contains(" key "));
}

#[rstest]
fn the_picked_provider_survives_a_restart(mut synced: World) {
    synced.ok(
        "router.set",
        json!({ "provider": "subq", "api_key": "subq-test" }),
    );
    synced.restart();
    assert_eq!(synced.ok("router.get", json!({}))["provider"], "subq");
}

/// TypeSafe in use and out of credits, with a SubQ key saved.
fn typesafe_spent_subq_ready(w: &World) {
    out_of_credits(w);
    w.ok(
        "router.set",
        json!({ "api_key": "subq-test", "key_provider": "subq" }),
    );
    suggest_todo(w);
}

#[rstest]
fn out_of_credits_offers_the_other_provider(synced: World) {
    typesafe_spent_subq_ready(&synced);
    assert_eq!(
        synced.ok("router.get", json!({}))["offer"]["message"],
        "TypeSafe credits ran out. Use SubQ instead?"
    );
}

#[rstest]
fn suggestions_carry_the_offer(synced: World) {
    typesafe_spent_subq_ready(&synced);
    assert_eq!(suggest_todo(&synced)["offer"]["provider"], "subq");
}

#[rstest]
fn out_of_credits_never_switches_by_itself(synced: World) {
    typesafe_spent_subq_ready(&synced);
    assert_eq!(synced.ok("router.get", json!({}))["provider"], "typesafe");
}

#[rstest]
fn no_offer_without_a_key_for_the_other(synced: World) {
    out_of_credits(&synced);
    suggest_todo(&synced);
    assert!(synced.ok("router.get", json!({}))["offer"].is_null());
}

#[rstest]
fn the_hold_stays_with_the_spent_provider(synced: World) {
    typesafe_spent_subq_ready(&synced);
    synced.ok("router.set", json!({ "provider": "subq" }));
    assert!(suggest_todo(&synced)["paused"].is_null());
}

#[rstest]
fn the_other_provider_sorts_after_the_switch(synced: World) {
    typesafe_spent_subq_ready(&synced);
    synced.ok("router.set", json!({ "provider": "subq" }));
    synced.wait_until("sort queue", |w| {
        suggest_todo(w);
        !w.thread_by_subject("Budget sheet")["suggestion"].is_null()
    });
    assert_eq!(
        synced.thread_by_subject("Budget sheet")["suggestion"]["status"],
        "archive"
    );
}

#[rstest]
fn switching_back_finds_the_provider_still_on_hold(synced: World) {
    typesafe_spent_subq_ready(&synced);
    synced.ok("router.set", json!({ "provider": "subq" }));
    synced.ok("router.set", json!({ "provider": "typesafe" }));
    assert_eq!(suggest_todo(&synced)["paused"], "credits");
}

#[rstest]
fn subq_out_of_credits_is_said_with_its_name(synced: World) {
    subq_router(&synced);
    synced.mailbox.lock().unwrap().subq_out_of_credits = true;
    suggest_todo(&synced);
    assert_eq!(
        synced.ok("router.get", json!({}))["error"],
        "Sorting paused: your SubQ credits ran out"
    );
}

#[rstest]
fn new_mail_shows_while_the_provider_is_slow(synced: World) {
    enable_router(&synced);
    {
        let mut m = synced.mailbox.lock().unwrap();
        m.system_one_delay_ms = 4000;
        m.add_message(
            "m9",
            "t9",
            &["INBOX", "UNREAD"],
            &raw("Lina <lina@example.com>", "Slow day", "Hello", "m9"),
            NOW + 9000,
        );
    }
    synced.wait_until("new mail", |w| {
        w.ok("threads.list", json!({ "limit": 100 }))["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["subject"] == "Slow day")
    });
    assert!(synced.thread_by_subject("Slow day")["suggestion"].is_null());
}

// ---- answering suggestions, and what they learn ----------------------------

/// `n` more read To do threads from Jonas, the "Budget sheet" sender.
fn more_from_jonas(w: &World, n: usize) {
    {
        let mut m = w.mailbox.lock().unwrap();
        for i in 0..n {
            let id = format!("j{i}");
            m.add_message(
                &id,
                &format!("tj{i}"),
                &["INBOX"],
                &raw(
                    "Jonas <jonas@example.com>",
                    &format!("Sheet {i}"),
                    "Numbers",
                    &id,
                ),
                NOW - 5000 - i as i64,
            );
        }
    }
    w.wait_until("sync", |w| {
        w.ok("threads.list", json!({}))["total"] == 3 + n as i64
    });
}

fn answer(w: &World, subject: &str, a: &str) -> Value {
    let t = w.thread_by_subject(subject);
    w.ok("sort.answer", json!({ "id": t["id"], "answer": a }))
}

#[rstest]
fn yes_moves_the_thread(synced: World) {
    sorted_todo(&synced);
    answer(&synced, "Budget sheet", "yes");
    assert_eq!(
        synced.thread_by_subject("Budget sheet")["status"],
        "archive"
    );
}

#[rstest]
fn undo_puts_the_card_back(synced: World) {
    sorted_todo(&synced);
    let v = answer(&synced, "Budget sheet", "yes");
    synced.ok("sort.undo", json!({ "undo": v["undo"] }));
    assert_eq!(synced.thread_by_subject("Budget sheet")["status"], "todo");
}

#[rstest]
fn change_files_where_the_user_chose(synced: World) {
    sorted_todo(&synced);
    let t = synced.thread_by_subject("Budget sheet");
    synced.ok(
        "sort.answer",
        json!({ "id": t["id"], "answer": "change", "status": "waiting" }),
    );
    assert_eq!(
        synced.thread_by_subject("Budget sheet")["status"],
        "waiting"
    );
}

#[rstest]
fn no_means_the_same_sender_is_not_offered_it_again(synced: World) {
    more_from_jonas(&synced, 1);
    sorted_todo(&synced);
    answer(&synced, "Budget sheet", "no");
    suggest_todo(&synced);
    assert!(synced.thread_by_subject("Sheet 0")["suggestion"].is_null());
}

#[rstest]
fn third_yes_teaches_a_rule(synced: World) {
    more_from_jonas(&synced, 2);
    enable_router(&synced);
    let mut last = Value::Null;
    for subject in ["Budget sheet", "Sheet 0", "Sheet 1"] {
        synced.wait_until("card", |w| {
            suggest_todo(w);
            !w.thread_by_subject(subject)["suggestion"].is_null()
        });
        last = answer(&synced, subject, "yes");
    }
    assert_eq!(
        last["learned"],
        "Learned: mail from Jonas goes to archive · Receipts"
    );
}

#[rstest]
fn learned_rule_is_listed(synced: World) {
    sorted_todo(&synced);
    answer(&synced, "Budget sheet", "no");
    assert_eq!(
        synced.ok("learned.list", json!({}))["learned"][0]["kind"],
        "avoid"
    );
}

#[rstest]
fn forgotten_refusal_is_offered_again(synced: World) {
    more_from_jonas(&synced, 1);
    sorted_todo(&synced);
    answer(&synced, "Budget sheet", "no");
    let item = synced.ok("learned.list", json!({}))["learned"][0].clone();
    synced.ok("learned.forget", json!({ "item": item }));
    let id = synced.thread_by_subject("Sheet 0")["id"].clone();
    assert_eq!(
        suggested(&synced.ok("threads.suggest", json!({ "ids": [id] }))),
        vec![id]
    );
}

/// `n` read automated mails from one no-reply sender, "Parcel 0" to "Parcel n-1".
fn parcels(w: &World, n: usize) {
    {
        let mut m = w.mailbox.lock().unwrap();
        for i in 0..n {
            let id = format!("p{i}");
            m.add_message(
                &id,
                &format!("tp{i}"),
                &["INBOX"],
                &raw(
                    "Parcel Hub <noreply@parcels.example>",
                    &format!("Parcel {i}"),
                    "On its way",
                    &id,
                ),
                NOW - 7000 - i as i64,
            );
        }
    }
    w.wait_until("sync", |w| {
        w.ok("threads.list", json!({}))["total"] == 3 + n as i64
    });
}

#[rstest]
fn three_accepted_archives_put_the_sender_in_the_batch_card(synced: World) {
    parcels(&synced, 4);
    // No Receipts folder: the stub suggests Archive alone.
    synced.ok("router.set", json!({ "api_key": "jev-test" }));
    for i in 0..3 {
        let id = synced.thread_by_subject(&format!("Parcel {i}"))["id"].clone();
        synced.wait_until("suggestion", |w| {
            !suggested(&w.ok("threads.suggest", json!({ "ids": [id] }))).is_empty()
        });
        synced.ok("sort.answer", json!({ "id": id, "answer": "yes" }));
    }
    let batch = synced.ok("feed.list", json!({}))["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["cards"].as_array().unwrap().clone())
        .find(|c| c["id"] == "batch:cleanup")
        .unwrap();
    assert!(
        batch["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["subject"] == "Parcel 3")
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

// ---- attachments -------------------------------------------------------

const PDF_BYTES: &[u8] = b"%PDF-1.4 fictional plan";
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nfictional\x00\xff\xfe";

/// Two tiny fictional files on disk, one with a French accented name.
fn fixture_files(w: &World) -> (PathBuf, PathBuf) {
    let dir = w.home.path().join("to-attach");
    std::fs::create_dir_all(&dir).unwrap();
    let pdf = dir.join("Relevé d'été.pdf");
    let png = dir.join("chart.png");
    std::fs::write(&pdf, PDF_BYTES).unwrap();
    std::fs::write(&png, PNG_BYTES).unwrap();
    (pdf, png)
}

/// The last message the stub Gmail received, parsed.
fn last_sent(w: &World) -> mail_parser::Message<'static> {
    let raw = w.mailbox.lock().unwrap().sent_raw.last().cloned().unwrap();
    mail_parser::MessageParser::default()
        .parse(&raw)
        .unwrap()
        .into_owned()
}

/// The app (a trusted caller) sends a new message with both fixture files.
fn app_sends_two_files(w: &World) {
    let (pdf, png) = fixture_files(w);
    let d = w.ok(
        "drafts.create",
        json!({ "to": ["marc@example.com"], "subject": "Plan", "body": "Both files attached.",
                "attachments": [pdf, { "path": png, "inline": true }] }),
    );
    w.ok("drafts.send", json!({ "id": d["id"] }));
}

/// A reply to "Audit report v2" carrying the PDF.
fn app_replies_with_pdf(w: &World) {
    let (pdf, _) = fixture_files(w);
    let t = w.thread_by_subject("Audit report v2");
    let d = w.ok(
        "drafts.create",
        json!({ "thread_id": t["id"], "body": "Signed copy attached.", "attachments": [pdf] }),
    );
    w.ok("drafts.send", json!({ "id": d["id"] }));
}

#[fixture]
fn upload_synced() -> World {
    // Every send takes the large-message path, so tiny fixtures exercise it.
    let w = start_world(&[("MERMOZ_GMAIL_UPLOAD_OVER", "0")]);
    sign_in(&w);
    w.wait_until("initial import", |w| {
        w.ok("threads.list", json!({}))["total"] == 3
    });
    w
}

#[rstest]
fn sent_message_carries_both_files(synced: World) {
    app_sends_two_files(&synced);
    assert_eq!(last_sent(&synced).attachment_count(), 2);
}

#[rstest]
fn sent_pdf_keeps_its_bytes(synced: World) {
    app_sends_two_files(&synced);
    assert_eq!(
        last_sent(&synced).attachment(0).unwrap().contents(),
        PDF_BYTES
    );
}

#[rstest]
fn sent_png_keeps_its_bytes(synced: World) {
    app_sends_two_files(&synced);
    assert_eq!(
        last_sent(&synced).attachment(1).unwrap().contents(),
        PNG_BYTES
    );
}

#[rstest]
fn sent_accented_name_survives(synced: World) {
    use mail_parser::MimeHeaders;
    app_sends_two_files(&synced);
    assert_eq!(
        last_sent(&synced).attachment(0).unwrap().attachment_name(),
        Some("Relevé d'été.pdf")
    );
}

#[rstest]
fn sent_text_is_the_first_part(synced: World) {
    app_sends_two_files(&synced);
    assert_eq!(
        last_sent(&synced).body_text(0).unwrap().trim(),
        "Both files attached."
    );
}

#[rstest]
fn sent_draft_files_are_cleaned_up(synced: World) {
    app_sends_two_files(&synced);
    let left = std::fs::read_dir(synced.home.path().join("blobs/drafts"))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(left, 0);
}

#[rstest]
fn reply_with_file_stays_in_the_gmail_thread(synced: World) {
    app_replies_with_pdf(&synced);
    assert_eq!(
        synced.mailbox.lock().unwrap().sent[0].1.as_deref(),
        Some("t1")
    );
}

#[rstest]
fn reply_with_file_keeps_in_reply_to(synced: World) {
    app_replies_with_pdf(&synced);
    assert_eq!(
        last_sent(&synced).in_reply_to().as_text(),
        Some("m1@example.com")
    );
}

#[rstest]
fn large_message_goes_through_the_upload_endpoint(upload_synced: World) {
    app_replies_with_pdf(&upload_synced);
    assert_eq!(
        upload_synced.mailbox.lock().unwrap().sent_via,
        vec!["upload"]
    );
}

#[rstest]
fn uploaded_message_keeps_its_thread(upload_synced: World) {
    app_replies_with_pdf(&upload_synced);
    assert_eq!(
        upload_synced.mailbox.lock().unwrap().sent[0].1.as_deref(),
        Some("t1")
    );
}

#[rstest]
fn uploaded_message_keeps_its_bytes(upload_synced: World) {
    app_replies_with_pdf(&upload_synced);
    assert_eq!(
        last_sent(&upload_synced).attachment(0).unwrap().contents(),
        PDF_BYTES
    );
}

#[rstest]
fn small_message_uses_the_json_endpoint(synced: World) {
    app_replies_with_pdf(&synced);
    assert_eq!(synced.mailbox.lock().unwrap().sent_via, vec!["json"]);
}

#[rstest]
fn message_attachment_can_be_sent_again(synced: World) {
    let t = synced.thread_by_subject("Budget sheet");
    let full = synced.ok("threads.get", json!({ "id": t["id"] }));
    let att = full["messages"][0]["attachments"][0]["id"].clone();
    let d = synced.ok(
        "drafts.create",
        json!({ "to": ["ana@example.com"], "subject": "Fwd: Budget sheet", "body": "FYI",
                "attachments": [{ "from_attachment": att }] }),
    );
    synced.ok("drafts.send", json!({ "id": d["id"] }));
    assert_eq!(
        last_sent(&synced).attachment(0).unwrap().contents(),
        b"%PDF-1.4"
    );
}

#[rstest]
fn too_many_megabytes_are_refused_in_plain_words(synced: World) {
    let big = synced.home.path().join("video.mov");
    // Sparse: 26 MB by size, nothing written.
    std::fs::File::create(&big)
        .unwrap()
        .set_len(26 * 1024 * 1024)
        .unwrap();
    let err = synced
        .call(
            "drafts.create",
            json!({ "to": ["marc@example.com"], "body": "x", "attachments": [big] }),
        )
        .unwrap_err();
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .starts_with("These files add up to 26.0 MB, more than the 25.0 MB Gmail can send")
    );
}

#[rstest]
fn refused_attachment_leaves_no_draft(synced: World) {
    let _ = synced.call(
        "drafts.create",
        json!({ "to": ["marc@example.com"], "body": "x", "attachments": ["/nonexistent/plan.pdf"] }),
    );
    assert_eq!(synced.ok("drafts.list", json!({})), json!([]));
}

#[rstest]
fn detached_file_is_not_sent(synced: World) {
    let (pdf, png) = fixture_files(&synced);
    let d = synced.ok(
        "drafts.create",
        json!({ "to": ["marc@example.com"], "body": "x", "attachments": [pdf, png] }),
    );
    synced.ok(
        "drafts.detach",
        json!({ "id": d["id"], "attachment_id": d["attachments"][0]["id"] }),
    );
    synced.ok("drafts.send", json!({ "id": d["id"] }));
    assert_eq!(last_sent(&synced).attachment_count(), 1);
}

/// `mermoz draft reply --attach` twice.
fn cli_reply_with_two_files(w: &World) -> Value {
    let (pdf, png) = fixture_files(w);
    let t = w.thread_by_subject("Audit report v2");
    let out = w.cli(&[
        "--json",
        "draft",
        "reply",
        t["number"].as_str().unwrap(),
        "--body",
        "Files attached.",
        "--attach",
        pdf.to_str().unwrap(),
        "--attach",
        png.to_str().unwrap(),
    ]);
    serde_json::from_slice(&out.stdout).unwrap()
}

#[rstest]
fn cli_attach_stores_every_file(synced: World) {
    let d = cli_reply_with_two_files(&synced);
    assert_eq!(d["attachments"].as_array().unwrap().len(), 2);
}

#[rstest]
fn cli_attach_resolves_relative_paths(synced: World) {
    fixture_files(&synced);
    let out = Command::new(bin_dir().join("mermoz"))
        .args([
            "--json",
            "draft",
            "new",
            "--to",
            "marc@example.com",
            "--body",
            "x",
            "--attach",
            "chart.png",
        ])
        .current_dir(synced.home.path().join("to-attach"))
        .env("MERMOZ_HOME", synced.home.path())
        .output()
        .unwrap();
    let d: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(d["attachments"][0]["filename"], "chart.png");
}

#[rstest]
fn cli_attach_missing_file_fails(synced: World) {
    let out = synced.cli(&[
        "draft",
        "new",
        "--to",
        "marc@example.com",
        "--body",
        "x",
        "--attach",
        "/nonexistent/plan.pdf",
    ]);
    assert_ne!(out.status.code(), Some(0));
}

#[rstest]
fn approved_agent_draft_sends_its_files(synced: World) {
    let d = cli_reply_with_two_files(&synced);
    // The agent's send waits for approval (exit 3); the user approves in the app.
    synced.cli(&["send", d["id"].as_str().unwrap()]);
    synced.ok("review.approve", json!({ "id": d["id"] }));
    assert_eq!(last_sent(&synced).attachment_count(), 2);
}

#[rstest]
fn cli_draft_attach_adds_to_an_existing_draft(synced: World) {
    let (pdf, _) = fixture_files(&synced);
    let d = synced.ok(
        "drafts.create",
        json!({ "to": ["marc@example.com"], "body": "x" }),
    );
    let out = synced.cli(&[
        "--json",
        "draft",
        "attach",
        d["id"].as_str().unwrap(),
        pdf.to_str().unwrap(),
    ]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["attachments"][0]["mime"], "application/pdf");
}

#[rstest]
fn mcp_create_draft_takes_attachment_paths(synced: World) {
    let (pdf, _) = fixture_files(&synced);
    let mut child = Command::new(bin_dir().join("mermoz"))
        .arg("mcp")
        .env("MERMOZ_HOME", synced.home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "create_draft",
            "arguments": { "to": ["marc@example.com"], "body": "x", "attachments": [pdf] } } })
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
    let v: Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
    let draft: Value =
        serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(draft["attachments"][0]["filename"], "Relevé d'été.pdf");
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

#[rstest]
fn cli_reads_a_whole_conversation(nora: World) {
    let n = nora.thread_by_subject("Signing postponed")["number"]
        .as_str()
        .unwrap()
        .to_string();
    let out = nora.cli(&["thread", &n, "--conversation", "--json"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["messages"].as_array().unwrap().len(), 2);
}

#[rstest]
fn cli_lists_one_row_per_conversation(nora: World) {
    let out = nora.cli(&["list", "--conversations", "--json", "--limit", "100"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = v["threads"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["conversation_threads"] == 2)
        .count();
    assert_eq!(rows, 1);
}

#[rstest]
fn cli_splits_a_conversation(nora: World) {
    let n = nora.thread_by_subject("Signing postponed")["number"]
        .as_str()
        .unwrap()
        .to_string();
    nora.cli(&["conversation", "split", &n]);
    assert_ne!(
        conversation_of(&nora, "Signing postponed"),
        conversation_of(&nora, "Flat sale, next steps")
    );
}

#[rstest]
fn cli_lists_shared_files(synced: World) {
    let out = synced.cli(&["files", "report", "--json"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v[0]["filename"], "report.pdf");
}

#[rstest]
fn mcp_offers_conversation_tools(synced: World) {
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
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "list_files", "arguments": {} } })
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
        last["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("report.pdf")
    );
}

// ---- Feed (on the fictional demo mailbox) ------------------------------------------

/// A daemon serving the demo mailbox (`mermoz_core::demo`), offline.
#[fixture]
fn demo() -> World {
    let home = tempfile::tempdir().unwrap();
    {
        let store = mermoz_core::store::Store::open(
            &home.path().join("mermoz.sqlite"),
            &home.path().join("blobs"),
        )
        .unwrap();
        mermoz_core::demo::seed(&store).unwrap();
    }
    // MERMOZ_NO_SYNC serves the store only, as with demo data.
    launch(home, Default::default(), &[("MERMOZ_NO_SYNC", "1")])
}

impl World {
    fn feed(&self) -> Value {
        self.ok("feed.list", json!({}))
    }

    fn cards(&self) -> Vec<Value> {
        self.feed()["sections"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["cards"].as_array().unwrap().clone())
            .collect()
    }

    /// The card about the thread whose subject starts with `subject`.
    fn card(&self, subject: &str) -> Value {
        self.cards()
            .into_iter()
            .find(|c| {
                c["thread"]["subject"]
                    .as_str()
                    .is_some_and(|s| s.starts_with(subject))
            })
            .unwrap_or_else(|| panic!("no card for {subject}"))
    }

    fn section_of(&self, subject: &str) -> Value {
        self.card(subject)["section"].clone()
    }

    /// Sets the TypeSafe key and waits until every open thread is judged.
    fn judged(self) -> Self {
        self.ok("router.set", json!({ "api_key": "jev-test" }));
        self.wait_until("feed judgments", |w| {
            let f = w.feed();
            f["open"].as_u64() > Some(0) && f["judged"] == f["open"]
        });
        self
    }

    fn allow_ai_everywhere(&self) {
        let pr = self.ok(
            "providers.save",
            json!({ "name": "Personal AI", "kind": "anthropic", "model": "claude-opus-5-5",
            "base_url": format!("http://127.0.0.1:{}", self.port), "api_key": "sk-test" }),
        );
        for acc in self.ok("accounts.list", json!({})).as_array().unwrap() {
            self.ok(
                "accounts.agent.set",
                json!({ "account_id": acc["id"], "provider_ids": [pr["id"]] }),
            );
        }
    }
}

#[fixture]
fn judged_demo(demo: World) -> World {
    demo.judged()
}

#[rstest]
fn feed_shows_open_demo_mail_without_jev(demo: World) {
    assert!(demo.feed()["open"].as_u64().unwrap() > 5);
}

#[rstest]
fn senders_you_always_archive_are_cleared_together(demo: World) {
    let batch = demo
        .cards()
        .into_iter()
        .find(|c| c["id"] == "batch:cleanup")
        .unwrap();
    assert!(
        batch["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["subject"] == "Votre reçu Citymove")
    );
}

#[rstest]
fn three_archives_from_the_feed_join_the_batch(demo: World) {
    let id = demo.card("Commande 48211")["id"].clone();
    for _ in 0..3 {
        demo.ok("feed.learn_archive", json!({ "ids": [id] }));
    }
    let batch = demo
        .cards()
        .into_iter()
        .find(|c| c["id"] == "batch:cleanup")
        .unwrap();
    assert!(
        batch["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == id)
    );
}

#[rstest]
fn an_automated_sender_never_archived_is_its_own_card(demo: World) {
    assert_eq!(demo.card("Commande 48211")["kind"], "cleanup");
}

#[rstest]
fn feed_reports_sorting_while_jev_works(demo: World) {
    assert_eq!(demo.feed()["sorting"], false);
}

#[rstest]
fn a_choice_to_make_is_a_decision_card(judged_demo: World) {
    assert_eq!(judged_demo.card("Carrelage")["kind"], "decision");
}

#[rstest]
fn a_bill_shows_the_amount_written_in_the_mail(judged_demo: World) {
    assert_eq!(judged_demo.card("Facture cantine")["amount"], "126,00 €");
}

#[rstest]
fn a_bill_due_this_week_is_for_today(judged_demo: World) {
    assert_eq!(judged_demo.section_of("Facture cantine"), "today");
}

#[rstest]
fn backlog_with_a_close_date_comes_back(judged_demo: World) {
    assert_eq!(judged_demo.card("Réunion parents")["from_backlog"], true);
}

#[rstest]
fn backlog_card_says_why_it_came_back(judged_demo: World) {
    assert_eq!(
        judged_demo.card("Réunion parents")["reasons"][0],
        "It was in your Backlog, and its date is close."
    );
}

#[rstest]
fn backlog_event_is_now(judged_demo: World) {
    assert_eq!(judged_demo.section_of("Réunion parents"), "now");
}

#[rstest]
fn backlog_event_keeps_its_time(judged_demo: World) {
    assert_eq!(judged_demo.card("Réunion parents")["due_time"], "18:00");
}

#[rstest]
fn a_promise_you_made_is_a_card(judged_demo: World) {
    assert_eq!(judged_demo.card("Chiffres du T3")["kind"], "promise");
}

#[rstest]
fn an_overdue_wait_rises_to_today(judged_demo: World) {
    assert_eq!(judged_demo.section_of("Devis cuisine"), "today");
}

#[rstest]
fn newsletters_are_one_reading_card(judged_demo: World) {
    let reading = judged_demo
        .cards()
        .into_iter()
        .find(|c| c["id"] == "batch:reading")
        .unwrap();
    assert_eq!(reading["members"].as_array().unwrap().len(), 2);
}

#[rstest]
fn each_open_thread_is_judged_once(judged_demo: World) {
    let calls = judged_demo.mailbox.lock().unwrap().jev_feed_calls;
    judged_demo.ok("sync.now", json!({}));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(judged_demo.mailbox.lock().unwrap().jev_feed_calls, calls);
}

#[rstest]
fn feed_calls_count_towards_the_daily_cap(judged_demo: World) {
    assert!(
        judged_demo.ok("router.get", json!({}))["calls_today"]
            .as_i64()
            .unwrap()
            > 0
    );
}

#[rstest]
fn cards_want_words_once_an_ai_is_allowed(judged_demo: World) {
    judged_demo.allow_ai_everywhere();
    assert_eq!(judged_demo.card("Carrelage")["texts_wanted"], true);
}

#[rstest]
fn cards_want_no_words_without_an_ai(judged_demo: World) {
    assert_eq!(judged_demo.card("Carrelage")["texts_wanted"], false);
}

fn ask_words(w: &World, subject: &str) {
    let id = w.card(subject)["id"].clone();
    w.ok("feed.texts", json!({ "ids": [id] }));
    w.wait_until("card words", |w| w.card(subject)["texts"].is_object());
}

#[rstest]
fn the_ai_writes_the_title_of_a_visible_card(judged_demo: World) {
    judged_demo.allow_ai_everywhere();
    ask_words(&judged_demo, "Carrelage");
    assert_eq!(
        judged_demo.card("Carrelage")["title"],
        "Choose the kitchen tiles"
    );
}

#[rstest]
fn the_ai_offers_the_options_of_a_decision(judged_demo: World) {
    judged_demo.allow_ai_everywhere();
    ask_words(&judged_demo, "Carrelage");
    assert_eq!(
        judged_demo.card("Carrelage")["texts"]["options"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[rstest]
fn card_words_are_written_once(judged_demo: World) {
    judged_demo.allow_ai_everywhere();
    ask_words(&judged_demo, "Carrelage");
    let id = judged_demo.card("Carrelage")["id"].clone();
    judged_demo.ok("feed.texts", json!({ "ids": [id] }));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(judged_demo.mailbox.lock().unwrap().feed_text_calls, 1);
}

#[rstest]
fn only_requested_cards_get_words(judged_demo: World) {
    judged_demo.allow_ai_everywhere();
    ask_words(&judged_demo, "Carrelage");
    assert!(judged_demo.card("Facture cantine")["texts"].is_null());
}

#[rstest]
fn later_hides_a_card(demo: World) {
    let id = demo.card("Carrelage")["id"].clone();
    demo.ok(
        "feed.snooze",
        json!({ "ids": [id], "until": 4_000_000_000_000_i64 }),
    );
    assert!(
        !demo
            .cards()
            .iter()
            .any(|c| c["thread"]["subject"] == "Carrelage cuisine : ton choix avant vendredi")
    );
}

#[rstest]
fn undoing_later_brings_the_card_back(demo: World) {
    let id = demo.card("Carrelage")["id"].clone();
    demo.ok(
        "feed.snooze",
        json!({ "ids": [id.clone()], "until": 4_000_000_000_000_i64 }),
    );
    demo.ok("feed.unsnooze", json!({ "ids": [id] }));
    assert_eq!(demo.card("Carrelage")["kind"], "other");
}

#[rstest]
fn archiving_a_batch_empties_the_clean_up_card(demo: World) {
    let batch = demo
        .cards()
        .into_iter()
        .find(|c| c["id"] == "batch:cleanup")
        .unwrap();
    let ids: Vec<Value> = batch["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].clone())
        .collect();
    demo.ok("threads.update", json!({ "ids": ids, "status": "archive" }));
    assert!(!demo.cards().iter().any(|c| c["id"] == "batch:cleanup"));
}

#[rstest]
fn restoring_a_batch_brings_it_back(demo: World) {
    let batch = demo
        .cards()
        .into_iter()
        .find(|c| c["id"] == "batch:cleanup")
        .unwrap();
    let before: Vec<Value> = batch["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            json!({ "id": t["id"], "unread": t["unread"], "status": t["status"],
                    "priority": t["priority"], "projects": t["projects"], "due": t["due"] })
        })
        .collect();
    let ids: Vec<Value> = before.iter().map(|t| t["id"].clone()).collect();
    demo.ok("threads.update", json!({ "ids": ids, "status": "archive" }));
    demo.ok("threads.restore", json!({ "threads": before }));
    assert!(demo.cards().iter().any(|c| c["id"] == "batch:cleanup"));
}

#[rstest]
fn cli_feed_json_lists_sections(demo: World) {
    let out = demo.cli(&["--json", "feed"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!v["sections"].as_array().unwrap().is_empty());
}

#[rstest]
fn cli_later_hides_a_thread(demo: World) {
    let number = demo.card("Carrelage")["thread"]["number"]
        .as_str()
        .unwrap()
        .to_string();
    demo.cli(&["later", &number]);
    assert!(
        !demo
            .cards()
            .iter()
            .any(|c| c["thread"]["number"] == number.as_str())
    );
}

#[rstest]
fn mcp_lists_the_feed(demo: World) {
    let mut child = Command::new(bin_dir().join("mermoz"))
        .arg("mcp")
        .env("MERMOZ_HOME", demo.home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "feed", "arguments": {} } })).unwrap();
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
            .contains("\"sections\"")
    );
}

// ---- reading: instant original, inline images, source ----------------------

/// HTML mail with an image pasted in the body (`cid:`) and a PDF sent along.
fn raw_with_inline_image() -> String {
    "From: Nora Lind <nora@agency.example>\r\nTo: me@example.com\r\nSubject: Chart\r\nMessage-ID: <chart@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"M\"\r\n\r\n--M\r\nContent-Type: multipart/related; boundary=\"R\"\r\n\r\n--R\r\nContent-Type: text/html\r\n\r\n<p>See the chart:</p><img src=\"cid:ii_chart1\" alt=\"Graphique.png\">\r\n--R\r\nContent-Type: image/png\r\nContent-ID: <ii_chart1>\r\nContent-Disposition: inline; filename=\"Graphique.png\"\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--R--\r\n--M\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"notes.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQ=\r\n--M--\r\n".into()
}

fn with_chart(w: &World) -> Value {
    w.mailbox.lock().unwrap().add_message(
        "c1",
        "tc",
        &["INBOX"],
        &raw_with_inline_image(),
        NOW + 1000,
    );
    w.wait_until("the chart mail", |w| {
        w.ok("threads.list", json!({}))["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["subject"] == "Chart")
    });
    let id = w.thread_by_subject("Chart")["id"].clone();
    w.ok("threads.get", json!({ "id": id, "conversation": true }))
}

#[rstest]
fn pasted_images_show_in_the_body(synced: World) {
    let mid = with_chart(&synced)["messages"][0]["id"].clone();
    let v = synced.ok("messages.html", json!({ "id": mid }));
    assert!(
        v["html"]
            .as_str()
            .unwrap()
            .contains("src=\"data:image/png;base64,iVBORw0KGgo=\"")
    );
}

#[rstest]
fn pasted_images_are_not_remote_images(synced: World) {
    let mid = with_chart(&synced)["messages"][0]["id"].clone();
    let v = synced.ok("messages.html", json!({ "id": mid }));
    assert_eq!(v["remote_images"], false);
}

#[rstest]
fn a_pasted_image_is_marked_inline(synced: World) {
    let m = &with_chart(&synced)["messages"][0];
    let image = m["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["filename"] == "Graphique.png")
        .cloned()
        .unwrap();
    assert_eq!(image["inline"], true);
}

#[rstest]
fn a_file_sent_along_stays_an_attachment(synced: World) {
    let m = &with_chart(&synced)["messages"][0];
    let pdf = m["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["filename"] == "notes.pdf")
        .cloned()
        .unwrap();
    assert_eq!(pdf["inline"], false);
}

#[rstest]
fn opening_a_conversation_never_asks_the_ai(nora: World) {
    allow_ai(&nora);
    let id = nora.thread_by_subject("Signing postponed")["id"].clone();
    let v = nora.ok("threads.get", json!({ "id": id, "conversation": true }));
    for m in v["messages"].as_array().unwrap() {
        nora.ok("messages.html", json!({ "id": m["id"] }));
    }
    nora.ok("threads.mark_read", json!({ "id": id }));
    let m = nora.mailbox.lock().unwrap();
    assert_eq!(
        (
            m.llm_keys.len(),
            m.responses_auth.len(),
            m.chat_requests.len()
        ),
        (0, 0, 0)
    );
}

#[rstest]
fn the_source_is_the_email_as_received(synced: World) {
    let mid = with_chart(&synced)["messages"][0]["id"].clone();
    let v = synced.ok("messages.source", json!({ "id": mid }));
    assert!(
        v["source"]
            .as_str()
            .unwrap()
            .contains("Content-ID: <ii_chart1>")
    );
}

/// The demo feed with a TypeSafe key whose credits ran out.
#[fixture]
fn broke_demo(demo: World) -> World {
    demo.mailbox.lock().unwrap().jev_out_of_credits = true;
    demo.ok("router.set", json!({ "api_key": "jev-test" }));
    demo.wait_until("the AI hold", |w| w.feed()["paused"] == "credits");
    demo
}

#[rstest]
fn feed_says_sorting_is_paused_when_credits_ran_out(broke_demo: World) {
    assert_eq!(broke_demo.feed()["sorting"], false);
}

#[rstest]
fn feed_answers_at_once_when_credits_ran_out(broke_demo: World) {
    let t = Instant::now();
    broke_demo.feed();
    assert!(t.elapsed() < Duration::from_millis(150));
}

#[rstest]
fn feed_stops_asking_jev_once_credits_ran_out(broke_demo: World) {
    let before = broke_demo.mailbox.lock().unwrap().jev_auth.len();
    broke_demo.ok("sync.now", json!({}));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(broke_demo.mailbox.lock().unwrap().jev_auth.len(), before);
}

// ---- newsletters read from their content -----------------------------------------

/// A newsletter whose plain part is a dump opening on its "view in browser" link.
fn raw_newsletter() -> String {
    "From: Lettre du Quartier <news@quartier.example>\r\nTo: me@example.com\r\nSubject: La lettre d'octobre\r\nMessage-ID: <news@example.com>\r\nList-Id: Lettre <lettre.quartier.example>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"A\"\r\n\r\n--A\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nhttps://us.list-manage.com/AIW_vCP29mP?e=4f2a9c&c2id=1b2c3d4e5f60718293a4b5c6\r\n\r\n\r\n42% des habitants utilisent le velo.\r\n--A\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p><a href=\"https://us.list-manage.com/v?u=1\">Voir la version en ligne</a></p><h1>Le marche rouvre samedi</h1><p>42% des habitants utilisent le velo.</p><p>Se desabonner</p>\r\n--A--\r\n".into()
}

fn newsletter(w: &World) -> Value {
    w.mailbox
        .lock()
        .unwrap()
        .add_message("nl1", "tnl", &["INBOX"], &raw_newsletter(), NOW + 1000);
    w.wait_until("the newsletter", |w| {
        w.ok("threads.list", json!({}))["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["subject"] == "La lettre d'octobre")
    });
    let id = w.thread_by_subject("La lettre d'octobre")["id"].clone();
    w.ok("threads.get", json!({ "id": id }))
}

#[rstest]
fn a_newsletter_bubble_opens_on_its_headline(synced: World) {
    let m = &newsletter(&synced)["messages"][0];
    assert!(
        m["body_new"]
            .as_str()
            .unwrap()
            .starts_with("Le marche rouvre samedi")
    );
}

#[rstest]
fn a_newsletter_bubble_has_no_tracking_link(synced: World) {
    let m = &newsletter(&synced)["messages"][0];
    assert!(!m["body_new"].as_str().unwrap().contains("list-manage"));
}

#[rstest]
fn a_newsletter_preview_has_no_tracking_link(synced: World) {
    newsletter(&synced);
    let t = synced.thread_by_subject("La lettre d'octobre");
    assert!(!t["snippet"].as_str().unwrap().contains("list-manage"));
}

#[rstest]
fn the_whole_newsletter_keeps_its_original_text(synced: World) {
    let m = &newsletter(&synced)["messages"][0];
    assert!(m["body_text"].as_str().unwrap().contains("list-manage"));
}

// ---- Later -------------------------------------------------------------------------------

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Puts the decision card off until `in_ms` from now, as "Friday evening".
fn put_off(w: &World, in_ms: i64) -> Value {
    let id = w.card("Carrelage")["thread"]["id"].clone();
    w.ok(
        "feed.snooze",
        json!({ "ids": [id], "until": now_ms() + in_ms, "label": "Friday evening" }),
    );
    id
}

fn todo_ids(w: &World) -> Vec<Value> {
    w.ok(
        "threads.list",
        json!({ "status": ["unread", "todo"], "limit": 200 }),
    )["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].clone())
        .collect()
}

#[rstest]
fn a_put_off_thread_leaves_its_column(demo: World) {
    let id = put_off(&demo, 3_600_000);
    assert!(!todo_ids(&demo).contains(&id));
}

#[rstest]
fn a_put_off_thread_comes_back_at_its_time(demo: World) {
    let id = put_off(&demo, 1_500);
    demo.wait_until("the thread back", |w| todo_ids(w).contains(&id));
    assert_eq!(
        demo.ok("threads.get", json!({ "id": id }))["thread"]["status"],
        "unread"
    );
}

#[rstest]
fn a_returning_thread_is_first_in_the_feed(demo: World) {
    put_off(&demo, 1_000);
    demo.wait_until("the card back", |w| {
        w.cards()
            .iter()
            .any(|c| c["reasons"][0] == "You set this for Friday evening.")
    });
    assert_eq!(demo.card("Carrelage")["section"], "now");
}

#[rstest]
fn later_reads_what_the_user_types(demo: World) {
    assert!(
        demo.ok("later.parse", json!({ "text": "vendredi soir" }))["until"]
            .as_i64()
            .is_some()
    );
}

#[rstest]
fn later_offers_tomorrow_morning(demo: World) {
    let options = demo.ok("later.options", json!({}));
    assert!(
        options
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["label"] == "Tomorrow morning")
    );
}

#[rstest]
fn later_says_nothing_for_unclear_text(demo: World) {
    assert!(
        demo.ok("later.parse", json!({ "text": "bientôt peut-être" }))
            .is_null()
    );
}

#[rstest]
fn cli_later_reads_plain_words(demo: World) {
    let number = demo.card("Carrelage")["thread"]["number"]
        .as_str()
        .unwrap()
        .to_string();
    let out = demo.cli(&["later", &number, "vendredi", "soir"]);
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("later: back "));
}

/// The demo conversation with the most messages, whole: (thread id, its messages).
fn longest_conversation(w: &World) -> (String, Vec<Value>) {
    let list = w.ok(
        "threads.list",
        json!({ "limit": 200, "conversations": true }),
    );
    list["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let id = t["id"].as_str().unwrap().to_string();
            let d = w.ok("threads.get", json!({ "id": id, "conversation": true }));
            (id, d["messages"].as_array().unwrap().clone())
        })
        .max_by_key(|(_, m)| m.len())
        .unwrap()
}

fn window(w: &World, id: &str) -> Value {
    w.ok(
        "threads.get",
        json!({ "id": id, "conversation": true, "latest": 1 }),
    )
}

#[rstest]
fn a_window_sends_only_the_latest_messages(demo: World) {
    let (id, _) = longest_conversation(&demo);
    assert_eq!(window(&demo, &id)["messages"].as_array().unwrap().len(), 1);
}

#[rstest]
fn a_window_counts_what_it_left_out(demo: World) {
    let (id, all) = longest_conversation(&demo);
    assert_eq!(window(&demo, &id)["earlier"], json!(all.len() - 1));
}

#[rstest]
fn a_window_keeps_the_newest_message(demo: World) {
    let (id, all) = longest_conversation(&demo);
    assert_eq!(window(&demo, &id)["messages"][0], all[all.len() - 1]);
}

#[rstest]
fn a_window_keeps_every_file_and_link(demo: World) {
    let (id, _) = longest_conversation(&demo);
    let full = demo.ok("threads.get", json!({ "id": id, "conversation": true }));
    let w = window(&demo, &id);
    assert_eq!(
        (&w["conversation"]["files"], &w["conversation"]["links"]),
        (
            &full["conversation"]["files"],
            &full["conversation"]["links"]
        )
    );
}

/// Synced, plus a flight confirmation Gmail holds that sync never saw.
#[fixture]
fn with_unseen_mail(synced: World) -> World {
    synced.mailbox.lock().unwrap().add_unseen(
        "m9",
        "t9",
        &raw(
            "Frenchbee <noreply@frenchbee.example>",
            "Your flight to Papeete",
            "Booking confirmed",
            "m9",
        ),
        NOW - 5000,
    );
    synced
}

fn gmail_hits(w: &World, q: &str) -> Vec<Value> {
    w.ok("search.gmail", json!({ "query": q }))["results"]
        .as_array()
        .unwrap()
        .clone()
}

#[rstest]
fn local_search_misses_mail_it_does_not_hold(with_unseen_mail: World) {
    assert!(
        with_unseen_mail.ok("search", json!({ "query": "papeete" }))["threads"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[rstest]
fn gmail_search_finds_it(with_unseen_mail: World) {
    assert_eq!(
        gmail_hits(&with_unseen_mail, "papeete")[0]["subject"],
        "Your flight to Papeete"
    );
}

#[rstest]
fn gmail_search_says_it_is_not_in_mermoz(with_unseen_mail: World) {
    assert_eq!(
        gmail_hits(&with_unseen_mail, "papeete")[0]["in_mermoz"],
        false
    );
}

#[rstest]
fn gmail_search_knows_mail_already_here(with_unseen_mail: World) {
    assert_eq!(
        gmail_hits(&with_unseen_mail, "remediation")[0]["in_mermoz"],
        true
    );
}

#[rstest]
fn pulling_a_gmail_hit_makes_it_searchable(with_unseen_mail: World) {
    let hit = gmail_hits(&with_unseen_mail, "papeete")[0].clone();
    with_unseen_mail.ok(
        "search.gmail_pull",
        json!({ "account_id": hit["account_id"], "ids": [hit["id"]] }),
    );
    assert_eq!(
        with_unseen_mail.ok("search", json!({ "query": "papeete" }))["threads"][0]["subject"],
        "Your flight to Papeete"
    );
}

/// A town letter sent through Mailjet: a portrait placeholder, then lines made of
/// non-breaking spaces and invisible characters before the letter.
fn raw_town_letter() -> String {
    "From: Mairie de Villeneuve <lettre@villeneuve.example>\r\nTo: me@example.com\r\nSubject: La lettre du maire\r\nMessage-ID: <town@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n<<Camille Durand - Maire de Villeneuve>> [https://x1abc.mjt.lu/img/x1abc/b/portrait.png]\r\n\u{00A0}\r\n\u{200B}\u{200C}\r\n\u{2007}\r\nChers Habitants,\r\n\u{00A0}\r\nLe nouveau marche ouvre samedi.\r\n".into()
}

#[rstest]
fn a_town_letter_bubble_starts_with_words(synced: World) {
    synced.mailbox.lock().unwrap().add_message(
        "tl1",
        "ttl",
        &["INBOX"],
        &raw_town_letter(),
        NOW + 1000,
    );
    synced.wait_until("the town letter", |w| {
        w.ok("threads.list", json!({}))["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["subject"] == "La lettre du maire")
    });
    let id = synced.thread_by_subject("La lettre du maire")["id"].clone();
    let v = synced.ok("threads.get", json!({ "id": id }));
    assert!(
        v["messages"][0]["body_new"]
            .as_str()
            .unwrap()
            .starts_with("Camille Durand - Maire de Villeneuve\n\nChers Habitants,")
    );
}

// ---- suggestions with each provider, and when they are held ---------------

#[rstest]
#[case::typesafe("typesafe")]
#[case::subq("subq")]
fn suggestions_come_with_each_provider(synced: World, #[case] provider: &str) {
    if provider == "subq" {
        subq_router(&synced);
    } else {
        enable_router(&synced);
    }
    assert_eq!(
        suggested(&suggest_todo(&synced)),
        vec![synced.thread_by_subject("Budget sheet")["id"].clone()]
    );
}

#[rstest]
fn turned_off_sorting_is_said(synced: World) {
    synced.ok("router.set", json!({ "daily_cap": 0 }));
    enable_router(&synced);
    assert_eq!(suggest_todo(&synced)["paused"], "off");
}

#[rstest]
fn turned_off_sorting_suggests_nothing(synced: World) {
    synced.ok("router.set", json!({ "daily_cap": 0 }));
    enable_router(&synced);
    assert_eq!(suggest_todo(&synced)["suggestions"], json!([]));
}

#[rstest]
fn used_up_limit_is_said(synced: World) {
    synced.ok("router.set", json!({ "daily_cap": 1 }));
    more_from_jonas(&synced, 3);
    enable_router(&synced);
    suggest_todo(&synced);
    assert_eq!(suggest_todo(&synced)["paused"], "limit");
}
