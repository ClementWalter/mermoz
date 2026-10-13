//! Counts stay exact across writes: each case reads the counts once (filling the
//! cache), changes the mailbox, then checks the answer.

use super::*;
use crate::mime::ParsedMessage;
use crate::model::{Address, now_ms};
use crate::store::{IncomingMessage, SyncMode, ThreadPatch};
use rstest::{fixture, rstest};

const ME: &str = "me@example.com";

struct Env {
    dir: tempfile::TempDir,
    store: Store,
    acc: String,
    /// Two Unread threads from different people; the first is in project "Billing".
    threads: Vec<String>,
}

fn mail(pid: &str, tid: &str, from: &str) -> IncomingMessage {
    IncomingMessage {
        provider_id: pid.into(),
        provider_thread_id: tid.into(),
        label_ids: vec!["INBOX".into(), "UNREAD".into()],
        internal_date: now_ms(),
        parsed: ParsedMessage {
            from: Some(Address {
                name: "Ada Example".into(),
                email: from.into(),
            }),
            to: vec![Address {
                name: "".into(),
                email: ME.into(),
            }],
            subject: format!("Invoice {tid}"),
            body_text: "Please find the invoice attached.".into(),
            message_id: Some(format!("<{pid}@example.com>")),
            ..Default::default()
        },
    }
}

#[fixture]
fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
    let acc = store.add_account(ME, "gmail").unwrap().id;
    let threads = vec![
        store
            .ingest(
                &acc,
                ME,
                &mail("m1", "t1", "ada@example.com"),
                SyncMode::Incremental,
            )
            .unwrap(),
        store
            .ingest(
                &acc,
                ME,
                &mail("m2", "t2", "bob@example.org"),
                SyncMode::Incremental,
            )
            .unwrap(),
    ];
    store
        .update_threads(
            &threads[..1],
            &ThreadPatch {
                add_projects: vec!["Billing".into()],
                ..Default::default()
            },
        )
        .unwrap();
    // Fills the cache, so every case checks it is dropped when it must be.
    store.counts(true).unwrap();
    store.counts(false).unwrap();
    Env {
        dir,
        store,
        acc,
        threads,
    }
}

#[derive(Debug, Clone, Copy)]
enum Change {
    Status,
    AddProject,
    RemoveProject,
    Delete,
    NewMail,
    Read,
    Priority,
}

fn apply(env: &Env, change: Change) {
    let s = &env.store;
    let first = &env.threads[..1];
    let patch = |p: ThreadPatch| {
        s.update_threads(first, &p).unwrap();
    };
    match change {
        Change::Status => patch(ThreadPatch {
            status: Some("waiting".into()),
            ..Default::default()
        }),
        Change::AddProject => patch(ThreadPatch {
            add_projects: vec!["Travel".into()],
            ..Default::default()
        }),
        Change::RemoveProject => patch(ThreadPatch {
            remove_projects: vec!["Billing".into()],
            ..Default::default()
        }),
        Change::Delete => patch(ThreadPatch {
            status: Some("deleted".into()),
            ..Default::default()
        }),
        Change::Priority => patch(ThreadPatch {
            priority: Some(1),
            ..Default::default()
        }),
        Change::NewMail => {
            s.ingest(
                &env.acc,
                ME,
                &mail("m3", "t3", "carol@example.net"),
                SyncMode::Incremental,
            )
            .unwrap();
        }
        Change::Read => s.mark_read(&env.threads[0], true).unwrap(),
    }
}

fn count(v: &Value, dim: &str, key: &str) -> Option<i64> {
    v[dim].get(key).and_then(Value::as_i64)
}

#[rstest]
#[case::status_change_leaves_unread(Change::Status, "status", "unread", Some(1))]
#[case::status_change_reaches_waiting(Change::Status, "status", "waiting", Some(1))]
#[case::project_add(Change::AddProject, "project", "Travel", Some(1))]
#[case::project_remove(Change::RemoveProject, "project", "Billing", None)]
#[case::delete_leaves_project(Change::Delete, "project", "Billing", None)]
#[case::delete_leaves_unread(Change::Delete, "status", "unread", Some(1))]
#[case::new_mail_is_unread(Change::NewMail, "status", "unread", Some(3))]
#[case::read_moves_on(Change::Read, "status", "unread", Some(1))]
#[case::priority_change(Change::Priority, "priority", "1", Some(1))]
fn conversation_counts_follow_a_change(
    env: Env,
    #[case] change: Change,
    #[case] dim: &str,
    #[case] key: &str,
    #[case] want: Option<i64>,
) {
    apply(&env, change);
    assert_eq!(count(&env.store.counts(true).unwrap(), dim, key), want);
}

#[rstest]
#[case::status_change(Change::Status, "status", "waiting", Some(1))]
#[case::project_add(Change::AddProject, "project", "Travel", Some(1))]
#[case::project_remove(Change::RemoveProject, "project", "Billing", None)]
#[case::delete(Change::Delete, "status", "unread", Some(1))]
#[case::new_mail(Change::NewMail, "status", "unread", Some(3))]
fn thread_counts_follow_a_change(
    env: Env,
    #[case] change: Change,
    #[case] dim: &str,
    #[case] key: &str,
    #[case] want: Option<i64>,
) {
    apply(&env, change);
    assert_eq!(count(&env.store.counts(false).unwrap(), dim, key), want);
}

/// The kept answer matches a fresh count from a store that never cached anything.
#[rstest]
#[case::status_change(Change::Status)]
#[case::project_add(Change::AddProject)]
#[case::project_remove(Change::RemoveProject)]
#[case::delete(Change::Delete)]
#[case::new_mail(Change::NewMail)]
#[case::read(Change::Read)]
#[case::priority(Change::Priority)]
fn cached_counts_match_a_fresh_count(env: Env, #[case] change: Change) {
    apply(&env, change);
    let fresh = Store::open(
        &env.dir.path().join("db.sqlite"),
        &env.dir.path().join("blobs"),
    )
    .unwrap()
    .counts(true)
    .unwrap();
    assert_eq!(env.store.counts(true).unwrap(), fresh);
}

#[rstest]
fn an_unrelated_write_keeps_the_generation(env: Env) {
    let before: i64 = env
        .store
        .r
        .lock()
        .unwrap()
        .query_row("SELECT n FROM counts_gen", [], |r| r.get(0))
        .unwrap();
    env.store
        .update_threads(
            &env.threads[..1],
            &ThreadPatch {
                due: Some(Some("2026-11-02".into())),
                ..Default::default()
            },
        )
        .unwrap();
    let after: i64 = env
        .store
        .r
        .lock()
        .unwrap()
        .query_row("SELECT n FROM counts_gen", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before);
}

#[rstest]
fn closing_a_status_drops_its_count(env: Env) {
    env.store
        .w
        .lock()
        .unwrap()
        .execute("UPDATE statuses SET archived = 1 WHERE key = 'unread'", [])
        .unwrap();
    assert_eq!(
        count(&env.store.counts(true).unwrap(), "status", "unread"),
        None
    );
}
