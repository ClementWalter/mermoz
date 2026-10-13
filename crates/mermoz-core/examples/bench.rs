//! Speed-bar benchmark: seeds N synthetic messages through the real ingest path,
//! then times the hot reads against the budgets in SPEC.md. Exits non-zero on a miss.
//!
//! cargo run --release -p mermoz-core --example bench -- [messages] [db-dir]

use mermoz_core::mime::ParsedMessage;
use mermoz_core::model::Address;
use mermoz_core::store::{IncomingMessage, Store, SyncMode, ThreadFilter};
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::time::{Duration, Instant};

const COMMON: &[&str] = &[
    "the", "and", "to", "of", "a", "in", "for", "is", "on", "that", "with", "please", "thanks",
    "meeting", "report", "update", "review", "next", "week", "team",
];

fn word(rng: &mut StdRng) -> String {
    if rng.random_bool(0.5) {
        return COMMON[rng.random_range(0..COMMON.len())].to_string();
    }
    // ~20k distinct pseudo-words gives a realistic long tail.
    let n: u32 = rng.random_range(0..20_000);
    format!("w{n:x}")
}

fn sentence(rng: &mut StdRng, n: usize) -> String {
    (0..n).map(|_| word(rng)).collect::<Vec<_>>().join(" ")
}

/// One synthetic email: a tenth in the inbox, ~10 KB of HTML on a third of them
/// (newsletters, rich replies).
fn message(
    rng: &mut StdRng,
    people: &[Address],
    id: &str,
    thread: &str,
    date: i64,
) -> IncomingMessage {
    let from = people[rng.random_range(0..people.len())].clone();
    let labels: Vec<String> = if rng.random_bool(0.1) {
        vec!["INBOX".into()]
    } else {
        vec![]
    };
    IncomingMessage {
        provider_id: id.into(),
        provider_thread_id: thread.into(),
        label_ids: labels,
        internal_date: date,
        parsed: ParsedMessage {
            from: Some(from),
            to: vec![Address {
                name: "Me".into(),
                email: "me@example.com".into(),
            }],
            subject: sentence(rng, 6),
            body_text: sentence(rng, 120),
            body_html: rng
                .random_bool(0.3)
                .then(|| format!("<html><body><p>{}</p></body></html>", sentence(rng, 1500))),
            message_id: Some(format!("<{id}@x>")),
            ..Default::default()
        },
    }
}

fn pct(v: &mut [Duration], p: f64) -> Duration {
    v.sort();
    v[((v.len() as f64 - 1.0) * p) as usize]
}

struct Check {
    name: &'static str,
    p95: Duration,
    budget: Duration,
}

