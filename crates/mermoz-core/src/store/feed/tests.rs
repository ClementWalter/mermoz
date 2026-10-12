//! Store tests for the feed: what gets judged, what the feed reads back, snoozes,
//! cached card words and how people usually answer.

use super::*;
use crate::mime::ParsedMessage;
use crate::model::Address;
use crate::store::{IncomingMessage, SyncMode, ThreadPatch};
use rstest::{fixture, rstest};

const ME: &str = "me@example.com";
const MARIE: &str = "marie@archi.example";

struct Env {
    _dir: tempfile::TempDir,
    store: Store,
    acc: String,
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

fn today() -> chrono::NaiveDate {
    feed::today()
}

impl Env {
    /// One message in thread `tid`, `days_ago` days back, from `from`.
    fn mail(&self, pid: &str, tid: &str, from: &str, days_ago: i64, body: &str) -> String {
        let to = if from == ME { MARIE } else { ME };
        let msg = IncomingMessage {
            provider_id: pid.into(),
            provider_thread_id: tid.into(),
            label_ids: vec!["INBOX".into(), "UNREAD".into()],
            internal_date: now_ms() - days_ago * DAY_MS,
            parsed: ParsedMessage {
                from: Some(Address {
                    name: String::new(),
                    email: from.into(),
                }),
                to: vec![Address {
                    name: String::new(),
                    email: to.into(),
                }],
                subject: format!("Subject {tid}"),
                body_text: body.into(),
                message_id: Some(format!("<{pid}@x>")),
                ..Default::default()
            },
        };
        self.store
            .ingest(&self.acc, ME, &msg, SyncMode::Initial)
            .unwrap()
    }

    fn set_status(&self, id: &str, status: &str) {
        self.store
            .update_threads(
                &[id.to_string()],
                &ThreadPatch {
                    status: Some(status.into()),
                    ..Default::default()
                },
            )
            .unwrap();
    }

    fn last_date(&self, id: &str) -> i64 {
        self.store.thread(id).unwrap().last_date
    }

    fn judge(&self, id: &str, j: &Judgment) {
        self.store
            .save_judgment(id, j, self.last_date(id), true, &today().to_string())
            .unwrap();
    }

    fn to_judge(&self) -> Vec<ToJudge> {
        self.store
            .threads_to_judge(&today().to_string(), 50)
            .unwrap()
    }

    fn units(&self) -> Vec<FeedUnit> {
        self.store
            .feed_units(&ColumnScope::default(), today(), now_ms())
            .unwrap()
    }

    fn unit_ids(&self) -> Vec<String> {
        self.units().into_iter().map(|u| u.id).collect()
    }

    fn unit(&self, id: &str) -> FeedUnit {
        self.units().into_iter().find(|u| u.id == id).unwrap()
    }
}

fn due_in(days: i64) -> Judgment {
    Judgment {
        kind: Some(Kind::Event),
        due: Some((today() + chrono::Duration::days(days)).to_string()),
        ..Default::default()
    }
}

#[rstest]
#[case::none(vec![], None)]
#[case::odd(vec![DAY_MS, 3 * DAY_MS, 2 * DAY_MS], Some(2.0))]
#[case::even(vec![DAY_MS, 2 * DAY_MS], Some(1.5))]
fn median_of_reply_delays(#[case] gaps: Vec<i64>, #[case] want: Option<f64>) {
    assert_eq!(median_days(gaps), want);
}

// ---- what to judge -------------------------------------------------------------

#[rstest]
fn open_thread_is_to_judge(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    assert_eq!(env.to_judge()[0].0, t);
}

#[rstest]
fn judged_thread_is_left_alone(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.judge(&t, &Judgment::default());
    assert!(env.to_judge().is_empty());
}

#[rstest]
fn new_mail_asks_again(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 1, "hi");
    env.judge(&t, &Judgment::default());
    env.mail("m2", "t1", MARIE, 0, "again");
    assert_eq!(env.to_judge().len(), 1);
}

#[rstest]
fn archived_thread_is_not_judged(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.set_status(&t, "archive");
    assert!(env.to_judge().is_empty());
}

#[rstest]
fn recent_backlog_is_offered_as_backlog(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 3, "hi");
    env.set_status(&t, "backlog");
    assert!(env.to_judge()[0].2);
}

#[rstest]
fn old_backlog_is_not_scanned(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 90, "hi");
    env.set_status(&t, "backlog");
    assert!(env.to_judge().is_empty());
}

#[rstest]
fn backlog_without_a_close_date_waits_for_tomorrow(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 3, "hi");
    env.set_status(&t, "backlog");
    env.store
        .save_judgment(
            &t,
            &Judgment::default(),
            env.last_date(&t),
            false,
            &today().to_string(),
        )
        .unwrap();
    assert!(env.to_judge().is_empty());
}

// ---- units ---------------------------------------------------------------------

#[rstest]
fn open_threads_are_units(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    assert_eq!(env.unit_ids(), vec![t]);
}

#[rstest]
fn done_threads_are_not_units(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.set_status(&t, "done");
    assert!(env.unit_ids().is_empty());
}

#[rstest]
fn judgment_comes_with_the_unit(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.judge(&t, &due_in(1));
    assert_eq!(env.unit(&t).judgment.unwrap().kind, Some(Kind::Event));
}

#[rstest]
fn backlog_with_a_close_date_comes_back(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 3, "demain");
    env.set_status(&t, "backlog");
    env.judge(&t, &due_in(1));
    assert!(env.unit(&t).from_backlog);
}

