//! Queue order (the user's edits before a bulk run's) and the batched label mirror.

use super::*;
use crate::mime::ParsedMessage;
use crate::model::Address;
use crate::store::{IncomingMessage, SyncMode, ThreadPatch};
use rstest::{fixture, rstest};

const ME: &str = "me@example.com";

struct Box {
    _dir: tempfile::TempDir,
    store: Store,
    acc: String,
}

impl Box {
    /// One thread in the inbox with `messages` messages, `p-<tid>-<n>` in Gmail.
    fn thread(&self, tid: &str, messages: usize) -> String {
        let mut id = String::new();
        for n in 0..messages {
            let msg = IncomingMessage {
                provider_id: format!("p-{tid}-{n}"),
                provider_thread_id: tid.into(),
                label_ids: vec!["INBOX".into()],
                internal_date: 1_000 + n as i64,
                parsed: ParsedMessage {
                    from: Some(Address {
                        name: "Nora Lind".into(),
                        email: "nora@lind.example".into(),
                    }),
                    subject: format!("Ferry {tid}"),
                    body_text: "Hello".into(),
                    message_id: Some(format!("<{tid}-{n}@lind.example>")),
                    ..Default::default()
                },
            };
            id = self
                .store
                .ingest(&self.acc, ME, &msg, SyncMode::Initial)
                .unwrap();
        }
        id
    }

    fn archive(&self, ids: &[String]) {
        self.store
            .update_threads(
                ids,
                &ThreadPatch {
                    status: Some("archive".into()),
                    ..Default::default()
                },
            )
            .unwrap();
    }

    fn queue(&self) -> Vec<String> {
        self.store
            .pending_ops_for(&self.acc, 100)
            .unwrap()
            .into_iter()
            .map(|o| o.key)
            .collect()
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
fn the_users_edit_goes_before_a_bulk_run(mailbox: Box) {
    let bulk = vec![mailbox.thread("t1", 1), mailbox.thread("t2", 1)];
    mailbox.archive(&bulk);
    mailbox.store.mark_ops_bulk(&bulk).unwrap();
    let mine = mailbox.thread("t3", 1);
    mailbox.archive(std::slice::from_ref(&mine));
    assert_eq!(
        mailbox.queue(),
        vec![mine, bulk[0].clone(), bulk[1].clone()]
    );
}

#[rstest]
fn editing_a_bulk_thread_again_makes_it_the_users(mailbox: Box) {
    let bulk = vec![mailbox.thread("t1", 1), mailbox.thread("t2", 1)];
    mailbox.archive(&bulk);
    mailbox.store.mark_ops_bulk(&bulk).unwrap();
    mailbox.archive(std::slice::from_ref(&bulk[1]));
    assert_eq!(mailbox.queue(), vec![bulk[1].clone(), bulk[0].clone()]);
}

#[rstest]
fn the_queue_holds_only_this_accounts_ops(mailbox: Box) {
    let t = mailbox.thread("t1", 1);
    mailbox.archive(std::slice::from_ref(&t));
    let other = mailbox
        .store
        .add_account("work@example.com", "gmail")
        .unwrap()
        .id;
    assert!(
        mailbox
            .store
            .pending_ops_for(&other, 100)
            .unwrap()
            .is_empty()
    );
}

#[rstest]
fn every_message_of_each_thread_is_listed(mailbox: Box) {
    let t = mailbox.thread("t1", 3);
    let ids = mailbox
        .store
        .provider_message_ids(std::slice::from_ref(&t))
        .unwrap();
    assert_eq!(ids[&t].len(), 3);
}

#[rstest]
fn a_batched_change_is_mirrored_on_every_message(mailbox: Box) {
    let ts = vec![mailbox.thread("t1", 2), mailbox.thread("t2", 1)];
    mailbox
        .store
        .apply_label_delta_many(&ts, &[], &["INBOX".to_string()])
        .unwrap();
    let still: Vec<bool> = ["p-t1-0", "p-t1-1", "p-t2-0"]
        .iter()
        .map(|p| {
            mailbox
                .store
                .message_label_ids(&mailbox.acc, p)
                .unwrap()
                .unwrap()
                .contains("INBOX")
        })
        .collect();
    assert_eq!(still, vec![false, false, false]);
}

#[rstest]
fn an_unknown_message_has_no_labels(mailbox: Box) {
    assert_eq!(
        mailbox
            .store
            .message_label_ids(&mailbox.acc, "nope")
            .unwrap(),
        None
    );
}

#[rstest]
fn done_ops_leave_the_queue(mailbox: Box) {
    let ts = vec![mailbox.thread("t1", 1), mailbox.thread("t2", 1)];
    mailbox.archive(&ts);
    let ops = mailbox.store.pending_ops_for(&mailbox.acc, 100).unwrap();
    mailbox
        .store
        .ops_done(&ops.iter().collect::<Vec<_>>())
        .unwrap();
    assert!(mailbox.queue().is_empty());
}
