//! An Outlook account's sync against the stub Graph: import, delta changes, echoes,
//! write-back and send.

use super::*;
use crate::model::{Draft, ThreadSummary};
use crate::oauth::{ClientConfig, TokenSource};
use crate::outlook::stub::{Graph, Shared};
use crate::store::{Store, ThreadFilter, ThreadPatch};
use rstest::{fixture, rstest};
use tokio::sync::broadcast;

const ME: &str = "me@outlook.example";

fn mime(from: &str, subject: &str, id: &str, extra: &str) -> String {
    format!(
        "From: {from}\r\nTo: {ME}\r\nSubject: {subject}\r\nMessage-ID: <{id}@example.com>\r\n{extra}Date: Wed, 21 Oct 2026 08:00:00 +0000\r\n\r\nAbout {subject}\r\n"
    )
}

struct Env {
    _dir: tempfile::TempDir,
    graph: Shared,
    sync: AccountSync,
}

impl Env {
    fn store(&self) -> &Store {
        &self.sync.store
    }

    fn thread(&self, subject: &str) -> ThreadSummary {
        self.store()
            .threads(&ThreadFilter {
                limit: 100,
                ..Default::default()
            })
            .unwrap()
            .0
            .into_iter()
            .find(|t| t.subject == subject)
            .unwrap_or_else(|| panic!("no thread {subject}"))
    }

    fn subjects(&self) -> BTreeSet<String> {
        self.store()
            .threads(&ThreadFilter {
                limit: 100,
                ..Default::default()
            })
            .unwrap()
            .0
            .into_iter()
            .map(|t| t.subject)
            .collect()
    }

    fn graph(&self) -> std::sync::MutexGuard<'_, Graph> {
        self.graph.lock().unwrap()
    }
}

/// A mailbox with mail in the Inbox (a conversation with a sent reply), Archive, a
/// "Clients" folder, Junk and Deleted Items, and the account syncing it.
#[fixture]
fn env() -> Env {
    let now = chrono::Utc::now().timestamp_millis();
    let mut g = Graph::new(ME);
    g.add_folder("f-clients", "Clients", "root");
    g.add_message(
        "i1",
        "c1",
        "f-inbox",
        false,
        &mime("Ana <ana@example.com>", "Lunch", "i1", ""),
        now - 60_000,
    );
    g.add_message(
        "s1",
        "c1",
        "f-sent",
        true,
        &mime(ME, "Re: Lunch", "s1", "In-Reply-To: <i1@example.com>\r\n"),
        now - 30_000,
    );
    g.add_message(
        "a1",
        "c2",
        "f-archive",
        true,
        &mime("Bank <no-reply@bank.example>", "Invoice", "a1", ""),
        now - 2_000,
    );
    g.add_message(
        "u1",
        "c3",
        "f-clients",
        true,
        &mime("Bo <bo@example.com>", "Contract", "u1", ""),
        now - 3_000,
    );
    g.add_message(
        "j1",
        "c4",
        "f-junk",
        false,
        &mime("x@spam.example", "Winner", "j1", ""),
        now - 4_000,
    );
    g.add_message(
        "d1",
        "c5",
        "f-deleted",
        true,
        &mime("Cy <cy@example.com>", "Old", "d1", ""),
        now - 5_000,
    );
    let graph: Shared = Arc::new(std::sync::Mutex::new(g));
    let port = crate::outlook::stub::start(graph.clone());
    let dir = tempfile::tempdir().unwrap();
    let store =
        Arc::new(Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap());
    let acc = store.add_account(ME, "outlook").unwrap().id;
    let mut cfg = ClientConfig::microsoft("ms-cid".into());
    cfg.token_url = format!("http://127.0.0.1:{port}/ms/token");
    let client = Outlook::new(
        format!("http://127.0.0.1:{port}/graph"),
        Arc::new(TokenSource::new(cfg, "rt".into())),
    );
    let (events, _) = broadcast::channel(64);
    let sync = AccountSync {
        store,
        backend: crate::provider::Backend::Outlook(Arc::new(client)),
        account_id: acc,
        email: ME.into(),
        events,
        read_only: false,
    };
    Env {
        _dir: dir,
        graph,
        sync,
    }
}

