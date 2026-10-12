//! Seeds a Mermoz home with fictional demo mail (the validated sketches' content)
//! for UI work and screenshots without touching a real mailbox.
//!
//! MERMOZ_HOME=/some/dir cargo run --release -p mermoz-core --example seed_demo
//! then run mermozd with MERMOZ_NO_SYNC=1 and the same MERMOZ_HOME.

use mermoz_core::mime::{ParsedAttachment, ParsedMessage};
use mermoz_core::model::{Address, Draft, new_id, now_ms};
use mermoz_core::paths;
use mermoz_core::store::{IncomingMessage, Store, SyncMode, ThreadPatch};

const H: i64 = 3_600_000;

fn addr(name: &str, email: &str) -> Address {
    Address {
        name: name.into(),
        email: email.into(),
    }
}

struct Seed<'a> {
    tid: &'a str,
    from: Address,
    subject: &'a str,
    body: &'a str,
    hours_ago: i64,
    labels: &'a [&'a str],
    attachments: Vec<ParsedAttachment>,
}

fn main() {
    let store = Store::open(&paths::db_path(), &paths::blobs_dir()).expect("open store");
    let me = "you@work.example";
    let work = store.add_account(me, "gmail").unwrap();
    store
        .set_sync(&work.id, "idle", Some(0), Some(0), None)
        .unwrap();
    let personal = store.add_account("you@personal.example", "gmail").unwrap();
    store
        .set_sync(&personal.id, "idle", Some(0), Some(0), None)
        .unwrap();
    let you = addr("You", me);
    let pdf = ParsedAttachment {
        filename: "audit-report-v2.pdf".into(),
        mime: "application/pdf".into(),
        data: b"%PDF-1.4 demo".to_vec(),
    };
    let xls = ParsedAttachment {
        filename: "findings.xlsx".into(),
        mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
        data: b"demo".to_vec(),
    };

    let seeds = vec![
        (
            &work,
            Seed {
                tid: "audit",
                from: addr("Marc Petit", "marc@work.example"),
                subject: "Audit report v2 — sign-off before Friday",
                body: "Hi,\n\nHere's the first draft, comments welcome.\n\nMarc",
                hours_ago: 120,
                labels: &[],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "audit",
                from: you.clone(),
                subject: "Re: Audit report v2 — sign-off before Friday",
                body: "Thanks — a few notes on §3 and §5 inline.",
                hours_ago: 72,
                labels: &["SENT"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "audit",
                from: addr("Marc Petit", "marc@work.example"),
                subject: "Re: Audit report v2 — sign-off before Friday",
                body: "Hi,\n\nAttached is v2 with your comments addressed. The main changes are in section 3 (scope of the key-management review) and section 5 (remediation timeline).\n\nCould you sign off by Friday? I'd like to send it to the board on Monday.\n\nThanks,\nMarc",
                hours_ago: 2,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![pdf, xls],
            },
        ),
        (
            &personal,
            Seed {
                tid: "lease",
                from: addr("Léa Dubois", "lea@agency.example"),
                subject: "Lease renewal — signed copy needed today",
                body: "Hello,\n\nPlease sign the attached renewal and send it back before 6pm.\n\nLéa",
                hours_ago: 1,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "interview",
                from: addr("Sofia Reyes", "sofia@candidate.example"),
                subject: "Re: Senior engineer interview — next steps",
                body: "Thanks for the chat yesterday! Happy to do the next round any time next week.",
                hours_ago: 20,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "budget",
                from: addr("Jonas Weber", "jonas@work.example"),
                subject: "Budget sheet for Q1 planning",
                body: "Could each team lead fill in their line in the attached sheet by Wednesday?",
                hours_ago: 50,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "viewing",
                from: addr("Agence Martin", "contact@agence.example"),
                subject: "Viewing confirmed for Saturday 10:00",
                body: "Your viewing is confirmed for Saturday at 10:00.",
                hours_ago: 70,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "bank",
                from: addr("Bank", "noreply@bank.example"),
                subject: "Your October statement is ready",
                body: "Your statement is available in your online banking.",
                hours_ago: 5,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "intro",
                from: addr("Nora Lind", "nora@partner.example"),
                subject: "Partnership intro — 20 min call?",
                body: "Hi! A mutual friend suggested we talk. Would you have 20 minutes this week?",
                hours_ago: 6,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "digest",
                from: addr("Product digest", "digest@news.example"),
                subject: "Weekly product digest",
                body: "This week: three launches, two deep dives and one long read.",
                hours_ago: 30,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "offer",
                from: addr("HR team", "hr@work.example"),
                subject: "Offer letter draft for Sofia",
                body: "Draft attached, can you review the compensation section?",
                hours_ago: 26,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "claim",
                from: you.clone(),
                subject: "Insurance claim — documents sent",
                body: "Please find the requested documents attached.",
                hours_ago: 216,
                labels: &["SENT"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "redlines",
                from: you.clone(),
                subject: "Contract redlines sent to vendor",
                body: "Here are our redlines on the MSA.",
                hours_ago: 40,
                labels: &["SENT"],
                attachments: vec![],
            },
        ),
    ];
    for (i, (acc, s)) in seeds.into_iter().enumerate() {
        let msg = IncomingMessage {
            provider_id: format!("demo{i}"),
            provider_thread_id: s.tid.into(),
            label_ids: s.labels.iter().map(|l| l.to_string()).collect(),
            internal_date: now_ms() - s.hours_ago * H,
            parsed: ParsedMessage {
                from: Some(s.from),
                to: vec![addr("You", &acc.email)],
                subject: s.subject.into(),
                body_text: s.body.into(),
                message_id: Some(format!("<demo{i}@demo.example>")),
                attachments: s.attachments,
                ..Default::default()
            },
        };
        store
            .ingest(&acc.id, &acc.email, &msg, SyncMode::Initial)
            .unwrap();
    }
    // A conversation spread over three Gmail threads with Nora (the user wrote first),
    // and two automated mails from one sender: grouping shows them as one chat and one feed.
    let nora = addr("Nora Lind", "nora@agency.example");
    let sale = [
        (
            "sale1",
            you.clone(),
            nora.clone(),
            "Flat sale, next steps",
            "Hi Nora,\n\nHere is the cellar plan. The cellar is lot 14, not 12 as in the draft.\n\nThanks!",
            150,
            vec!["SENT"],
            Some(ParsedAttachment {
                filename: "cellar-plan.pdf".into(),
                mime: "application/pdf".into(),
                data: b"%PDF-1.4 demo".to_vec(),
            }),
        ),
        (
            "sale2",
            nora.clone(),
            you.clone(),
            "Signing postponed",
            "Hello,\n\nThe signing moves to the 20th: the property description must be corrected first. You can pick a slot here: https://notary.example/booking\n\nNora",
            96,
            vec!["INBOX"],
            None,
        ),
        (
            "sale3",
            nora.clone(),
            you.clone(),
            "Photo of the cellar door",
            "As promised, the cellar door after the repair.",
            20,
            vec!["INBOX", "UNREAD"],
            Some(ParsedAttachment {
                filename: "cellar-door.png".into(),
                mime: "image/png".into(),
                data: b"\x89PNG demo".to_vec(),
            }),
        ),
        (
            "bank1",
            addr("Demo Bank", "noreply@bank.example"),
            you.clone(),
            "Your October statement",
            "Your statement is ready.",
            70,
            vec!["INBOX"],
            None,
        ),
        (
            "bank2",
            addr("Demo Bank", "noreply@bank.example"),
            you.clone(),
            "Card payment of 42.00",
            "A card payment was made.",
            10,
            vec!["INBOX", "UNREAD"],
            None,
        ),
    ];
    for (i, (tid, from, to, subject, body, hours, labels, file)) in sale.into_iter().enumerate() {
        let msg = IncomingMessage {
            provider_id: format!("conv{i}"),
            provider_thread_id: tid.into(),
            label_ids: labels.iter().map(|l| l.to_string()).collect(),
            internal_date: now_ms() - hours * H,
            parsed: ParsedMessage {
                from: Some(from),
                to: vec![to],
                subject: subject.into(),
                body_text: body.into(),
                message_id: Some(format!("<conv{i}@demo.example>")),
                attachments: file.into_iter().collect(),
                ..Default::default()
            },
        };
        store.ingest(&work.id, me, &msg, SyncMode::Initial).unwrap();
    }
    for t in store.threads_to_group("pending", 1000, false).unwrap() {
        store.classify_for_grouping(&t).unwrap();
    }
    let sale_ids: Vec<String> = store
        .threads(&Default::default())
        .unwrap()
        .0
        .into_iter()
        .filter(|t| {
            ["Flat sale", "Signing postponed", "Photo of the cellar"]
                .iter()
                .any(|p| t.subject.starts_with(p))
        })
        .map(|t| t.id)
        .collect();
    store.merge_threads(&sale_ids).unwrap();

    let q4 = store.create_project("Q4 audit", Some("#5B8DEF")).unwrap();
    let hiring = store.create_project("Hiring", Some("#E0782F")).unwrap();
    let house = store.create_project("House", Some("#2E9A6B")).unwrap();
    let set = |n: &str, p: ThreadPatch| {
        let id = store.resolve_thread_id(n).unwrap();
        store.update_threads(&[id], &p).unwrap();
    };
    let tid = |subject: &str| {
        store
            .threads(&Default::default())
            .unwrap()
            .0
            .into_iter()
            .find(|t| t.subject.starts_with(subject))
            .unwrap()
            .number
    };
    set(
        &tid("Audit report"),
        ThreadPatch {
            status: Some("todo".into()),
            priority: Some(2),
            add_projects: vec![q4.id.clone()],
            due: Some(Some("2026-10-16".into())),
            ..Default::default()
        },
    );
    set(
        &tid("Lease renewal"),
        ThreadPatch {
            status: Some("todo".into()),
            priority: Some(1),
            add_projects: vec![house.id.clone()],
            ..Default::default()
        },
    );
    set(
        &tid("Re: Senior"),
        ThreadPatch {
            status: Some("todo".into()),
            priority: Some(2),
            add_projects: vec![hiring.id.clone()],
            ..Default::default()
        },
    );
    set(
        &tid("Budget sheet"),
        ThreadPatch {
            status: Some("todo".into()),
            priority: Some(2),
            ..Default::default()
        },
    );
    set(
        &tid("Viewing"),
        ThreadPatch {
            status: Some("todo".into()),
            priority: Some(3),
            add_projects: vec![house.id],
            ..Default::default()
        },
    );
    set(
        &tid("Offer letter"),
        ThreadPatch {
            status: Some("in_progress".into()),
            priority: Some(3),
            add_projects: vec![hiring.id],
            ..Default::default()
        },
    );
    set(
        &tid("Insurance claim"),
        ThreadPatch {
            status: Some("waiting".into()),
            due: Some(Some("2026-10-09".into())),
            ..Default::default()
        },
    );
    set(
        &tid("Contract redlines"),
        ThreadPatch {
            status: Some("waiting".into()),
            add_projects: vec![q4.id],
            due: Some(Some("2026-10-14".into())),
            ..Default::default()
        },
    );
    // An agent's draft waiting for approval, as in the Review sketch.
    let budget = store.resolve_thread_id(&tid("Budget sheet")).unwrap();
    let d = Draft {
        id: new_id("dft"),
        account_id: work.id.clone(),
        thread_id: Some(budget),
        to: vec!["jonas@work.example".into()],
        cc: vec!["marc@work.example".into()],
        subject: "Re: Budget sheet for Q1 planning".into(),
        body: "Hi Jonas,\n\nI've filled in our team's line: headcount stays flat, and the tooling budget goes up to cover the new CI runners. Happy to walk you through it on Thursday.\n\nBest,".into(),
        state: "draft".into(),
        origin: "agent".into(),
        created: now_ms(),
        updated: now_ms(),
        attachments: vec![],
    };
    store.create_draft(&d).unwrap();
    store.move_to_review(&d.id).unwrap();
}
