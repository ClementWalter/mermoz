//! The demo account: a rich, fictional, local-only mailbox that looks alive offline.

use super::*;
use crate::store::{ColumnScope, ThreadFilter};
use rstest::{fixture, rstest};
use std::io::Read;

/// A fixed moment, so every case sees the same mailbox.
const NOW: i64 = 1_791_000_000_000;

struct Env {
    _dir: tempfile::TempDir,
    store: Store,
    acc: Account,
}

#[fixture]
fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
    let acc = add(&store).unwrap();
    Env {
        _dir: dir,
        store,
        acc,
    }
}

fn all_threads(s: &Store) -> Vec<crate::model::ThreadSummary> {
    s.threads(&ThreadFilter {
        limit: 2000,
        ..Default::default()
    })
    .unwrap()
    .0
}

fn png_pixels(png: &[u8]) -> usize {
    // IHDR is the first chunk; IDAT the second.
    let w = u32::from_be_bytes(png[16..20].try_into().unwrap()) as usize;
    let h = u32::from_be_bytes(png[20..24].try_into().unwrap()) as usize;
    let len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
    let mut raw = vec![];
    flate2::read::ZlibDecoder::new(&png[41..41 + len])
        .read_to_end(&mut raw)
        .unwrap();
    assert_eq!(raw.len(), (w * 3 + 1) * h, "scanlines");
    w * h
}

#[rstest]
fn a_banner_is_a_valid_png() {
    let b = art::banner([10, 20, 30], [200, 210, 220], [90, 100, 200], 3);
    assert_eq!(png_pixels(&b), 600 * 160);
}

#[rstest]
fn a_photo_is_a_valid_png() {
    let p = art::tiles([196, 110, 74], [214, 132, 92], [236, 224, 210]);
    assert_eq!(png_pixels(&p), 480 * 320);
}

#[rstest]
fn pictures_compress() {
    let b = art::banner([10, 20, 30], [200, 210, 220], [90, 100, 200], 3);
    assert!(b.len() < 60_000, "{} bytes", b.len());
}

#[rstest]
fn a_pdf_is_complete() {
    let p = art::pdf("Invoice", &["Total 12 EUR".into()]);
    assert!(p.starts_with(b"%PDF-1.4") && p.ends_with(b"%%EOF\n"));
}

#[rstest]
fn the_mailbox_is_the_same_every_time() {
    let subjects = |now| {
        mailbox(now)
            .0
            .iter()
            .map(|m| (m.subject.clone(), m.text.clone(), m.at))
            .collect::<Vec<_>>()
    };
    assert_eq!(subjects(NOW), subjects(NOW));
}

#[rstest]
fn the_mailbox_is_rich() {
    let threads: std::collections::BTreeSet<String> =
        mailbox(NOW).0.into_iter().map(|m| m.thread).collect();
    assert!((150..=300).contains(&threads.len()), "{}", threads.len());
}

#[rstest]
fn every_address_is_fictional() {
    let (mails, _) = mailbox(NOW);
    let real: Vec<String> = mails
        .iter()
        .flat_map(|m| std::iter::once(&m.from).chain(&m.to).chain(&m.cc))
        .map(|a| a.email.clone())
        .filter(|e| !e.ends_with(".example"))
        .collect();
    assert!(real.is_empty(), "{real:?}");
}

#[rstest]
fn nothing_is_dated_in_the_future() {
    let (mails, _) = mailbox(NOW);
    assert!(mails.iter().all(|m| m.at < NOW));
}

#[rstest]
fn every_plan_names_a_thread_of_the_mailbox() {
    let (mails, plans) = mailbox(NOW);
    let threads: std::collections::BTreeSet<&str> =
        mails.iter().map(|m| m.thread.as_str()).collect();
    assert!(plans.iter().all(|p| threads.contains(p.thread.as_str())));
}

#[rstest]
fn the_account_is_local(env: Env) {
    assert_eq!(env.acc.provider, PROVIDER);
}

#[rstest]
fn adding_twice_keeps_one_demo_account(env: Env) {
    add(&env.store).unwrap();
    assert_eq!(env.store.accounts().unwrap().len(), 1);
}

#[rstest]
fn the_demo_fills_the_lists(env: Env) {
    assert!(all_threads(&env.store).len() >= 150);
}

#[rstest]
fn some_mail_is_unread(env: Env) {
    assert!(all_threads(&env.store).iter().filter(|t| t.unread).count() >= 10);
}

#[rstest]
fn mail_is_filed_in_folders(env: Env) {
    let names: Vec<String> = env
        .store
        .projects()
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert!(names.contains(&"Clients/Northbeam".to_string()));
}

