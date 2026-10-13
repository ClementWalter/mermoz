//! Filter resolution against a small fictional mailbox, and the operation log.

use super::*;
use crate::mime::ParsedMessage;
use crate::model::Address;
use crate::plan::{Age, Unit};
use crate::store::{IncomingMessage, SyncMode, ThreadPatch};
use rstest::{fixture, rstest};

const ME: &str = "me@example.com";
const DAY: i64 = 86_400_000;
/// 2026-10-13T00:00:00Z: "now" for every filter here.
const NOW: i64 = 1_791_849_600_000;

struct Mailbox {
    _dir: tempfile::TempDir,
    store: Store,
}

impl Mailbox {
    /// One thread, its single mail `days_ago` days before NOW.
    fn mail(
        &self,
        tid: &str,
        from: (&str, &str),
        days_ago: i64,
        subject: &str,
        body: &str,
    ) -> String {
        let acc = self.store.accounts().unwrap()[0].id.clone();
        let msg = IncomingMessage {
            provider_id: format!("p-{tid}"),
            provider_thread_id: tid.into(),
            label_ids: vec!["INBOX".into()],
            internal_date: NOW - days_ago * DAY,
            parsed: ParsedMessage {
                from: Some(Address {
                    name: from.0.into(),
                    email: from.1.into(),
                }),
                to: vec![Address {
                    name: String::new(),
                    email: ME.into(),
                }],
                subject: subject.into(),
                body_text: body.into(),
                message_id: Some(format!("<{tid}@x>")),
                ..Default::default()
            },
        };
        self.store
            .ingest(&acc, ME, &msg, SyncMode::Initial)
            .unwrap()
    }

    fn patch(&self, id: &str, p: ThreadPatch) {
        self.store.update_threads(&[id.to_string()], &p).unwrap();
    }

    fn subjects(&self, f: Filter) -> Vec<String> {
        let mut s: Vec<String> = self
            .store
            .plan_matches(&f, NOW)
            .unwrap()
            .into_iter()
            .map(|m| m.subject)
            .collect();
        s.sort();
        s
    }
}

/// Five fictional threads:
/// - "Ferry tickets" from Nora, 200 days old, Done, in Travel
/// - "Old newsletter" from news.example, 300 days old, To do
/// - "Garden quote" from Paul, 10 days old, To do, in Home
/// - "Trashed promo" from shop.example, 400 days old, Deleted
/// - "Hotel booking" from Nora, 180 days old, Archive, in Travel
#[fixture]
fn mailbox() -> Mailbox {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
    store.add_account(ME, "gmail").unwrap();
    let m = Mailbox { _dir: dir, store };
    let status = |s: &str| ThreadPatch {
        status: Some(s.into()),
        ..Default::default()
    };
    let ferry = m.mail(
        "t1",
        ("Nora Lind", "nora@lind.example"),
        200,
        "Ferry tickets",
        "Cabin for two",
    );
    m.patch(&ferry, status("done"));
    m.patch(
        &ferry,
        ThreadPatch {
            add_projects: vec!["Travel".into()],
            ..Default::default()
        },
    );
    let news = m.mail(
        "t2",
        ("Weekly", "digest@mail.news.example"),
        300,
        "Old newsletter",
        "Ten links",
    );
    m.patch(&news, status("todo"));
    let garden = m.mail(
        "t3",
        ("Paul Roux", "paul@roux.example"),
        10,
        "Garden quote",
        "Hedge and lawn invoice",
    );
    m.patch(&garden, status("todo"));
    m.patch(
        &garden,
        ThreadPatch {
            add_projects: vec!["Home".into()],
            ..Default::default()
        },
    );
    let promo = m.mail(
        "t4",
        ("Shop", "deals@shop.example"),
        400,
        "Trashed promo",
        "Sale",
    );
    m.patch(&promo, status("deleted"));
    let hotel = m.mail(
        "t5",
        ("Nora Lind", "nora@lind.example"),
        180,
        "Hotel booking",
        "Two nights",
    );
    m.patch(&hotel, status("archive"));
    m.patch(
        &hotel,
        ThreadPatch {
            add_projects: vec!["Travel".into()],
            ..Default::default()
        },
    );
    m
}

fn months(n: u32) -> Option<Age> {
    Some(Age {
        amount: n,
        unit: Unit::Month,
    })
}

#[rstest]
fn older_than_five_months_and_not_done(mailbox: Mailbox) {
    let f = Filter {
        older_than: months(5),
        status_not_in: vec!["done".into()],
        ..Default::default()
    };
    assert_eq!(mailbox.subjects(f), vec!["Hotel booking", "Old newsletter"]);
}

#[rstest]
fn older_than_five_months(mailbox: Mailbox) {
    let f = Filter {
        older_than: months(5),
        ..Default::default()
    };
    assert_eq!(
        mailbox.subjects(f),
        vec!["Ferry tickets", "Hotel booking", "Old newsletter"]
    );
}

#[rstest]
fn newer_than_a_month(mailbox: Mailbox) {
    let f = Filter {
        newer_than: months(1),
        ..Default::default()
    };
    assert_eq!(mailbox.subjects(f), vec!["Garden quote"]);
}

