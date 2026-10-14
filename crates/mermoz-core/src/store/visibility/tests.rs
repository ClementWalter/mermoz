//! A hidden account's mail is left out of what the app reads, and comes back when shown.

use super::*;
use crate::mime::ParsedMessage;
use crate::model::{Address, now_ms};
use crate::store::{ColumnScope, IncomingMessage, SyncMode, ThreadFilter};
use rstest::{fixture, rstest};

struct Env {
    dir: tempfile::TempDir,
    store: Store,
    hidden: String,
}

fn mail(pid: &str, name: &str, from: &str, to: &str) -> IncomingMessage {
    IncomingMessage {
        provider_id: pid.into(),
        provider_thread_id: pid.into(),
        label_ids: vec!["INBOX".into(), "UNREAD".into()],
        internal_date: now_ms(),
        parsed: ParsedMessage {
            from: Some(Address {
                name: name.into(),
                email: from.into(),
            }),
            to: vec![Address {
                name: "".into(),
                email: to.into(),
            }],
            subject: "Quarterly invoice".into(),
            body_text: "Please find the invoice attached.".into(),
            message_id: Some(format!("<{pid}@example.com>")),
            ..Default::default()
        },
    }
}

/// Two accounts with one Unread invoice each; the second account is hidden.
#[fixture]
fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
    let shown = store.add_account("me@example.com", "gmail").unwrap().id;
    let hidden = store.add_account("demo@example.org", "gmail").unwrap().id;
    let a = mail("m1", "Ada Shown", "ada@example.com", "me@example.com");
    store
        .ingest(&shown, "me@example.com", &a, SyncMode::Incremental)
        .unwrap();
    let b = mail("m2", "Bob Hidden", "bob@example.org", "demo@example.org");
    store
        .ingest(&hidden, "demo@example.org", &b, SyncMode::Incremental)
        .unwrap();
    // Fills the counts cache, so hiding must drop it.
    store.counts(false).unwrap();
    store.set_account_hidden(&hidden, true).unwrap();
    Env { dir, store, hidden }
}

fn total(s: &Store) -> i64 {
    s.threads(&ThreadFilter::default()).unwrap().1
}

#[rstest]
fn the_account_reads_back_hidden(env: Env) {
    assert!(env.store.account(&env.hidden).unwrap().hidden);
}

#[rstest]
fn lists_leave_hidden_mail_out(env: Env) {
    assert_eq!(total(&env.store), 1);
}

#[rstest]
fn counts_leave_the_hidden_account_out(env: Env) {
    let c = env.store.counts(false).unwrap();
    assert!(c["account"].get(&env.hidden).is_none());
}

#[rstest]
fn unread_count_drops_hidden_mail(env: Env) {
    assert_eq!(env.store.counts(false).unwrap()["status"]["unread"], 1);
}

#[rstest]
fn search_leaves_hidden_mail_out(env: Env) {
    assert_eq!(env.store.search("invoice", 50, false).unwrap().len(), 1);
}

#[rstest]
fn the_feed_leaves_hidden_mail_out(env: Env) {
    let today = chrono::Local::now().date_naive();
    let units = env
        .store
        .feed_units(&ColumnScope::default(), today, now_ms())
        .unwrap();
    assert_eq!(units.len(), 1);
}

#[rstest]
fn ask_mermoz_never_finds_hidden_senders(env: Env) {
    let found = env.store.resolve_senders(&["Bob Hidden".into()]).unwrap();
    assert!(found.is_empty());
}

#[rstest]
fn bulk_changes_never_match_hidden_mail(env: Env) {
    let matches = env
        .store
        .plan_matches(&crate::plan::Filter::default(), now_ms())
        .unwrap();
    assert_eq!(matches.len(), 1);
}

#[rstest]
fn showing_again_brings_the_mail_back(env: Env) {
    env.store.set_account_hidden(&env.hidden, false).unwrap();
    assert_eq!(total(&env.store), 2);
}

#[rstest]
fn hiding_survives_a_restart(env: Env) {
    let Env { dir, store, .. } = env;
    drop(store);
    let again = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
    assert_eq!(total(&again), 1);
}

#[rstest]
fn removing_the_hidden_account_unhides_nothing_else(env: Env) {
    env.store.remove_account(&env.hidden).unwrap();
    assert!(env.store.hidden_accounts().unwrap().is_empty());
}

#[rstest]
fn hiding_an_unknown_account_fails(env: Env) {
    assert!(env.store.set_account_hidden("acc_nope", true).is_err());
}
