//! Gmail drafts are drafts of their conversation, never mail, and follow Gmail's edits,
//! deletions and sends.

use super::*;
use crate::mime::ParsedMessage;
use crate::store::SyncMode;
use rstest::{fixture, rstest};

const ME: &str = "me@example.com";

struct Env {
    _dir: tempfile::TempDir,
    store: Store,
    acc: String,
}

impl Env {
    fn incoming(
        &self,
        pid: &str,
        labels: &[&str],
        from: &str,
        body: &str,
        date: i64,
    ) -> IncomingMessage {
        IncomingMessage {
            provider_id: pid.into(),
            provider_thread_id: "gt1".into(),
            label_ids: labels.iter().map(|l| l.to_string()).collect(),
            internal_date: date,
            parsed: ParsedMessage {
                from: Some(Address {
                    name: String::new(),
                    email: from.into(),
                }),
                to: vec![Address {
                    name: String::new(),
                    email: "drfip@dgfip.example".into(),
                }],
                subject: "Réponse à la demande de renseignements".into(),
                body_text: body.into(),
                ..Default::default()
            },
        }
    }

    fn ingest(&self, m: &IncomingMessage) -> String {
        self.store
            .ingest_many(
                &self.acc,
                ME,
                std::slice::from_ref(m),
                SyncMode::Incremental,
            )
            .unwrap()
            .pop()
            .unwrap_or_default()
    }

    /// The tax office's question, received in thread `gt1`.
    fn question(&self) -> String {
        self.ingest(&self.incoming(
            "q1",
            &["INBOX"],
            "drfip@dgfip.example",
            "Merci de préciser.",
            1_000,
        ))
    }

    /// The user's reply draft written in Gmail, Gmail message `pid`.
    fn draft(&self, pid: &str, body: &str) -> String {
        self.ingest(&self.incoming(pid, &["DRAFT"], ME, body, 2_000))
    }

    fn drafts(&self, thread: &str) -> Vec<crate::model::Draft> {
        self.store.thread_drafts(&[thread.to_string()]).unwrap()
    }
}

#[fixture]
fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
    let acc = store.add_account(ME, "gmail").unwrap().id;
    Env {
        _dir: dir,
        store,
        acc,
    }
}

#[rstest]
#[case::draft(&["DRAFT"], true)]
#[case::sent(&["SENT"], false)]
#[case::inbox(&["INBOX", "UNREAD"], false)]
fn drafts_are_known_by_their_label(#[case] labels: &[&str], #[case] expected: bool) {
    let labels: Vec<String> = labels.iter().map(|l| l.to_string()).collect();
    assert_eq!(is_draft(&labels), expected);
}

#[rstest]
fn a_gmail_draft_is_not_mail(env: Env) {
    let t = env.question();
    env.draft("d1", "Bonjour");
    assert_eq!(env.store.messages(&t).unwrap().len(), 1);
}

#[rstest]
fn a_gmail_draft_shows_in_its_conversation(env: Env) {
    let t = env.question();
    env.draft("d1", "Bonjour");
    assert_eq!(env.drafts(&t)[0].body, "Bonjour");
}

#[rstest]
fn a_gmail_draft_says_where_it_was_written(env: Env) {
    let t = env.question();
    env.draft("d1", "Bonjour");
    assert_eq!(env.drafts(&t)[0].origin, "gmail");
}

#[rstest]
fn a_gmail_draft_keeps_its_recipients(env: Env) {
    let t = env.question();
    env.draft("d1", "Bonjour");
    assert_eq!(
        env.drafts(&t)[0].to,
        vec!["drfip@dgfip.example".to_string()]
    );
}

#[rstest]
fn a_gmail_draft_leaves_the_thread_preview_alone(env: Env) {
    let t = env.question();
    env.draft("d1", "Bonjour");
    assert_ne!(env.store.thread(&t).unwrap().last_from, "You");
}

#[rstest]
fn a_gmail_draft_counts_as_known(env: Env) {
    env.question();
    env.draft("d1", "Bonjour");
    assert!(env.store.has_message(&env.acc, "d1").unwrap());
}