#[rstest]
fn the_trash_is_left_out_unless_asked(mailbox: Mailbox) {
    assert!(
        !mailbox
            .subjects(Filter::default())
            .contains(&"Trashed promo".to_string())
    );
}

#[rstest]
fn the_trash_is_found_when_asked(mailbox: Mailbox) {
    let f = Filter {
        status_in: vec!["deleted".into()],
        ..Default::default()
    };
    assert_eq!(mailbox.subjects(f), vec!["Trashed promo"]);
}

#[rstest]
fn by_sender_address(mailbox: Mailbox) {
    let f = Filter {
        from: vec!["nora@lind.example".into()],
        ..Default::default()
    };
    assert_eq!(mailbox.subjects(f), vec!["Ferry tickets", "Hotel booking"]);
}

#[rstest]
fn by_sender_domain_includes_subdomains(mailbox: Mailbox) {
    let f = Filter {
        from: vec!["news.example".into()],
        ..Default::default()
    };
    assert_eq!(mailbox.subjects(f), vec!["Old newsletter"]);
}

#[rstest]
fn by_sender_name(mailbox: Mailbox) {
    let f = Filter {
        from: vec!["paul".into()],
        ..Default::default()
    };
    assert_eq!(mailbox.subjects(f), vec!["Garden quote"]);
}

#[rstest]
fn by_project(mailbox: Mailbox) {
    let f = Filter {
        projects: vec!["Travel".into()],
        ..Default::default()
    };
    assert_eq!(mailbox.subjects(f), vec!["Ferry tickets", "Hotel booking"]);
}

#[rstest]
fn not_in_a_project(mailbox: Mailbox) {
    let f = Filter {
        not_projects: vec!["Travel".into()],
        ..Default::default()
    };
    assert_eq!(mailbox.subjects(f), vec!["Garden quote", "Old newsletter"]);
}

#[rstest]
fn by_words(mailbox: Mailbox) {
    let f = Filter {
        query: Some("invoice".into()),
        ..Default::default()
    };
    assert_eq!(mailbox.subjects(f), vec!["Garden quote"]);
}

#[rstest]
fn by_status_and_project(mailbox: Mailbox) {
    let f = Filter {
        status_in: vec!["archive".into()],
        projects: vec!["Travel".into()],
        ..Default::default()
    };
    assert_eq!(mailbox.subjects(f), vec!["Hotel booking"]);
}

#[rstest]
fn matches_come_newest_first(mailbox: Mailbox) {
    let first = mailbox
        .store
        .plan_matches(&Filter::default(), NOW)
        .unwrap()
        .remove(0);
    assert_eq!(first.subject, "Garden quote");
}

// ---- the operation log

fn op(id: &str) -> BulkOp {
    BulkOp {
        id: id.into(),
        sentence: "archive old mail".into(),
        plan: serde_json::json!({ "steps": [] }),
        summary: "Archive 2 threads".into(),
        state: "planned".into(),
        total: 2,
        done: 0,
        origin: "app".into(),
        error: None,
        created: NOW,
        finished: None,
        undone: None,
    }
}

#[rstest]
fn targets_are_kept_per_step(mailbox: Mailbox) {
    let s = &mailbox.store;
    s.save_bulk_op(
        &op("op1"),
        &[vec!["a".into(), "b".into()], vec!["c".into()]],
    )
    .unwrap();
    assert_eq!(s.bulk_op_targets("op1", 1).unwrap(), vec!["c".to_string()]);
}

#[rstest]
fn the_first_state_before_a_run_is_kept(mailbox: Mailbox) {
    let s = &mailbox.store;
    s.save_bulk_op(&op("op1"), &[]).unwrap();
    let state = |status: &str| ThreadState {
        id: "a".into(),
        unread: false,
        status: status.into(),
        priority: 0,
        projects: vec![],
        due: None,
    };
    s.save_bulk_before("op1", &[state("todo")]).unwrap();
    s.save_bulk_before("op1", &[state("archive")]).unwrap();
    assert_eq!(s.bulk_before("op1").unwrap()[0].status, "todo");
}

#[rstest]
fn planned_runs_are_not_listed(mailbox: Mailbox) {
    let s = &mailbox.store;
    s.save_bulk_op(&op("op1"), &[]).unwrap();
    assert!(s.bulk_ops(10).unwrap().is_empty());
}

#[rstest]
fn a_finished_run_is_listed_with_its_count(mailbox: Mailbox) {
    let s = &mailbox.store;
    s.save_bulk_op(&op("op1"), &[]).unwrap();
    s.set_bulk_op_state("op1", "done", 2, None).unwrap();
    assert_eq!(s.bulk_ops(10).unwrap()[0].done, 2);
}

#[rstest]
fn an_undone_run_records_when(mailbox: Mailbox) {
    let s = &mailbox.store;
    s.save_bulk_op(&op("op1"), &[]).unwrap();
    s.set_bulk_op_state("op1", "undone", 2, None).unwrap();
    assert!(s.bulk_op("op1").unwrap().undone.is_some());
}
