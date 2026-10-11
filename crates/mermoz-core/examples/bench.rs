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
            let from = people[rng.random_range(0..people.len())].clone();
            let labels: Vec<String> = if rng.random_bool(0.1) {
                vec!["INBOX".into()]
            } else {
                vec![]
            };
            batch.push(IncomingMessage {
                provider_id: format!("m{i}"),
                provider_thread_id: format!("t{thread}"),
                label_ids: labels,
                internal_date: now - (n - i) as i64 * 60_000,
                parsed: ParsedMessage {
                    from: Some(from),
                    to: vec![Address {
                        name: "Me".into(),
                        email: "me@example.com".into(),
                    }],
                    subject: sentence(&mut rng, 6),
                    body_text: sentence(&mut rng, 120),
                    message_id: Some(format!("<m{i}@x>")),
                    ..Default::default()
                },
            });
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
    let mut list = time(50, || {
        store
            .threads(&ThreadFilter {
                status: Some(vec!["backlog".into()]),
                limit: 300,
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
        store.messages(id).unwrap();
    });
    checks.push(Check {
        name: "open thread",
        p95: pct(&mut open, 0.95),
        budget: Duration::from_millis(16),
    });
    let mut counts = time(50, || {
        store.counts().unwrap();
    });
    checks.push(Check {
        name: "sidebar counts",
        p95: pct(&mut counts, 0.95),
        budget: Duration::from_millis(16),
    });
    for (name, q) in [
        ("search rare word", "w1a2b"),
        ("search common word", "meeting"),
        ("search two words", "report w3f"),
        ("search from:", "from:person"),
    ] {
        let mut s = time(30, || {
            store.search(q, 50).unwrap();
        });
        checks.push(Check {
            name,
            p95: pct(&mut s, 0.95),
            budget: Duration::from_millis(50),
        });
    }

    let mut failed = false;
    for c in &checks {
        let ok = c.p95 <= c.budget;
        failed |= !ok;
        eprintln_info(&format!(
            "{} {:<24} p95 {:>8.2} ms  (budget {} ms)",
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