#[rstest]
fn backlog_with_a_far_date_stays_out(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 3, "hi");
    env.set_status(&t, "backlog");
    env.judge(&t, &due_in(10));
    assert!(env.unit_ids().is_empty());
}

#[rstest]
fn snoozed_thread_is_hidden(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.store.snooze(&[t], now_ms() + DAY_MS).unwrap();
    assert!(env.unit_ids().is_empty());
}

#[rstest]
fn snooze_ends_at_its_time(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.store
        .snooze(std::slice::from_ref(&t), now_ms() - 1)
        .unwrap();
    assert_eq!(env.unit_ids(), vec![t]);
}

#[rstest]
fn new_mail_ends_a_snooze(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 1, "hi");
    env.store
        .snooze(std::slice::from_ref(&t), now_ms() + DAY_MS)
        .unwrap();
    env.mail("m2", "t1", MARIE, 0, "again");
    assert_eq!(env.unit_ids(), vec![t]);
}

#[rstest]
fn unsnooze_brings_it_back(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.store
        .snooze(std::slice::from_ref(&t), now_ms() + DAY_MS)
        .unwrap();
    env.store.unsnooze(std::slice::from_ref(&t)).unwrap();
    assert_eq!(env.unit_ids(), vec![t]);
}

#[rstest]
fn no_reply_sender_is_flagged(env: Env) {
    let t = env.mail("m1", "t1", "noreply@shop.example", 0, "receipt");
    assert!(env.unit(&t).automated_sender);
}

#[rstest]
fn waiting_counts_days_since_my_last_message(env: Env) {
    env.mail("m1", "t1", MARIE, 9, "devis ?");
    let t = env.mail("m2", "t1", ME, 6, "Pouvez-vous m'envoyer le devis ?");
    env.set_status(&t, "waiting");
    assert_eq!(env.unit(&t).waiting_days, Some(6));
}

#[rstest]
fn shown_cards_get_their_projects(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.store
        .update_threads(
            std::slice::from_ref(&t),
            &ThreadPatch {
                add_projects: vec!["Kitchen".into()],
                ..Default::default()
            },
        )
        .unwrap();
    let mut f = feed::assemble(env.units(), today(), &Default::default());
    env.store.complete_feed(&mut f).unwrap();
    assert_eq!(f.sections[0].cards[0].thread.projects, vec!["Kitchen"]);
}

#[rstest]
fn project_scope_keeps_only_its_threads(env: Env) {
    env.mail("m1", "t1", MARIE, 0, "hi");
    let scope = ColumnScope {
        project: Some("Rénovation".into()),
        account_id: None,
    };
    assert!(
        env.store
            .feed_units(&scope, today(), now_ms())
            .unwrap()
            .is_empty()
    );
}

// ---- card words -----------------------------------------------------------------

fn words() -> Texts {
    Texts {
        title: "Choose the tiles".into(),
        ..Default::default()
    }
}

#[rstest]
fn missing_words_are_needed(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    assert_eq!(env.store.feed_texts_needed(&[t]).unwrap().len(), 1);
}

#[rstest]
fn saved_words_come_with_the_unit(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.store
        .save_feed_texts(&t, env.last_date(&t), "prv", &Ok(words()))
        .unwrap();
    assert_eq!(env.unit(&t).texts.unwrap().title, "Choose the tiles");
}

#[rstest]
fn failed_words_are_not_asked_again(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.store
        .save_feed_texts(
            &t,
            env.last_date(&t),
            "prv",
            &Err(crate::Error::other("no")),
        )
        .unwrap();
    assert!(env.store.feed_texts_needed(&[t]).unwrap().is_empty());
}

#[rstest]
fn new_mail_makes_words_stale(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 1, "hi");
    env.store
        .save_feed_texts(&t, env.last_date(&t), "prv", &Ok(words()))
        .unwrap();
    env.mail("m2", "t1", MARIE, 0, "again");
    assert!(env.unit(&t).texts.is_none());
}

// ---- how people answer -------------------------------------------------------------

#[rstest]
fn answered_people_weigh_more(env: Env) {
    env.mail("a1", "old1", MARIE, 30, "q1");
    env.mail("a2", "old1", ME, 29, "r1");
    env.mail("b1", "old2", MARIE, 20, "q2");
    let t = env.mail("c1", "new", MARIE, 0, "q3");
    assert_eq!(env.store.sender_stats(&t).unwrap().0, 0.5);
}

#[rstest]
fn typical_reply_is_the_median_delay(env: Env) {
    env.mail("a1", "old1", ME, 30, "q1");
    env.mail("a2", "old1", MARIE, 28, "r1");
    env.mail("b1", "old2", ME, 20, "q2");
    env.mail("b2", "old2", MARIE, 16, "r2");
    let t = env.mail("c1", "new", MARIE, 0, "q3");
    // Rounded: each message is stamped a few milliseconds apart.
    let days = env.store.sender_stats(&t).unwrap().1.map(f64::round);
    assert_eq!(days, Some(3.0));
}

#[rstest]
fn waiting_thread_shows_as_waiting_for_the_ai(env: Env) {
    let t = env.mail("m1", "t1", MARIE, 0, "hi");
    env.set_status(&t, "waiting");
    assert_eq!(env.store.feed_kind(&t).unwrap(), Kind::Waiting);
}