/// The account once imported.
#[fixture]
async fn imported(env: Env) -> Env {
    env.sync.initial_sync().await.unwrap();
    env
}

#[rstest]
#[tokio::test]
async fn import_brings_the_imported_folders_only(#[future] imported: Env) {
    let e = imported.await;
    assert_eq!(
        e.subjects(),
        ["Lunch", "Invoice", "Contract"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
}

#[rstest]
#[tokio::test]
async fn a_conversation_is_one_thread(#[future] imported: Env) {
    assert_eq!(imported.await.thread("Lunch").message_count, 2);
}

#[rstest]
#[case::unread_inbox("Lunch", "unread")]
#[case::archived("Invoice", "archive")]
#[case::filed_in_a_folder("Contract", "archive")]
#[tokio::test]
async fn imported_threads_get_their_status(
    #[future] imported: Env,
    #[case] subject: &str,
    #[case] status: &str,
) {
    assert_eq!(imported.await.thread(subject).status, status);
}

#[rstest]
#[tokio::test]
async fn a_folder_is_a_project(#[future] imported: Env) {
    assert_eq!(imported.await.thread("Contract").projects, vec!["Clients"]);
}

#[rstest]
#[tokio::test]
async fn import_leaves_a_delta_link_per_imported_folder(#[future] imported: Env) {
    let e = imported.await;
    assert_eq!(
        e.store()
            .delta_links(&e.sync.account_id)
            .unwrap()
            .into_keys()
            .collect::<Vec<_>>(),
        vec!["f-archive", "f-clients", "f-inbox", "f-sent"]
    );
}

#[rstest]
#[tokio::test]
async fn import_ends_idle(#[future] imported: Env) {
    let e = imported.await;
    assert_eq!(
        e.store().account(&e.sync.account_id).unwrap().sync.state,
        "idle"
    );
}

#[rstest]
#[tokio::test]
async fn import_survives_throttling(env: Env) {
    env.graph().throttle = 3;
    env.sync.initial_sync().await.unwrap();
    assert_eq!(env.subjects().len(), 3);
}

#[rstest]
#[tokio::test]
async fn an_interrupted_import_resumes_from_its_cursor(env: Env) {
    // The listing stopped after its first page (two messages): its next link is saved.
    let first = match &env.sync.backend {
        crate::provider::Backend::Outlook(o) => o.messages_page(None).await.unwrap(),
        _ => unreachable!(),
    };
    env.store()
        .set_cursor(&env.sync.account_id, first.next_link.as_deref())
        .unwrap();
    env.sync.initial_sync().await.unwrap();
    assert_eq!(env.subjects().len(), 3);
}

#[rstest]
#[tokio::test]
async fn new_mail_arrives_through_the_delta(#[future] imported: Env) {
    let e = imported.await;
    let now = chrono::Utc::now().timestamp_millis();
    e.graph().add_message(
        "i2",
        "c9",
        "f-inbox",
        false,
        &mime("Di <di@example.com>", "Trip", "i2", ""),
        now,
    );
    e.sync.incremental().await.unwrap();
    assert_eq!(e.thread("Trip").status, "unread");
}

#[rstest]
#[tokio::test]
async fn read_elsewhere_reads_the_thread(#[future] imported: Env) {
    let e = imported.await;
    e.graph().edit("i1", |m| m.is_read = true);
    e.sync.incremental().await.unwrap();
    assert!(!e.thread("Lunch").unread);
}

#[rstest]
#[tokio::test]
async fn deleted_elsewhere_is_deleted(#[future] imported: Env) {
    let e = imported.await;
    e.graph().move_message("i1", "f-deleted");
    e.sync.incremental().await.unwrap();
    assert_eq!(e.thread("Lunch").status, "deleted");
}

#[rstest]
#[tokio::test]
async fn gone_for_good_is_deleted(#[future] imported: Env) {
    let e = imported.await;
    e.graph().delete("a1");
    e.sync.incremental().await.unwrap();
    assert_eq!(e.thread("Invoice").status, "deleted");
}

#[rstest]
#[tokio::test]
async fn filed_elsewhere_joins_the_project(#[future] imported: Env) {
    let e = imported.await;
    e.graph().move_message("a1", "f-clients");
    e.sync.incremental().await.unwrap();
    assert_eq!(e.thread("Invoice").projects, vec!["Clients"]);
}

#[rstest]
#[tokio::test]
async fn a_category_set_elsewhere_is_a_project(#[future] imported: Env) {
    let e = imported.await;
    e.graph()
        .edit("a1", |m| m.categories = vec!["Taxes".into()]);
    e.sync.incremental().await.unwrap();
    assert_eq!(e.thread("Invoice").projects, vec!["Taxes"]);
}

#[rstest]
#[tokio::test]
async fn a_move_between_imported_folders_is_not_looked_up(#[future] imported: Env) {
    let e = imported.await;
    e.graph().move_message("a1", "f-clients");
    let gets_before = e.graph().prefer.len();
    e.sync.incremental().await.unwrap();
    // One delta call per imported folder, nothing else.
    assert_eq!(e.graph().prefer.len() - gets_before, 4);
}

#[rstest]
#[tokio::test]
async fn an_expired_delta_link_walks_the_folder_again(#[future] imported: Env) {
    let e = imported.await;
    let now = chrono::Utc::now().timestamp_millis();
    e.graph().add_message(
        "i2",
        "c9",
        "f-inbox",
        false,
        &mime("Di <di@example.com>", "Trip", "i2", ""),
        now,
    );
    e.graph().expire_deltas = true;
    e.sync.incremental().await.unwrap();
    assert!(e.subjects().contains("Trip"));
}

#[rstest]
#[tokio::test]
async fn new_mail_shows_while_the_import_runs(env: Env) {
    // No delta link yet: only the newest Inbox mail is checked.
    env.sync.incremental().await.unwrap();
    assert_eq!(env.subjects(), ["Lunch".to_string()].into());
}

/// Changes a thread here and pushes it.
async fn push(e: &Env, subject: &str, patch: ThreadPatch) {
    let t = e.thread(subject);
    e.store().update_threads(&[t.id], &patch).unwrap();
    e.sync.push_ops().await.unwrap();
}

fn status(s: &str) -> ThreadPatch {
    ThreadPatch {
        status: Some(s.into()),
        ..Default::default()
    }
}

fn folder_of(e: &Env, id: &str) -> String {
    e.graph().messages[id].folder.clone()
}

#[rstest]
#[tokio::test]
async fn archive_moves_to_the_archive_folder(#[future] imported: Env) {
    let e = imported.await;
    push(&e, "Lunch", status("archive")).await;
    assert_eq!(folder_of(&e, "i1"), "f-archive");
}

#[rstest]
#[tokio::test]
async fn archive_leaves_sent_mail_in_sent_items(#[future] imported: Env) {
    let e = imported.await;
    push(&e, "Lunch", status("archive")).await;
    assert_eq!(folder_of(&e, "s1"), "f-sent");
}

#[rstest]
#[tokio::test]
async fn a_status_is_a_category(#[future] imported: Env) {
    let e = imported.await;
    push(&e, "Lunch", status("waiting")).await;
    assert_eq!(e.graph().messages["i1"].categories, vec!["Mermoz/Waiting"]);
}

#[rstest]
#[tokio::test]
async fn delete_moves_to_deleted_items(#[future] imported: Env) {
    let e = imported.await;
    push(&e, "Lunch", status("deleted")).await;
    assert_eq!(folder_of(&e, "i1"), "f-deleted");
}

#[rstest]
#[tokio::test]
async fn undoing_a_delete_brings_it_back(#[future] imported: Env) {
    let e = imported.await;
    push(&e, "Lunch", status("deleted")).await;
    push(&e, "Lunch", status("todo")).await;
    assert_eq!(folder_of(&e, "i1"), "f-inbox");
}

#[rstest]
#[tokio::test]
async fn a_project_on_inbox_mail_is_a_category(#[future] imported: Env) {
    let e = imported.await;
    push(
        &e,
        "Lunch",
        ThreadPatch {
            add_projects: vec!["Travel".into()],
            ..Default::default()
        },
    )
    .await;
    assert_eq!(e.graph().messages["i1"].categories, vec!["Travel"]);
}

#[rstest]
#[tokio::test]
async fn a_folder_project_on_archived_mail_files_it(#[future] imported: Env) {
    let e = imported.await;
    push(
        &e,
        "Invoice",
        ThreadPatch {
            add_projects: vec!["Clients".into()],
            ..Default::default()
        },
    )
    .await;
    assert_eq!(folder_of(&e, "a1"), "f-clients");
}

#[rstest]
#[tokio::test]
async fn reading_here_marks_read_in_outlook(#[future] imported: Env) {
    let e = imported.await;
    let t = e.thread("Lunch");
    e.store().mark_read(&t.id, true).unwrap();
    e.sync.push_ops().await.unwrap();
    assert!(e.graph().messages["i1"].is_read);
}

#[rstest]
#[tokio::test]
async fn our_own_change_comes_back_as_an_echo(#[future] imported: Env) {
    let e = imported.await;
    push(&e, "Lunch", status("archive")).await;
    assert_eq!(e.sync.incremental().await.unwrap(), 0);
}

#[rstest]
#[tokio::test]
async fn an_echo_keeps_the_status(#[future] imported: Env) {
    let e = imported.await;
    push(&e, "Lunch", status("waiting")).await;
    e.sync.incremental().await.unwrap();
    assert_eq!(e.thread("Lunch").status, "waiting");
}

#[rstest]
#[tokio::test]
async fn a_failed_write_back_stays_queued(#[future] imported: Env) {
    let e = imported.await;
    let t = e.thread("Lunch");
    e.store()
        .update_threads(&[t.id], &status("archive"))
        .unwrap();
    // Graph down for longer than the retries.
    e.graph().throttle = 100;
    e.sync.push_ops().await.unwrap();
    e.graph().throttle = 0;
    assert_eq!(
        e.store()
            .pending_ops_for(&e.sync.account_id, 10)
            .unwrap()
            .len(),
        1
    );
}

/// Answers "Lunch" from Mermoz and sends it.
async fn send_reply(e: &Env) {
    let t = e.thread("Lunch");
    let now = crate::model::now_ms();
    let draft = e
        .store()
        .create_draft(&Draft {
            id: crate::model::new_id("drf"),
            account_id: e.sync.account_id.clone(),
            thread_id: Some(t.id.clone()),
            to: vec!["Ana <ana@example.com>".into()],
            cc: vec![],
            subject: "Re: Lunch".into(),
            body: "See you at noon".into(),
            state: "draft".into(),
            origin: "user".into(),
            created: now,
            updated: now,
            attachments: vec![],
            reply_to_message_id: None,
        })
        .unwrap();
    e.sync.send_draft(&draft, None).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn a_reply_goes_out_as_mime(#[future] imported: Env) {
    let e = imported.await;
    send_reply(&e).await;
    assert!(e.graph().sent[0].contains("In-Reply-To: <i1@example.com>"));
}

#[rstest]
#[tokio::test]
async fn a_sent_reply_joins_its_thread(#[future] imported: Env) {
    let e = imported.await;
    send_reply(&e).await;
    assert_eq!(e.thread("Lunch").message_count, 3);
}

#[rstest]
#[tokio::test]
async fn deleting_a_category_project_takes_it_off_the_mail(#[future] imported: Env) {
    let e = imported.await;
    e.graph()
        .edit("a1", |m| m.categories = vec!["Taxes".into()]);
    e.sync.incremental().await.unwrap();
    let id = e
        .store()
        .projects()
        .unwrap()
        .into_iter()
        .find(|p| p.name == "Taxes")
        .unwrap()
        .id;
    e.store().delete_project(&id).unwrap();
    e.sync.push_ops().await.unwrap();
    assert!(e.graph().messages["a1"].categories.is_empty());
}

#[rstest]
#[tokio::test]
async fn a_refused_sign_in_stops_the_sync(env: Env) {
    env.graph().refuse_refresh = true;
    assert!(matches!(
        env.sync.initial_sync().await,
        Err(Error::SignInExpired)
    ));
}

#[rstest]
#[tokio::test]
async fn gmail_search_finds_nothing_on_outlook(#[future] imported: Env) {
    assert!(
        imported
            .await
            .sync
            .search_gmail("lunch", 10)
            .await
            .unwrap()
            .is_empty()
    );
}
