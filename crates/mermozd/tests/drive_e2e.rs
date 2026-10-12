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
    let trusted_exe = if trusted {
        std::env::current_exe().unwrap()
    } else {
        "/nonexistent/Mermoz".into()
    };
    let daemon = Command::new(env!("CARGO_BIN_EXE_mermozd"))
        .env("MERMOZ_HOME", home.path())
        .env("MERMOZ_TOKEN_STORE", "file")
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
        .unwrap();
    let w = World {
        home,
        mailbox,
        drive,
        daemon,
        port,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(w.home.path().join("mermozd.sock")).is_err() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    sign_in(&w);
    w.wait_until("initial import", |w| {
        w.ok("threads.list", json!({}))["total"] == 3
    });
    w
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