fn time<F: FnMut()>(iters: usize, mut f: F) -> Vec<Duration> {
    (0..iters)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(500_000);
    let tmp = tempfile::tempdir().unwrap();
    let dir = args
        .get(2)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| tmp.path().to_path_buf());
    let store = Store::open(&dir.join("bench.sqlite"), &dir.join("blobs")).unwrap();
    let acc = store.add_account("me@example.com", "gmail").unwrap().id;
    let mut rng = StdRng::seed_from_u64(42);
    let people: Vec<Address> = (0..2000)
        .map(|i| Address {
            name: format!("Person {i}"),
            email: format!("p{i}@corp{}.example", i % 50),
        })
        .collect();

    let existing = store.threads(&ThreadFilter::default()).unwrap().1;
    if existing == 0 {
        let t0 = Instant::now();
        let now = 1_791_700_000_000i64;
        let mut batch = Vec::with_capacity(1000);
        for i in 0..n {
            // ~3 messages per thread on average.
            let thread = i / 3 + rng.random_range(0..2);
            batch.push(message(
                &mut rng,
                &people,
                &format!("m{i}"),
                &format!("t{thread}"),
                now - (n - i) as i64 * 60_000,
            ));
            if batch.len() == 1000 {
                store
                    .ingest_many(&acc, "me@example.com", &batch, SyncMode::Initial)
                    .unwrap();
                batch.clear();
            }
        }
        store
            .ingest_many(&acc, "me@example.com", &batch, SyncMode::Initial)
            .unwrap();
        eprintln_info(&format!(
            "seeded {n} messages in {:.1}s",
            t0.elapsed().as_secs_f64()
        ));
    }

    let (threads, total) = store
        .threads(&ThreadFilter {
            limit: 2000,
            ..Default::default()
        })
        .unwrap();
    eprintln_info(&format!("{total} threads"));
    let ids: Vec<String> = threads.iter().map(|t| t.id.clone()).collect();

    let mut checks = vec![];
    // The reading pipeline alone (what ingest pays per message, off the write lock), on
    // mail as clients write it: half replies (CRLF, an attribution, the quoted history,
    // a signature), a third with HTML.
    let samples: Vec<IncomingMessage> = (0..2000)
        .map(|k| {
            let mut m = message(&mut rng, &people, &format!("r{k}"), "r", 0);
            if k % 2 == 0 {
                m.parsed.body_text = format!(
                    "{}\r\n\r\nNora Lind\r\n+33 6 12 34 56 78\r\n\r\nLe lun. 12 janv. 2026 à 11:54, Sam <sam@x.example> a écrit\u{a0}:\r\n> {}\r\n> \r\n> {}\r\n",
                    sentence(&mut rng, 60),
                    sentence(&mut rng, 40),
                    sentence(&mut rng, 40)
                );
            }
            m
        })
        .collect();
    let mut k = 0;
    let mut read_one = time(samples.len(), || {
        let p = &samples[k].parsed;
        k += 1;
        mermoz_core::reading::read(&mermoz_core::reading::Mail {
            subject: &p.subject,
            text: &p.body_text,
            html: p.body_html.as_deref(),
            listed: false,
        });
    });
    let mean = read_one.iter().sum::<Duration>() / read_one.len() as u32;
    eprintln_info(&format!(
        "reading: {:.0} µs per message on average",
        mean.as_secs_f64() * 1e6
    ));
    checks.push(Check {
        name: "read one message",
        p95: pct(&mut read_one, 0.95),
        budget: Duration::from_millis(2),
    });
    let mut list = time(50, || {
        store
            .threads(&ThreadFilter {
                status: Some(vec!["backlog".into()]),
                limit: 300,
                conversations: true,
                ..Default::default()
            })
            .unwrap();
    });
    checks.push(Check {
        name: "threads.list (300 rows)",
        p95: pct(&mut list, 0.95),
        budget: Duration::from_millis(16),
    });
    let mut i = 0;
    let mut open = time(200, || {
        let id = &ids[i % ids.len()];
        i += 1;
        store.thread(id).unwrap();
        let msgs = store.conversation_messages(id).unwrap();
        store.conversation(id, &msgs).unwrap();
    });
    checks.push(Check {
        name: "open thread",
        p95: pct(&mut open, 0.95),
        budget: Duration::from_millis(16),
    });
    // A sender with hundreds of threads (notifications, a long relationship): the whole
    // conversation opens at once.
    // Picked by thread number, which later runs' sync batches never shift.
    let members: Vec<String> = (1000..1300)
        .map(|k| store.resolve_thread_id(&format!("M-{k}")).unwrap())
        .collect();
    let big = &members[0];
    if store.conversation_thread_ids(big).unwrap().len() < 2 {
        store.merge_threads(&members).unwrap();
    }
    let big_n = store.conversation_thread_ids(big).unwrap().len();
    let mut open_big = time(20, || {
        let msgs = store.conversation_messages(big).unwrap();
        store.conversation(big, &msgs).unwrap();
    });
    eprintln_info(&format!("big conversation: {big_n} threads"));
    checks.push(Check {
        name: "open 300-thread conversation",
        p95: pct(&mut open_big, 0.95),
        budget: Duration::from_millis(16),
    });
    let mut counts = time(50, || {
        store.counts(true).unwrap();
    });
    checks.push(Check {
        name: "sidebar counts",
        p95: pct(&mut counts, 0.95),
        budget: Duration::from_millis(16),
    });
    // Counts right after a write that moves one, as when reading an email: a full recount.
    let mut recount: Vec<Duration> = (0..50)
        .map(|k| {
            store
                .update_threads(
                    &ids[..1],
                    &mermoz_core::store::ThreadPatch {
                        // Alternates, so every write really moves a count.
                        priority: Some(if k % 2 == 0 { 3 } else { 0 }),
                        ..Default::default()
                    },
                )
                .unwrap();
            let t = Instant::now();
            store.counts(true).unwrap();
            t.elapsed()
        })
        .collect();
    checks.push(Check {
        name: "sidebar counts after a change",
        p95: pct(&mut recount, 0.95),
        budget: Duration::from_millis(16),
    });
    // A busy feed: 150 open threads on top of the mailbox's own open ones.
    store
        .update_threads(
            &ids[..150.min(ids.len())],
            &mermoz_core::store::ThreadPatch {
                status: Some("todo".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut feed = time(50, || {
        let today = mermoz_core::feed::today();
        let units = store
            .feed_units(&Default::default(), today, mermoz_core::model::now_ms())
            .unwrap();
        let mut f = mermoz_core::feed::assemble(units, today, &Default::default());
        store.complete_feed(&mut f).unwrap();
    });
    checks.push(Check {
        name: "feed.list",
        p95: pct(&mut feed, 0.95),
        budget: Duration::from_millis(16),
    });
    for (name, q) in [
        ("search rare word", "w1a2b"),
        ("search common word", "meeting"),
        ("search two words", "report w3f"),
        ("search from:", "from:person"),
        ("search first keystroke", "m"),
        ("search from: first letter", "from:p"),
        ("search two long words", "meeting report"),
        ("search finished words", "meeting report "),
    ] {
        let mut s = time(30, || {
            // As the app asks while the query is typed.
            store.search(q, 50, true).unwrap();
        });
        checks.push(Check {
            name,
            p95: pct(&mut s, 0.95),
            budget: Duration::from_millis(50),
        });
    }

    // Sync while the user reads: 100-message batches (the daemon's chunk) into new and
    // existing threads, and the list read meanwhile. Ids are unique per run.
    let run = mermoz_core::model::now_ms();
    let batches: Vec<Vec<IncomingMessage>> = (0..20)
        .map(|b| {
            (0..100)
                .map(|k| {
                    let thread = if k % 2 == 0 {
                        format!("s{run}-{b}-{k}")
                    } else {
                        format!("t{}", rng.random_range(0..n / 3))
                    };
                    let mut m =
                        message(&mut rng, &people, &format!("s{run}-{b}-{k}"), &thread, run);
                    // Out of the inbox, so reruns don't keep growing the feed.
                    m.label_ids.clear();
                    m
                })
                .collect()
        })
        .collect();
    let syncing = std::sync::atomic::AtomicBool::new(true);
    let (mut writes, mut reads, mut edits) = (vec![], vec![], vec![]);
    std::thread::scope(|s| {
        // The daemon's checkpointer, once a second.
        s.spawn(|| {
            while syncing.load(std::sync::atomic::Ordering::Relaxed) {
                store.checkpoint().unwrap();
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        let writer = s.spawn(|| {
            let t = batches
                .iter()
                .map(|b| {
                    let t = Instant::now();
                    store
                        .ingest_many(&acc, "me@example.com", b, SyncMode::Incremental)
                        .unwrap();
                    t.elapsed()
                })
                .collect::<Vec<_>>();
            syncing.store(false, std::sync::atomic::Ordering::Relaxed);
            t
        });
        while syncing.load(std::sync::atomic::Ordering::Relaxed) {
            let t = Instant::now();
            store
                .threads(&ThreadFilter {
                    status: Some(vec!["backlog".into()]),
                    limit: 300,
                    conversations: true,
                    ..Default::default()
                })
                .unwrap();
            reads.push(t.elapsed());
            // A status change waits for the write lock behind the sync batch.
            let t = Instant::now();
            store
                .update_threads(
                    &ids[..1],
                    &mermoz_core::store::ThreadPatch {
                        priority: Some(reads.len() as i64 % 3),
                        ..Default::default()
                    },
                )
                .unwrap();
            edits.push(t.elapsed());
            std::thread::sleep(Duration::from_millis(2));
        }
        writes = writer.join().unwrap();
    });
    checks.push(Check {
        // Background throughput, its checkpoint included; clicks wait only on the lock.
        name: "sync batch (100 messages)",
        p95: pct(&mut writes, 0.95),
        budget: Duration::from_millis(200),
    });
    checks.push(Check {
        name: "threads.list during sync",
        p95: pct(&mut reads, 0.95),
        budget: Duration::from_millis(16),
    });
    // Optimistic in the app, so the user never sees it; it waits one batch at most.
    checks.push(Check {
        name: "status change during sync",
        p95: pct(&mut edits, 0.95),
        budget: Duration::from_millis(100),
    });

    let mut failed = false;
    for c in &checks {
        let ok = c.p95 <= c.budget;
        failed |= !ok;
        eprintln_info(&format!(
            "{} {:<29} p95 {:>8.2} ms  (budget {} ms)",
            if ok { "ok  " } else { "MISS" },
            c.name,
            c.p95.as_secs_f64() * 1000.0,
            c.budget.as_millis()
        ));
    }
    if failed {
        std::process::exit(1);
    }
}

/// Benchmark output goes to stderr so stdout stays clean for piping.
fn eprintln_info(s: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{s}");
}
