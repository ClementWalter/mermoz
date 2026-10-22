//! A card title replaces the subject on the card and survives new mail on the thread.

use super::*;
use crate::mime::ParsedMessage;
use crate::model::{Address, now_ms};
use crate::store::{IncomingMessage, SyncMode};
use rstest::{fixture, rstest};

struct Env {
    _dir: tempfile::TempDir,
    store: Store,
    account: String,
    thread: String,
}

fn mail(pid: &str, subject: &str) -> IncomingMessage {
    IncomingMessage {
        provider_id: pid.into(),
        provider_thread_id: "t1".into(),
        label_ids: vec!["INBOX".into(), "UNREAD".into()],
        internal_date: now_ms(),
        parsed: ParsedMessage {
            from: Some(Address {
                name: "Ada".into(),
                email: "ada@example.com".into(),
            }),
            to: vec![Address {
                name: "".into(),
                email: "me@example.com".into(),
            }],
            subject: subject.into(),
            body_text: "Hello".into(),
            message_id: Some(format!("<{pid}@example.com>")),
            ..Default::default()
        },
    }
}

/// One account with one thread, subject "Re: Fwd: invoice 2026-114".
#[fixture]
fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
    let account = store.add_account("me@example.com", "gmail").unwrap().id;
    store
        .ingest(
            &account,
            "me@example.com",
            &mail("m1", "Re: Fwd: invoice 2026-114"),
            SyncMode::Incremental,
        )
        .unwrap();
    let thread = store.resolve_thread_id("M-1").unwrap();
    Env {
        _dir: dir,
        store,
        account,
        thread,
    }
}

#[rstest]
fn a_new_thread_has_no_title(env: Env) {
    assert_eq!(env.store.thread(&env.thread).unwrap().title, None);
}

#[rstest]
fn the_title_is_stored(env: Env) {
    env.store
        .set_thread_title(&env.thread, Some("Pay Acme"))
        .unwrap();
    assert_eq!(
        env.store.thread(&env.thread).unwrap().title.as_deref(),
        Some("Pay Acme")
    );
}

#[rstest]
fn the_subject_is_kept(env: Env) {
    env.store
        .set_thread_title(&env.thread, Some("Pay Acme"))
        .unwrap();
    assert_eq!(
        env.store.thread(&env.thread).unwrap().subject,
        "Re: Fwd: invoice 2026-114"
    );
}

#[rstest]
#[case::spaces("  Pay   Acme ")]
#[case::newline("Pay\nAcme")]
fn the_title_is_one_trimmed_line(env: Env, #[case] typed: &str) {
    env.store
        .set_thread_title(&env.thread, Some(typed))
        .unwrap();
    assert_eq!(
        env.store.thread(&env.thread).unwrap().title.as_deref(),
        Some("Pay Acme")
    );
}

#[rstest]
#[case::none(None)]
#[case::blank(Some("   "))]
fn clearing_gives_the_subject_back(env: Env, #[case] cleared: Option<&str>) {
    env.store
        .set_thread_title(&env.thread, Some("Pay Acme"))
        .unwrap();
    env.store.set_thread_title(&env.thread, cleared).unwrap();
    assert_eq!(env.store.thread(&env.thread).unwrap().title, None);
}

#[rstest]
fn renaming_returns_the_previous_title(env: Env) {
    env.store
        .set_thread_title(&env.thread, Some("Pay Acme"))
        .unwrap();
    assert_eq!(
        env.store
            .set_thread_title(&env.thread, Some("Paid"))
            .unwrap()
            .as_deref(),
        Some("Pay Acme")
    );
}

#[rstest]
fn a_too_long_title_is_refused(env: Env) {
    let long = "x".repeat(MAX_TITLE_CHARS + 1);
    assert!(matches!(
        env.store.set_thread_title(&env.thread, Some(&long)),
        Err(Error::InvalidParams(_))
    ));
}

#[rstest]
fn an_unknown_thread_is_not_found(env: Env) {
    assert!(matches!(
        env.store.set_thread_title("thr_missing", Some("x")),
        Err(Error::NotFound(_))
    ));
}

#[rstest]
fn new_mail_keeps_the_title(env: Env) {
    env.store
        .set_thread_title(&env.thread, Some("Pay Acme"))
        .unwrap();
    env.store
        .ingest(
            &env.account,
            "me@example.com",
            &mail("m2", "Re: invoice 2026-114"),
            SyncMode::Incremental,
        )
        .unwrap();
    assert_eq!(
        env.store.thread(&env.thread).unwrap().title.as_deref(),
        Some("Pay Acme")
    );
}
