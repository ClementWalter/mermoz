//! End-to-end "Ask Mermoz to do": a real mermozd and `mermoz` CLI against stub Gmail,
//! with the stub AI returning a plan. Covers the preview (counts, plain words, nothing
//! changed), the run and its Gmail write-back, one undo for the whole run (after a
//! restart too), the typed confirmation for delete, refused plans and no AI.

#[allow(dead_code)]
mod stub_gmail;

use rstest::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use stub_gmail::Shared;

const DAY: i64 = 86_400_000;
const SENTENCE: &str = "archive all messages past 5 months not in done";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn raw(from: &str, subject: &str, msg_id: &str) -> String {
    format!(
        "From: {from}\r\nTo: Me <me@example.com>\r\nSubject: {subject}\r\nMessage-ID: <{msg_id}@example.com>\r\n\r\nHello\r\n"
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

fn spawn_daemon(home: &std::path::Path, port: u16) -> Child {
    Command::new(env!("CARGO_BIN_EXE_mermozd"))
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
        self.daemon = spawn_daemon(self.home.path(), self.port);
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

    fn thread(&self, subject: &str) -> Value {
        let v = self.ok("threads.list", json!({ "limit": 2000 }));
        v["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["subject"] == subject)
            .cloned()
            .unwrap_or_else(|| panic!("no thread {subject}"))
    }

    fn status_of(&self, subject: &str) -> String {
        self.thread(subject)["status"].as_str().unwrap().to_string()
    }

    fn set_status(&self, subject: &str, status: &str) {
        let id = self.thread(subject)["id"].clone();
        self.ok("threads.update", json!({ "ids": [id], "status": status }));
    }

    /// The AI's next answers to "Ask Mermoz to do".
    fn ai_answers(&self, plans: Vec<Value>) {
        self.mailbox.lock().unwrap().do_plans = plans;
    }

    fn enable_ai(&self) {
        let pr = self.ok(
            "providers.save",
            json!({ "name": "Work AI", "kind": "anthropic", "model": "claude-opus-5-5",
                    "base_url": format!("http://127.0.0.1:{}", self.port), "api_key": "sk-test" }),
        );
        let acc = self.ok("accounts.list", json!({}))[0]["id"].clone();
        self.ok(
            "accounts.agent.set",
            json!({ "account_id": acc, "provider_ids": [pr["id"]], "digest_enabled": false }),
        );
    }
}

fn sign_in(w: &World) {
    let v = w.ok("accounts.add_gmail", json!({}));
    let url = v["auth_url"].as_str().unwrap().to_string();
    let q = url.split_once('?').map(|x| x.1).unwrap_or("");
    let param = |k: &str| {
        q.split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(key, _)| *key == k)
            .map(|(_, v)| v.to_string())
            .unwrap()
    };
    let redirect = param("redirect_uri")
        .replace("%3A", ":")
        .replace("%2F", "/");
    let hostport = redirect.trim_start_matches("http://").trim_end_matches('/');
    let mut s = std::net::TcpStream::connect(hostport).unwrap();
    write!(
        s,
        "GET /?code=stub-code&state={} HTTP/1.1\r\nHost: {hostport}\r\n\r\n",
        param("state")
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

/// Four fictional threads in the inbox, then filed so the plan has something to sort:
/// - "Old invoice" (250 days, Waiting) and "Old newsletter" (300 days, To do) match
/// - "Ferry tickets" (200 days, Done) is done, "Garden quote" (10 days) is recent
fn start_mail(extra: fn(&mut stub_gmail::Mailbox, i64), extra_threads: usize) -> World {
    let home = tempfile::tempdir().unwrap();
    let mailbox: Shared = Default::default();
    {
        let mut m = mailbox.lock().unwrap();
        m.email = "me@example.com".into();
        m.history_id = 100;
        let t = now();
        m.add_message(
            "m1",
            "t1",
            &["INBOX"],
            &raw("Billing <billing@shop.example>", "Old invoice", "m1"),
            t - 250 * DAY,
        );
        m.add_message(
            "m2",
            "t2",
            &["INBOX"],
            &raw("Weekly <digest@news.example>", "Old newsletter", "m2"),
            t - 300 * DAY,
        );
        m.add_message(
            "m3",
            "t3",
            &["INBOX"],
            &raw("Nora Lind <nora@lind.example>", "Ferry tickets", "m3"),
            t - 200 * DAY,
        );
        m.add_message(
            "m4",
            "t4",
            &["INBOX"],
            &raw("Paul Roux <paul@roux.example>", "Garden quote", "m4"),
            t - 10 * DAY,
        );
        extra(&mut m, t);
    }
    let port = stub_gmail::start(mailbox.clone());
    std::fs::write(
        home.path().join("oauth_client.json"),
        json!({ "client_id": "cid", "client_secret": "secret", "auth_url": format!("http://127.0.0.1:{port}/auth"), "token_url": format!("http://127.0.0.1:{port}/token") }).to_string(),
    )
    .unwrap();
    let daemon = spawn_daemon(home.path(), port);
    let w = World {
        home,
        mailbox,
        daemon,
        port,
    };
    w.wait_for_socket();
    sign_in(&w);
    w.wait_until("initial import", |w| {
        w.ok("threads.list", json!({}))["total"] == 4 + extra_threads
    });
    w.set_status("Old invoice", "waiting");
    w.set_status("Old newsletter", "todo");
    w.set_status("Ferry tickets", "done");
    w.set_status("Garden quote", "todo");
    w
}

/// A list address with no display name (3 threads) and look-alike notes (2), all recent.
fn add_lists(m: &mut stub_gmail::Mailbox, t: i64) {
    for i in 0..3 {
        m.add_message(
            &format!("c{i}"),
            &format!("tc{i}"),
            &["INBOX"],
            &raw(
                "esrin-gourmet-club@agency.example",
                &format!("Gourmet Club renewal {i}"),
                &format!("c{i}"),
            ),
            t - (5 + i) * DAY,
        );
    }
    for i in 0..2 {
        m.add_message(
            &format!("n{i}"),
            &format!("tn{i}"),
            &["INBOX"],
            &raw(
                "gemini-notes@meet.example",
                &format!("Notes: sync {i}"),
                &format!("n{i}"),
            ),
            t - (3 + i) * DAY,
        );
    }
}

#[fixture]
fn mail() -> World {
    start_mail(add_lists, 5)
}

/// The acceptance case's shape: one display name "Gemini" on two domains. 40 meeting
/// notes ("Notes: …") from gemini@google.example, and 1 unrelated thread from
/// gemini@icloud.example (a crypto exchange).
fn add_gemini(m: &mut stub_gmail::Mailbox, t: i64) {
    for i in 0..40 {
        m.add_message(
            &format!("g{i}"),
            &format!("tg{i}"),
            &["INBOX"],
            &raw(
                "Gemini <gemini@google.example>",
                &format!("Notes: “Q3-26 End of Quarter Showcase {i}” Oct 13, 2026"),
                &format!("g{i}"),
            ),
            t - (20 + i) * DAY,
        );
    }
    m.add_message(
        "gx",
        "tgx",
        &["INBOX"],
        &raw("Gemini <gemini@icloud.example>", "Your crypto weekly", "gx"),
        t - 2 * DAY,
    );
}

/// The base mailbox plus the Gemini mail, with an AI connected.
#[fixture]
fn gemini() -> World {
    let w = start_mail(add_gemini, 41);
    w.enable_ai();
    w
}

/// The user's example as the AI fills the schema.
fn archive_plan() -> Value {
    json!({ "understood": true, "question": null, "steps": [{
        "filter": { "status_in": [], "status_not_in": ["done"], "older_than": { "amount": 5, "unit": "month" },
                    "newer_than": null, "from": [], "projects": [], "not_projects": [], "unread": null,
                    "has_attachment": null, "account": null, "query": null },
        "action": { "kind": "set_status", "status": "archive", "project": null, "until": null } }] })
}

fn delete_plan() -> Value {
    json!({ "understood": true, "question": null, "steps": [{
        "filter": { "from": ["shop.example"] },
        "action": { "kind": "delete" } }] })
}

/// AI connected, answering with the archive plan.
#[fixture]
fn with_ai(mail: World) -> World {
    mail.enable_ai();
    mail.ai_answers(vec![archive_plan()]);
    mail
}

fn ask(w: &World) -> Value {
    w.ok("do.ask", json!({ "text": SENTENCE }))
}

// ---- preview

#[rstest]
fn preview_counts_the_matching_threads(with_ai: World) {
    assert_eq!(ask(&with_ai)["total"], 2);
}

#[rstest]
fn preview_counts_per_current_status(with_ai: World) {
    let counts: Vec<(String, i64)> = ask(&with_ai)["steps"][0]["by_status"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().to_string(),
                c["count"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        counts,
        vec![("Todo".to_string(), 1), ("Waiting".to_string(), 1)]
    );
}

#[rstest]
fn preview_says_it_in_plain_words(with_ai: World) {
    assert_eq!(
        ask(&with_ai)["summary"],
        "Archive 2 threads older than 5 months that aren't Done"
    );
}

#[rstest]
fn preview_shows_how_it_was_understood(with_ai: World) {
    assert_eq!(
        ask(&with_ai)["interpretation"],
        "status ≠ Done AND older than 5 months"
    );
}

#[rstest]
fn preview_samples_the_newest_first(with_ai: World) {
    assert_eq!(
        ask(&with_ai)["steps"][0]["sample"][0]["subject"],
        "Old invoice"
    );
}

#[rstest]
fn preview_changes_nothing(with_ai: World) {
    ask(&with_ai);
    assert_eq!(with_ai.status_of("Old invoice"), "waiting");
}

#[rstest]
fn preview_is_confirmed_with_enter(with_ai: World) {
    assert_eq!(ask(&with_ai)["confirm"], "enter");
}

#[rstest]
fn the_ai_gets_the_sentence_and_no_mail(with_ai: World) {
    // No content words here ("archive", "months", "done"…): no candidates are added.
    ask(&with_ai);
    let body = with_ai.mailbox.lock().unwrap().do_asks[0].clone();
    assert_eq!(
        body["messages"][0]["content"],
        format!("Instruction: {SENTENCE}")
    );
}

#[rstest]
fn nothing_matching_says_how_it_was_understood(with_ai: World) {
    with_ai.ai_answers(vec![json!({ "understood": true, "steps": [{
        "filter": { "from": ["nobody@nowhere.example"] }, "action": { "kind": "mark_read" } }] })]);
    assert_eq!(
        ask(&with_ai)["warning"],
        "Nothing matches. I understood: from nobody@nowhere.example."
    );
}

// ---- refused plans and no AI

#[rstest]
fn a_send_from_the_ai_is_never_planned(with_ai: World) {
    with_ai.ai_answers(vec![json!({ "understood": true, "steps": [{
        "filter": {}, "action": { "kind": "send_email" } }] })]);
    assert_eq!(ask(&with_ai)["kind"], "question");
}

#[rstest]
fn a_refused_plan_is_asked_again(with_ai: World) {
    with_ai.ai_answers(vec![
        json!({ "understood": true, "steps": [{ "filter": { "projects": ["Nowhere"] }, "action": { "kind": "mark_read" } }] }),
        archive_plan(),
    ]);
    assert_eq!(ask(&with_ai)["total"], 2);
}

#[rstest]
fn the_second_ask_says_why_the_first_was_refused(with_ai: World) {
    with_ai.ai_answers(vec![
        json!({ "understood": true, "steps": [{ "filter": { "projects": ["Nowhere"] }, "action": { "kind": "mark_read" } }] }),
        archive_plan(),
    ]);
    ask(&with_ai);
    let body = with_ai.mailbox.lock().unwrap().do_asks[1].clone();
    assert!(
        body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("There is no project called 'Nowhere'.")
    );
}

#[rstest]
fn without_ai_the_bar_explains(mail: World) {
    assert_eq!(mail.ok("do.status", json!({}))["available"], false);
}

#[rstest]
fn without_ai_asking_says_where_to_connect_one(mail: World) {
    assert!(
        mail.ok("do.ask", json!({ "text": SENTENCE }))["message"]
            .as_str()
            .unwrap()
            .contains("Settings › Agents")
    );
}

// ---- run and undo

fn statuses(w: &World) -> Vec<String> {
    [
        "Old invoice",
        "Old newsletter",
        "Ferry tickets",
        "Garden quote",
    ]
    .iter()
    .map(|s| w.status_of(s))
    .collect()
}

#[rstest]
fn running_archives_every_match_and_nothing_else(with_ai: World) {
    let id = ask(&with_ai)["id"].clone();
    with_ai.ok("do.run", json!({ "id": id }));
    assert_eq!(
        statuses(&with_ai),
        vec!["archive", "archive", "done", "todo"]
    );
}

#[rstest]
fn running_reports_its_progress(with_ai: World) {
    let id = ask(&with_ai)["id"].clone();
    with_ai.ok("do.run", json!({ "id": id }));
    assert_eq!(with_ai.ok("do.get", json!({ "id": id }))["done"], 2);
}

#[rstest]
fn running_archives_in_gmail(with_ai: World) {
    let id = ask(&with_ai)["id"].clone();
    with_ai.ok("do.run", json!({ "id": id }));
    with_ai.wait_until("inbox label removed", |w| {
        !w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t2")
            .contains("INBOX")
    });
}

#[rstest]
fn a_plan_runs_only_once(with_ai: World) {
    let id = ask(&with_ai)["id"].clone();
    with_ai.ok("do.run", json!({ "id": id }));
    assert!(with_ai.call("do.run", json!({ "id": id })).is_err());
}

#[rstest]
fn one_undo_puts_every_thread_back(with_ai: World) {
    let id = ask(&with_ai)["id"].clone();
    with_ai.ok("do.run", json!({ "id": id }));
    with_ai.ok("do.undo", json!({ "id": id }));
    assert_eq!(statuses(&with_ai), vec!["waiting", "todo", "done", "todo"]);
}

#[rstest]
fn undo_works_after_a_restart(with_ai: World) {
    let mut w = with_ai;
    let id = ask(&w)["id"].clone();
    w.ok("do.run", json!({ "id": id }));
    w.restart();
    w.ok("do.undo", json!({ "id": id }));
    assert_eq!(statuses(&w), vec!["waiting", "todo", "done", "todo"]);
}

#[rstest]
fn undo_brings_the_mail_back_to_the_gmail_inbox(with_ai: World) {
    let id = ask(&with_ai)["id"].clone();
    with_ai.ok("do.run", json!({ "id": id }));
    with_ai.wait_until("archived in gmail", |w| {
        !w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t2")
            .contains("INBOX")
    });
    with_ai.ok("do.undo", json!({ "id": id }));
    with_ai.wait_until("back in the gmail inbox", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t2")
            .contains("INBOX")
    });
}

#[rstest]
fn a_run_is_undone_only_once(with_ai: World) {
    let id = ask(&with_ai)["id"].clone();
    with_ai.ok("do.run", json!({ "id": id }));
    with_ai.ok("do.undo", json!({ "id": id }));
    assert!(with_ai.call("do.undo", json!({ "id": id })).is_err());
}

// ---- a big run's write-back

/// 1,500 old newsletters on top of the base mailbox: a run bigger than one Gmail batch.
fn add_crowd(m: &mut stub_gmail::Mailbox, t: i64) {
    for i in 0..1500 {
        m.add_message(
            &format!("b{i}"),
            &format!("tb{i}"),
            &["INBOX"],
            &raw(
                "Weekly <digest@bulk.example>",
                &format!("Digest {i}"),
                &format!("b{i}"),
            ),
            t - (200 + i as i64 % 50) * DAY,
        );
    }
}

/// The 1,500 newsletters and the archive plan, ready to run.
#[fixture]
fn crowd() -> World {
    let w = start_mail(add_crowd, 1500);
    w.enable_ai();
    w.ai_answers(vec![archive_plan()]);
    w
}

/// Runs the archive plan and waits until Gmail shows every match archived.
fn run_to_gmail(w: &World) {
    let id = ask(w)["id"].clone();
    w.ok("do.run", json!({ "id": id }));
    w.wait_until("every match archived in gmail", |w| {
        let m = w.mailbox.lock().unwrap();
        (0..1500).all(|i| !m.thread_label_names(&format!("tb{i}")).contains("INBOX"))
    });
}

#[rstest]
fn a_big_run_reaches_gmail_in_a_few_dozen_calls(crowd: World) {
    run_to_gmail(&crowd);
    // Batches go out as the run applies them (200 threads each), up to 1,000 messages
    // per call: far fewer calls than the 1,500 threads.
    assert!(crowd.mailbox.lock().unwrap().batch_modify_calls < 30);
}

#[rstest]
fn a_big_run_sends_no_call_per_thread(crowd: World) {
    let before = crowd.mailbox.lock().unwrap().thread_modify_calls;
    run_to_gmail(&crowd);
    assert_eq!(crowd.mailbox.lock().unwrap().thread_modify_calls, before);
}

#[rstest]
fn the_echo_of_a_big_run_fetches_no_message(crowd: World) {
    run_to_gmail(&crowd);
    // Two more rounds read the history the push left behind.
    for _ in 0..2 {
        crowd.ok("sync.now", json!({}));
        std::thread::sleep(Duration::from_millis(1200));
    }
    assert_eq!(crowd.mailbox.lock().unwrap().minimal_gets, 0);
}

#[rstest]
fn the_users_edit_reaches_gmail_before_the_rest_of_a_big_run(crowd: World) {
    crowd.mailbox.lock().unwrap().batch_modify_delay_ms = 300;
    let id = ask(&crowd)["id"].clone();
    crowd.ok("do.run", json!({ "id": id }));
    crowd.set_status("Garden quote", "done");
    run_to_gmail(&crowd);
    let m = crowd.mailbox.lock().unwrap();
    // Where each message's INBOX removal sits in Gmail's history.
    let removed_at = |msg: &str| {
        m.history.iter().position(|(_, r)| {
            r["labelsRemoved"]
                .as_array()
                .is_some_and(|a| a.iter().any(|e| e["message"]["id"] == msg))
        })
    };
    let last_bulk = (0..1500).filter_map(|i| removed_at(&format!("b{i}"))).max();
    assert!(removed_at("m4") < last_bulk);
}

// ---- a big delete's write-back

fn bulk_delete_plan() -> Value {
    json!({ "understood": true, "question": null, "steps": [{
        "filter": { "from": ["bulk.example"] },
        "action": { "kind": "delete" } }] })
}

/// The 1,500 newsletters and a plan moving them all to the Trash.
#[fixture]
fn crowd_to_delete() -> World {
    let w = start_mail(add_crowd, 1500);
    w.enable_ai();
    w.ai_answers(vec![bulk_delete_plan()]);
    w
}

/// Every one of the 1,500 newsletters is (or is not) in the Gmail Trash.
fn crowd_trashed(w: &World, trashed: bool) -> bool {
    let m = w.mailbox.lock().unwrap();
    (0..1500).all(|i| m.thread_label_names(&format!("tb{i}")).contains("TRASH") == trashed)
}

/// Runs the delete plan and waits until Gmail shows every match in the Trash; returns
/// the run's id for its undo.
fn delete_to_gmail(w: &World) -> Value {
    let id = ask(w)["id"].clone();
    w.ok("do.run", json!({ "id": id, "confirm": 1500 }));
    w.wait_until("every match in the gmail trash", |w| crowd_trashed(w, true));
    id
}

/// Deletes, then undoes, and waits until Gmail shows every match out of the Trash.
fn undo_to_gmail(w: &World) {
    let id = delete_to_gmail(w);
    w.ok("do.undo", json!({ "id": id }));
    w.wait_until("every match out of the gmail trash", |w| {
        crowd_trashed(w, false)
    });
}

/// Every thread's status, by id.
fn all_statuses(w: &World) -> std::collections::BTreeMap<String, String> {
    w.ok("threads.list", json!({ "limit": 5000 }))["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["id"].as_str().unwrap().to_string(),
                t["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[rstest]
#[case::r01(1)]
#[case::r02(2)]
#[case::r03(3)]
#[case::r04(4)]
#[case::r05(5)]
#[case::r06(6)]
#[case::r07(7)]
#[case::r08(8)]
#[case::r09(9)]
#[case::r10(10)]
// The newsletters share a sender: the grouping worker gathers them into one feed
// stream while the run goes. Ten rounds, as a join can land between any two writes.
fn undo_restores_every_thread_while_grouping_gathers_them(
    crowd_to_delete: World,
    #[case] _round: u32,
) {
    let w = crowd_to_delete;
    let before = all_statuses(&w);
    let id = ask(&w)["id"].clone();
    w.ok("do.run", json!({ "id": id, "confirm": 1500 }));
    w.ok("do.undo", json!({ "id": id }));
    let after = all_statuses(&w);
    let changed = before
        .iter()
        .filter(|(t, s)| after.get(*t) != Some(*s))
        .count();
    assert_eq!(changed, 0);
}

/// `messages.batchModify` calls that add or remove TRASH.
fn trash_batches(w: &World, added: bool) -> usize {
    let m = w.mailbox.lock().unwrap();
    m.batch_modify_deltas
        .iter()
        .filter(|(add, remove)| {
            let side = if added { add } else { remove };
            side.iter().any(|l| l == "TRASH")
        })
        .count()
}

#[rstest]
fn a_big_delete_reaches_the_trash_in_a_few_calls(crowd_to_delete: World) {
    delete_to_gmail(&crowd_to_delete);
    // 1,500 one-message threads, applied 200 at a time: a handful of calls.
    assert!(trash_batches(&crowd_to_delete, true) <= 10);
}

#[rstest]
fn a_big_delete_trashes_no_thread_on_its_own(crowd_to_delete: World) {
    delete_to_gmail(&crowd_to_delete);
    assert_eq!(
        crowd_to_delete.mailbox.lock().unwrap().thread_trash_calls,
        0
    );
}

#[rstest]
fn a_big_delete_leaves_the_gmail_inbox(crowd_to_delete: World) {
    delete_to_gmail(&crowd_to_delete);
    let m = crowd_to_delete.mailbox.lock().unwrap();
    assert!((0..1500).all(|i| !m.thread_label_names(&format!("tb{i}")).contains("INBOX")));
}

#[rstest]
fn undoing_a_big_delete_untrashes_in_a_few_calls(crowd_to_delete: World) {
    undo_to_gmail(&crowd_to_delete);
    assert!(trash_batches(&crowd_to_delete, false) <= 10);
}

#[rstest]
fn undoing_a_big_delete_untrashes_no_thread_on_its_own(crowd_to_delete: World) {
    undo_to_gmail(&crowd_to_delete);
    assert_eq!(
        crowd_to_delete.mailbox.lock().unwrap().thread_untrash_calls,
        0
    );
}

#[rstest]
fn undoing_a_big_delete_brings_the_mail_back_to_the_gmail_inbox(crowd_to_delete: World) {
    undo_to_gmail(&crowd_to_delete);
    let m = crowd_to_delete.mailbox.lock().unwrap();
    assert!((0..1500).all(|i| m.thread_label_names(&format!("tb{i}")).contains("INBOX")));
}

#[rstest]
fn the_echo_of_a_big_delete_fetches_no_message(crowd_to_delete: World) {
    delete_to_gmail(&crowd_to_delete);
    // Two more rounds read the history the push left behind.
    for _ in 0..2 {
        crowd_to_delete.ok("sync.now", json!({}));
        std::thread::sleep(Duration::from_millis(1200));
    }
    assert_eq!(crowd_to_delete.mailbox.lock().unwrap().minimal_gets, 0);
}

#[rstest]
fn the_users_delete_reaches_the_trash_before_the_rest_of_a_big_delete(crowd_to_delete: World) {
    crowd_to_delete
        .mailbox
        .lock()
        .unwrap()
        .batch_modify_delay_ms = 300;
    let id = ask(&crowd_to_delete)["id"].clone();
    crowd_to_delete.ok("do.run", json!({ "id": id, "confirm": 1500 }));
    crowd_to_delete.set_status("Garden quote", "deleted");
    crowd_to_delete.wait_until("every match in the gmail trash", |w| crowd_trashed(w, true));
    crowd_to_delete.wait_until("the user's delete in the gmail trash", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t4")
            .contains("TRASH")
    });
    let m = crowd_to_delete.mailbox.lock().unwrap();
    // Where each message's move to the Trash sits in Gmail's history.
    let trashed_at = |msg: &str| {
        m.history.iter().position(|(_, r)| {
            r["labelsAdded"].as_array().is_some_and(|a| {
                a.iter()
                    .any(|e| e["message"]["id"] == msg && e["labelIds"][0] == "TRASH")
            })
        })
    };
    let last_bulk = (0..1500).filter_map(|i| trashed_at(&format!("b{i}"))).max();
    assert!(trashed_at("m4") < last_bulk);
}

// ---- delete

#[rstest]
fn a_delete_asks_to_type_the_count(with_ai: World) {
    with_ai.ai_answers(vec![delete_plan()]);
    assert_eq!(ask(&with_ai)["confirm"], "type_count");
}

#[rstest]
fn a_delete_without_the_count_is_refused(with_ai: World) {
    with_ai.ai_answers(vec![delete_plan()]);
    let id = ask(&with_ai)["id"].clone();
    let err = with_ai.call("do.run", json!({ "id": id })).unwrap_err();
    assert_eq!(
        err["message"],
        "This moves 1 threads to the Trash. Type 1 to confirm."
    );
}

#[rstest]
fn a_refused_delete_changes_nothing(with_ai: World) {
    with_ai.ai_answers(vec![delete_plan()]);
    let id = ask(&with_ai)["id"].clone();
    let _ = with_ai.call("do.run", json!({ "id": id, "confirm": 7 }));
    assert_eq!(with_ai.status_of("Old invoice"), "waiting");
}

#[rstest]
fn a_delete_with_the_count_goes_to_the_gmail_trash(with_ai: World) {
    with_ai.ai_answers(vec![delete_plan()]);
    let id = ask(&with_ai)["id"].clone();
    with_ai.ok("do.run", json!({ "id": id, "confirm": 1 }));
    with_ai.wait_until("trash", |w| {
        w.mailbox
            .lock()
            .unwrap()
            .thread_label_names("t1")
            .contains("TRASH")
    });
}

// ---- other actions

#[rstest]
fn later_and_its_undo(with_ai: World) {
    let until = day_ahead(3);
    with_ai.ai_answers(vec![json!({ "understood": true, "steps": [{
        "filter": { "from": ["paul@roux.example"] }, "action": { "kind": "later", "until": until } }] })]);
    let id = ask(&with_ai)["id"].clone();
    with_ai.ok("do.run", json!({ "id": id }));
    with_ai.ok("do.undo", json!({ "id": id }));
    let todo = with_ai.ok("threads.list", json!({ "status": ["todo"] }));
    assert!(
        todo["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["subject"] == "Garden quote")
    );
}

/// A date `days` ahead as YYYY-MM-DD, worked out from the Unix clock (UTC).
fn day_ahead(days: i64) -> String {
    let d = now() / DAY + days;
    // Civil-from-days (Howard Hinnant), so the test needs no date crate.
    let z = d + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[rstest]
fn adding_a_project_files_every_match(with_ai: World) {
    with_ai.ai_answers(vec![json!({ "understood": true, "steps": [{
        "filter": { "older_than": { "amount": 5, "unit": "month" } }, "action": { "kind": "add_project", "project": "Old" } }] })]);
    let id = ask(&with_ai)["id"].clone();
    with_ai.ok("do.run", json!({ "id": id }));
    assert_eq!(with_ai.thread("Ferry tickets")["projects"], json!(["Old"]));
}

// ---- CLI and MCP

#[rstest]
fn the_cli_is_a_dry_run_by_default(with_ai: World) {
    let out = with_ai.cli(&["do", SENTENCE]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("dry run: nothing changed"));
}

#[rstest]
fn the_cli_dry_run_changes_nothing(with_ai: World) {
    with_ai.cli(&["do", SENTENCE]);
    assert_eq!(with_ai.status_of("Old newsletter"), "todo");
}

#[rstest]
fn the_cli_prints_the_plan_and_count(with_ai: World) {
    let out = with_ai.cli(&["do", SENTENCE]);
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .contains("Archive 2 threads older than 5 months that aren't Done")
    );
}

#[rstest]
fn the_cli_runs_with_yes(with_ai: World) {
    with_ai.cli(&["do", SENTENCE, "--yes"]);
    assert_eq!(with_ai.status_of("Old newsletter"), "archive");
}

#[rstest]
fn the_cli_delete_needs_the_count(with_ai: World) {
    with_ai.ai_answers(vec![delete_plan()]);
    let out = with_ai.cli(&["do", "delete the shop mail", "--yes"]);
    // Exit 2: a usage problem, the --confirm count is missing.
    assert_eq!(out.status.code(), Some(2));
}

#[rstest]
fn the_cli_deletes_with_the_count(with_ai: World) {
    with_ai.ai_answers(vec![delete_plan()]);
    with_ai.cli(&["do", "delete the shop mail", "--yes", "--confirm", "1"]);
    assert_eq!(with_ai.status_of("Old invoice"), "deleted");
}

#[rstest]
fn the_cli_takes_a_plan_without_the_ai(mail: World) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.json");
    std::fs::write(&path, archive_plan().to_string()).unwrap();
    let out = mail.cli(&["do", "--plan", path.to_str().unwrap(), "--json"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["total"], 2);
}

/// One MCP tool call through `mermoz mcp`; returns the tool result.
fn mcp(w: &World, name: &str, args: Value) -> Value {
    let mut child = Command::new(bin_dir().join("mermoz"))
        .arg("mcp")
        .env("MERMOZ_HOME", w.home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
    writeln!(child.stdin.take().unwrap(), "{req}").unwrap();
    let out = child.wait_with_output().unwrap();
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap()
        .to_string();
    serde_json::from_str::<Value>(&line).unwrap()["result"].clone()
}

#[rstest]
fn an_agent_gets_the_plan_without_changes(with_ai: World) {
    mcp(
        &with_ai,
        "plan_bulk_action",
        json!({ "sentence": SENTENCE }),
    );
    assert_eq!(with_ai.status_of("Old newsletter"), "todo");
}

#[rstest]
fn an_agent_must_confirm_with_the_count(with_ai: World) {
    let r = mcp(
        &with_ai,
        "plan_bulk_action",
        json!({ "sentence": SENTENCE }),
    );
    let plan: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    let run = mcp(
        &with_ai,
        "run_bulk_action",
        json!({ "id": plan["id"], "confirm": 99 }),
    );
    assert_eq!(run["isError"], true);
}

// ---- senders by their words, candidates, questions and picks

const CLUB: &str = "esrin-gourmet-club@agency.example";
const NOTES: &str = "gemini-notes@meet.example";
const VAGUE: &str = "delete all the messages like notes from gemini";

fn delete_from(words: &str) -> Value {
    json!({ "understood": true, "steps": [{ "filter": { "from": [words] }, "action": { "kind": "delete" } }] })
}

fn asks_back(q: &str) -> Value {
    json!({ "understood": false, "question": q, "steps": [] })
}

/// The text the stub AI received on its `n`th "Ask Mermoz to do" call.
fn ai_saw(w: &World, n: usize) -> String {
    w.mailbox.lock().unwrap().do_asks[n]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string()
}

#[rstest]
fn a_sender_phrase_finds_an_address_with_no_name(with_ai: World) {
    with_ai.ai_answers(vec![delete_from("esrin gourmet club")]);
    assert_eq!(ask(&with_ai)["total"], 3);
}

#[rstest]
fn the_preview_names_the_resolved_sender(with_ai: World) {
    with_ai.ai_answers(vec![delete_from("esrin gourmet club")]);
    assert_eq!(
        ask(&with_ai)["steps"][0]["resolved"],
        format!("from {CLUB} (3)")
    );
}

#[rstest]
fn nothing_matching_offers_the_closest_sender(with_ai: World) {
    with_ai.ai_answers(vec![delete_from("gourmet society")]);
    assert_eq!(ask(&with_ai)["choices"][0]["pick"], json!({ "from": CLUB }));
}

#[rstest]
fn picking_a_suggestion_replans_with_that_sender(with_ai: World) {
    with_ai.ai_answers(vec![delete_from("gourmet society")]);
    let id = ask(&with_ai)["id"].clone();
    let again = with_ai.ok("do.replan", json!({ "id": id, "pick": { "from": CLUB } }));
    assert_eq!(again["total"], 3);
}

#[rstest]
fn the_ai_is_given_the_mailbox_candidates(with_ai: World) {
    with_ai.ai_answers(vec![delete_from(NOTES)]);
    with_ai.ok("do.ask", json!({ "text": VAGUE }));
    assert!(ai_saw(&with_ai, 0).contains(&format!("- sender {NOTES} · 2 threads")));
}

#[rstest]
fn the_ai_is_given_subject_beginnings(with_ai: World) {
    with_ai.ai_answers(vec![delete_from(NOTES)]);
    with_ai.ok("do.ask", json!({ "text": VAGUE }));
    assert!(ai_saw(&with_ai, 0).contains("- subject starts with \"Notes:\" · 2 threads"));
}

#[rstest]
fn a_vague_sentence_resolves_through_the_candidates(with_ai: World) {
    with_ai.ai_answers(vec![delete_from(NOTES)]);
    assert_eq!(with_ai.ok("do.ask", json!({ "text": VAGUE }))["total"], 2);
}

#[rstest]
fn the_ai_never_sees_bodies(with_ai: World) {
    with_ai.ai_answers(vec![delete_from(NOTES)]);
    with_ai.ok("do.ask", json!({ "text": VAGUE }));
    assert!(!ai_saw(&with_ai, 0).contains("Hello"));
}

#[rstest]
fn a_question_offers_the_candidates(with_ai: World) {
    with_ai.ai_answers(vec![asks_back("Which notes?")]);
    let v = with_ai.ok("do.ask", json!({ "text": VAGUE }));
    assert_eq!(v["choices"][0]["pick"], json!({ "from": NOTES }));
}

#[rstest]
fn a_question_can_be_answered(with_ai: World) {
    with_ai.ai_answers(vec![asks_back("Which notes?")]);
    let v = with_ai.ok("do.ask", json!({ "text": VAGUE }));
    assert_eq!(v["can_answer"], true);
}

#[rstest]
fn the_answer_goes_back_to_the_ai(with_ai: World) {
    with_ai.ai_answers(vec![asks_back("Which notes?"), delete_from(NOTES)]);
    with_ai.ok("do.ask", json!({ "text": VAGUE }));
    with_ai.ok(
        "do.ask",
        json!({ "text": VAGUE, "rounds": [{ "question": "Which notes?", "answer": "the meeting notes" }] }),
    );
    assert!(
        ai_saw(&with_ai, 1)
            .contains("You asked: Which notes?\nThe user answered: the meeting notes")
    );
}

#[rstest]
fn an_answered_question_ends_in_a_plan(with_ai: World) {
    with_ai.ai_answers(vec![asks_back("Which notes?"), delete_from(NOTES)]);
    with_ai.ok("do.ask", json!({ "text": VAGUE }));
    let v = with_ai.ok(
        "do.ask",
        json!({ "text": VAGUE, "rounds": [{ "question": "Which notes?", "answer": "the meeting notes" }] }),
    );
    assert_eq!(v["total"], 2);
}

#[rstest]
fn after_two_rounds_no_more_answers_are_taken(with_ai: World) {
    with_ai.ai_answers(vec![asks_back("Which notes?")]);
    let rounds = json!([{ "question": "a", "answer": "b" }, { "question": "c", "answer": "d" }]);
    let v = with_ai.ok("do.ask", json!({ "text": VAGUE, "rounds": rounds }));
    assert_eq!(v["can_answer"], false);
}

#[rstest]
fn picking_a_candidate_plans_with_that_filter_even_if_the_ai_still_asks(with_ai: World) {
    with_ai.ai_answers(vec![asks_back("Which notes?")]);
    let v = with_ai.ok(
        "do.ask",
        json!({ "text": VAGUE, "pick": { "subject_prefix": "Notes:" } }),
    );
    assert_eq!(
        (v["total"].clone(), v["deletes"].clone()),
        (json!(2), json!(true))
    );
}

#[rstest]
fn a_pick_replaces_the_filter_the_ai_chose(with_ai: World) {
    with_ai.ai_answers(vec![delete_from("paul@roux.example")]);
    let v = with_ai.ok(
        "do.ask",
        json!({ "text": VAGUE, "pick": { "from": NOTES } }),
    );
    assert_eq!(v["interpretation"], format!("from {NOTES}"));
}

// ---- acceptance: "delete all message from gemeni notes from google meet"

const TYPO: &str = "delete all message from gemeni notes from google meet";
const GEMINI: &str = "gemini@google.example";

/// What a grounded AI answers: Gemini's Google address, subjects starting "Notes:".
fn gemini_notes_plan() -> Value {
    json!({ "understood": true, "steps": [{
        "filter": { "from": [GEMINI], "subject_prefix": "Notes:" }, "action": { "kind": "delete" } }] })
}

#[rstest]
fn the_ai_sees_gemini_with_its_notes_before_planning(gemini: World) {
    gemini.ai_answers(vec![gemini_notes_plan()]);
    gemini.ok("do.ask", json!({ "text": TYPO }));
    assert!(ai_saw(&gemini, 0).contains(&format!(
        "- sender Gemini <{GEMINI}> · 40 threads · subjects \"Notes:\""
    )));
}

#[rstest]
fn the_google_gemini_comes_before_the_other_one(gemini: World) {
    gemini.ai_answers(vec![gemini_notes_plan()]);
    gemini.ok("do.ask", json!({ "text": TYPO }));
    let seen = ai_saw(&gemini, 0);
    let first_sender = seen.lines().find(|l| l.starts_with("- sender ")).unwrap();
    assert!(first_sender.contains(GEMINI));
}

#[rstest]
fn a_description_put_in_the_sender_field_is_asked_again(gemini: World) {
    gemini.ai_answers(vec![
        delete_from("gemini notes from google meet"),
        gemini_notes_plan(),
    ]);
    gemini.ok("do.ask", json!({ "text": TYPO }));
    assert!(ai_saw(&gemini, 1).contains("'gemini notes from google meet' is not a sender"));
}

#[rstest]
fn the_grounded_plan_deletes_the_40_notes(gemini: World) {
    gemini.ai_answers(vec![gemini_notes_plan()]);
    assert_eq!(gemini.ok("do.ask", json!({ "text": TYPO }))["total"], 40);
}

#[rstest]
fn the_preview_names_gemini_as_the_user_knows_it(gemini: World) {
    gemini.ai_answers(vec![gemini_notes_plan()]);
    assert_eq!(
        gemini.ok("do.ask", json!({ "text": TYPO }))["summary"],
        format!(
            "Delete 40 threads from Gemini <{GEMINI}> whose subject starts with “Notes:” (to Gmail Trash)"
        )
    );
}

#[rstest]
fn the_typo_still_finds_gemini_and_the_subject_keeps_the_other_one_out(gemini: World) {
    gemini.ai_answers(vec![json!({ "understood": true, "steps": [{
        "filter": { "from": ["gemeni"], "subject_prefix": "Notes:" }, "action": { "kind": "delete" } }] })]);
    assert_eq!(gemini.ok("do.ask", json!({ "text": TYPO }))["total"], 40);
}

#[rstest]
fn running_it_leaves_the_icloud_gemini_alone(gemini: World) {
    gemini.ai_answers(vec![gemini_notes_plan()]);
    let id = gemini.ok("do.ask", json!({ "text": TYPO }))["id"].clone();
    gemini.ok("do.run", json!({ "id": id, "confirm": 40 }));
    assert_eq!(gemini.status_of("Your crypto weekly"), "todo");
}

#[rstest]
fn a_question_offers_the_google_gemini_first(gemini: World) {
    gemini.ai_answers(vec![asks_back("Which Gemini?")]);
    let v = gemini.ok("do.ask", json!({ "text": TYPO }));
    assert_eq!(v["choices"][0]["pick"], json!({ "from": GEMINI }));
}

// ---- reviewing the list: every thread, and unticked ones left alone

/// The club delete planned; returns its id and the full list.
fn club_delete(w: &World) -> (Value, Vec<Value>) {
    w.ai_answers(vec![delete_from("esrin gourmet club")]);
    let id = ask(w)["id"].clone();
    let list = w.ok("do.targets", json!({ "id": id }))["threads"]
        .as_array()
        .unwrap()
        .clone();
    (id, list)
}

#[rstest]
fn the_full_list_holds_every_thread(with_ai: World) {
    assert_eq!(club_delete(&with_ai).1.len(), 3);
}

#[rstest]
fn the_full_list_says_each_current_status(with_ai: World) {
    let (_, list) = club_delete(&with_ai);
    assert!(
        list.iter()
            .all(|t| t["status"].is_string() && t["last_date"].is_i64())
    );
}

#[rstest]
fn an_unticked_thread_changes_the_count_to_type(with_ai: World) {
    let (id, list) = club_delete(&with_ai);
    let err = with_ai
        .call(
            "do.run",
            json!({ "id": id, "confirm": 3, "exclude": [list[0]["id"]] }),
        )
        .unwrap_err();
    assert_eq!(
        err["message"],
        "This moves 2 threads to the Trash. Type 2 to confirm."
    );
}

#[rstest]
fn the_run_changes_exactly_the_ticked_threads(with_ai: World) {
    let (id, list) = club_delete(&with_ai);
    with_ai.ok(
        "do.run",
        json!({ "id": id, "confirm": 2, "exclude": [list[0]["id"]] }),
    );
    let statuses: Vec<String> = list
        .iter()
        .map(|t| with_ai.status_of(t["subject"].as_str().unwrap()))
        .collect();
    assert_eq!(statuses[1..], ["deleted", "deleted"]);
}

#[rstest]
fn an_unticked_thread_is_left_alone(with_ai: World) {
    let (id, list) = club_delete(&with_ai);
    with_ai.ok(
        "do.run",
        json!({ "id": id, "confirm": 2, "exclude": [list[0]["id"]] }),
    );
    assert_ne!(
        with_ai.status_of(list[0]["subject"].as_str().unwrap()),
        "deleted"
    );
}

#[rstest]
fn nothing_ticked_runs_nothing(with_ai: World) {
    let (id, list) = club_delete(&with_ai);
    let all: Vec<Value> = list.iter().map(|t| t["id"].clone()).collect();
    assert!(
        with_ai
            .call("do.run", json!({ "id": id, "exclude": all }))
            .is_err()
    );
}