#[rstest]
fn an_edited_draft_replaces_the_old_version(env: Env) {
    let t = env.question();
    env.draft("d1", "Bonjour");
    env.draft("d2", "Bonjour, voici les pièces.");
    env.store.remove_gmail_draft(&env.acc, "d1").unwrap();
    let bodies: Vec<String> = env.drafts(&t).into_iter().map(|d| d.body).collect();
    assert_eq!(bodies, vec!["Bonjour, voici les pièces.".to_string()]);
}

#[rstest]
fn a_deleted_draft_names_its_thread(env: Env) {
    let t = env.question();
    env.draft("d1", "Bonjour");
    assert_eq!(
        env.store.remove_gmail_draft(&env.acc, "d1").unwrap(),
        Some(Some(t))
    );
}

#[rstest]
fn deleting_an_unknown_draft_changes_nothing(env: Env) {
    env.question();
    assert_eq!(env.store.remove_gmail_draft(&env.acc, "q1").unwrap(), None);
}

#[rstest]
fn a_draft_sent_under_its_own_id_becomes_mail(env: Env) {
    let t = env.question();
    env.draft("d1", "Bonjour");
    env.ingest(&env.incoming("d1", &["SENT"], ME, "Bonjour", 3_000));
    assert_eq!(env.store.messages(&t).unwrap().len(), 2);
}

#[rstest]
fn a_sent_draft_is_no_longer_a_draft(env: Env) {
    let t = env.question();
    env.draft("d1", "Bonjour");
    env.ingest(&env.incoming("d1", &["SENT"], ME, "Bonjour", 3_000));
    assert!(env.drafts(&t).is_empty());
}

#[rstest]
fn a_draft_imported_before_its_thread_joins_it(env: Env) {
    env.draft("d1", "Bonjour");
    let t = env.question();
    assert_eq!(env.drafts(&t).len(), 1);
}

/// A Gmail draft stored as mail by an older version: the thread's question and a
/// draft labelled `DRAFT` among its messages.
fn legacy(env: &Env) -> String {
    let t = env.question();
    env.ingest(&env.incoming("d1", &["INBOX"], ME, "Bonjour", 2_000));
    {
        let w = env.store.w.lock().unwrap();
        w.execute(
            "UPDATE messages SET labels_json = '[\"DRAFT\"]' WHERE provider_id = 'd1'",
            [],
        )
        .unwrap();
        w.execute(
            "DELETE FROM settings WHERE key = 'gmail_drafts_adopted'",
            [],
        )
        .unwrap();
    }
    t
}

#[rstest]
fn stored_drafts_leave_the_mail(env: Env) {
    let t = legacy(&env);
    env.store.adopt_stored_gmail_drafts().unwrap();
    assert_eq!(env.store.messages(&t).unwrap().len(), 1);
}

#[rstest]
fn stored_drafts_become_drafts(env: Env) {
    let t = legacy(&env);
    env.store.adopt_stored_gmail_drafts().unwrap();
    let id = env.drafts(&t)[0].id.clone();
    assert_eq!(env.store.draft_provider_id(&id).unwrap(), Some("d1".into()));
}

#[rstest]
fn stored_drafts_move_once(env: Env) {
    legacy(&env);
    env.store.adopt_stored_gmail_drafts().unwrap();
    assert_eq!(env.store.adopt_stored_gmail_drafts().unwrap(), 0);
}

#[rstest]
fn a_thread_that_only_held_a_draft_goes(env: Env) {
    let t = env.ingest(&env.incoming("d9", &["INBOX"], ME, "Brouillon", 2_000));
    env.store
        .w
        .lock()
        .unwrap()
        .execute(
            "UPDATE messages SET labels_json = '[\"DRAFT\"]' WHERE provider_id = 'd9'",
            [],
        )
        .unwrap();
    env.store
        .w
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM settings WHERE key = 'gmail_drafts_adopted'",
            [],
        )
        .unwrap();
    env.store.adopt_stored_gmail_drafts().unwrap();
    assert!(env.store.thread(&t).is_err());
}