#[rstest]
fn some_mail_is_put_off(env: Env) {
    let open =
        |t: &crate::model::ThreadSummary| crate::store::FEED_STATUSES.contains(&t.status.as_str());
    let all_open = all_threads(&env.store).iter().filter(|t| open(t)).count() as i64;
    // Mail put off with Later leaves the open columns until it comes back.
    let shown = env
        .store
        .threads(&ThreadFilter {
            status: Some(
                crate::store::FEED_STATUSES
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            limit: 2000,
            ..Default::default()
        })
        .unwrap()
        .1;
    assert!(shown < all_open, "shown {shown} of {all_open}");
}

#[rstest]
fn feed_cards_come_with_their_words(env: Env) {
    let today = chrono::Local::now().date_naive();
    let units = env
        .store
        .feed_units(&ColumnScope::default(), today, now_ms())
        .unwrap();
    assert!(units.iter().filter(|u| u.texts.is_some()).count() >= 15);
}

#[rstest]
fn suggestion_chips_are_ready(env: Env) {
    let with_chip = all_threads(&env.store)
        .iter()
        .filter(|t| t.suggestion.is_some())
        .count();
    assert!(with_chip >= 5, "{with_chip}");
}

#[rstest]
fn conversations_have_a_summary(env: Env) {
    let with_gist = all_threads(&env.store)
        .iter()
        .filter(|t| t.summary.is_some())
        .count();
    assert!(with_gist >= 5, "{with_gist}");
}

#[rstest]
fn a_delivery_failed(env: Env) {
    let t = all_threads(&env.store)
        .into_iter()
        .find(|t| t.subject.starts_with("Prototype link"))
        .unwrap();
    let msgs = env.store.messages(&t.id).unwrap();
    assert!(msgs.iter().any(|m| m.is_bounce));
}

#[rstest]
fn newsletters_carry_their_pictures(env: Env) {
    let t = all_threads(&env.store)
        .into_iter()
        .find(|t| t.subject.starts_with("Onboarding that respects"))
        .unwrap();
    let m = &env.store.messages(&t.id).unwrap()[0];
    assert_eq!(env.store.inline_parts(&m.id).unwrap().len(), 1);
}

#[rstest]
fn its_mail_never_goes_to_an_ai(env: Env) {
    assert!(
        env.store
            .provider_for_account(&env.acc.id)
            .unwrap()
            .is_none()
    );
}

#[rstest]
fn its_unread_mail_is_never_routed(env: Env) {
    assert!(env.store.threads_to_route(1000).unwrap().is_empty());
}

#[rstest]
fn its_mail_is_never_judged(env: Env) {
    assert!(
        env.store
            .threads_to_judge("2026-01-01", 1000)
            .unwrap()
            .is_empty()
    );
}

#[rstest]
fn fresh_mail_is_kept(env: Env) {
    assert!(!refresh_if_stale(&env.store, now_ms()).unwrap());
}

#[rstest]
fn stale_mail_is_generated_again(env: Env) {
    let later = now_ms() + (FRESH_FOR_DAYS + 1) * DAY;
    assert!(refresh_if_stale(&env.store, later).unwrap());
}

#[rstest]
fn regenerating_keeps_the_account_hidden(env: Env) {
    env.store.set_account_hidden(&env.acc.id, true).unwrap();
    refresh_if_stale(&env.store, now_ms() + (FRESH_FOR_DAYS + 1) * DAY).unwrap();
    assert!(env.store.account(&env.acc.id).unwrap().hidden);
}

#[rstest]
fn regenerating_replaces_the_mail(env: Env) {
    let before = all_threads(&env.store).len();
    refresh_if_stale(&env.store, now_ms() + (FRESH_FOR_DAYS + 1) * DAY).unwrap();
    assert_eq!(all_threads(&env.store).len(), before);
}

/// A reply drafted on Priya's thread.
fn reply(env: &Env) -> (String, Draft) {
    let t = all_threads(&env.store)
        .into_iter()
        .find(|t| t.subject.starts_with("Application: Senior"))
        .unwrap();
    let d = env
        .store
        .create_draft(&Draft {
            id: new_id("dft"),
            account_id: env.acc.id.clone(),
            thread_id: Some(t.id.clone()),
            to: vec!["priya.nair@mail.example".into()],
            cc: vec![],
            subject: "Re: Application: Senior Product Designer".into(),
            body: "Thursday 10am works, see you then!".into(),
            state: "draft".into(),
            origin: "user".into(),
            created: now_ms(),
            updated: now_ms(),
            attachments: vec![],
            reply_to_message_id: None,
        })
        .unwrap();
    (t.id, d)
}

#[rstest]
fn a_sent_reply_lands_in_the_conversation(env: Env) {
    let (tid, d) = reply(&env);
    deliver(&env.store, &env.acc, &d, None).unwrap();
    let last = env.store.messages(&tid).unwrap().pop().unwrap();
    assert!(last.is_from_me && last.body_text.contains("Thursday 10am"));
}

#[rstest]
fn a_sent_reply_waits_on_the_other_side(env: Env) {
    let (tid, d) = reply(&env);
    deliver(&env.store, &env.acc, &d, None).unwrap();
    assert_eq!(env.store.thread(&tid).unwrap().status, "waiting");
}

#[rstest]
fn a_sent_draft_is_spent(env: Env) {
    let (_, d) = reply(&env);
    deliver(&env.store, &env.acc, &d, None).unwrap();
    assert_eq!(env.store.draft(&d.id).unwrap().state, "sent");
}
