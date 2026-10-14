//! Thread signatures, documents and lookups for search by meaning (fictional mail).

use super::*;
use crate::mime::ParsedMessage;
use crate::model::Address;
use crate::store::{IncomingMessage, Store, SyncMode, ThreadPatch};
use rstest::{fixture, rstest};

const ME: &str = "me@example.com";

struct Box {
    _dir: tempfile::TempDir,
    store: Store,
    acc: String,
}

impl Box {
    /// Adds message `n` of Gmail thread `tid`, from `from`, at `date`.
    fn mail(&self, tid: &str, n: usize, from: &str, date: i64, body: &str) -> String {
        let msg = IncomingMessage {
            provider_id: format!("p-{tid}-{n}"),
            provider_thread_id: tid.into(),
            label_ids: vec!["INBOX".into()],
            internal_date: date,
            parsed: ParsedMessage {
                from: Some(Address {
                    name: from.into(),
                    email: format!("{}@ferry.example", from.to_lowercase()),
                }),
                subject: format!("Ferry {tid}"),
                body_text: body.into(),
                message_id: Some(format!("<{tid}-{n}@ferry.example>")),
                ..Default::default()
            },
        };
        self.store
            .ingest(&self.acc, ME, &msg, SyncMode::Initial)
            .unwrap()
    }
}

#[fixture]
fn mailbox() -> Box {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
    let acc = store.add_account(ME, "gmail").unwrap().id;
    Box {
        _dir: dir,
        store,
        acc,
    }
}

#[rstest]
fn sigs_list_the_newest_thread_first(mailbox: Box) {
    mailbox.mail("old", 0, "Nora", 1_000, "a");
    let new = mailbox.mail("new", 0, "Nora", 2_000, "b");
    assert_eq!(mailbox.store.semantic_sigs().unwrap()[0].0, new);
}

#[rstest]
fn a_new_message_changes_the_sig(mailbox: Box) {
    mailbox.mail("t", 0, "Nora", 1_000, "a");
    let before = mailbox.store.semantic_sigs().unwrap()[0].1.clone();
    mailbox.mail("t", 1, "Ivo", 2_000, "b");
    assert_ne!(mailbox.store.semantic_sigs().unwrap()[0].1, before);
}

#[rstest]
fn sigs_leave_deleted_threads_out(mailbox: Box) {
    let id = mailbox.mail("t", 0, "Nora", 1_000, "a");
    mailbox
        .store
        .update_threads(
            std::slice::from_ref(&id),
            &ThreadPatch {
                status: Some("deleted".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(mailbox.store.semantic_sigs().unwrap().is_empty());
}

#[rstest]
fn docs_carry_the_subject_and_writers(mailbox: Box) {
    let id = mailbox.mail("t", 0, "Nora", 1_000, "The ferry leaves at noon.");
    mailbox.mail("t", 1, "Ivo", 2_000, "Thanks!");
    let doc = mailbox.store.semantic_docs(&[id]).unwrap()[0].1.clone();
    assert!(doc.starts_with("title: Ferry t | text: From Ivo, Nora. The ferry leaves at noon."));
}

#[rstest]
fn docs_read_only_the_first_two_messages(mailbox: Box) {
    let id = mailbox.mail("t", 0, "Nora", 1_000, "one");
    mailbox.mail("t", 1, "Ivo", 2_000, "two");
    mailbox.mail("t", 2, "Ana", 3_000, "three");
    let doc = mailbox.store.semantic_docs(&[id]).unwrap()[0].1.clone();
    assert!(!doc.contains("three"));
}

#[rstest]
fn docs_skip_unknown_threads(mailbox: Box) {
    assert!(
        mailbox
            .store
            .semantic_docs(&["nope".into()])
            .unwrap()
            .is_empty()
    );
}

#[rstest]
fn unquoted_drops_quoted_lines() {
    assert_eq!(unquoted("Sure.\n> old text\n  > older"), "Sure.");
}

#[rstest]
fn threads_by_ids_keeps_the_order_given(mailbox: Box) {
    let a = mailbox.mail("a", 0, "Nora", 1_000, "x");
    let b = mailbox.mail("b", 0, "Nora", 2_000, "y");
    let got: Vec<String> = mailbox
        .store
        .threads_by_ids(&[a.clone(), b.clone()])
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(got, vec![a, b]);
}

#[rstest]
fn threads_by_ids_skips_hidden_accounts(mailbox: Box) {
    let a = mailbox.mail("a", 0, "Nora", 1_000, "x");
    mailbox
        .store
        .set_account_hidden(&mailbox.acc, true)
        .unwrap();
    assert!(mailbox.store.threads_by_ids(&[a]).unwrap().is_empty());
}

#[rstest]
fn threads_by_ids_skips_unknown_ids(mailbox: Box) {
    let a = mailbox.mail("a", 0, "Nora", 1_000, "x");
    assert_eq!(
        mailbox
            .store
            .threads_by_ids(&["gone".into(), a])
            .unwrap()
            .len(),
        1
    );
}
