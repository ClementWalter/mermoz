//! Unit tests for the feed's kinds, sections, order and reasons.

use super::*;
use rstest::{fixture, rstest};

/// Monday 12 October 2026.
#[fixture]
fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()
}

fn thread(id: &str, status: &str) -> ThreadSummary {
    ThreadSummary {
        id: id.into(),
        number: "M-1".into(),
        account_id: "acc_work".into(),
        subject: format!("Subject {id}"),
        snippet: String::new(),
        participants: vec![],
        last_from: "Marie Lefèvre".into(),
        last_date: 1_791_700_000_000,
        unread: false,
        message_count: 1,
        has_attachments: false,
        status: status.into(),
        priority: 0,
        projects: vec![],
        due: None,
        summary: None,
        next_action: None,
        suggestion: None,
        conversation_id: String::new(),
        conversation_kind: String::new(),
        conversation_threads: 1,
    }
}

fn unit(id: &str, status: &str, j: Option<Judgment>) -> FeedUnit {
    FeedUnit {
        id: id.into(),
        threads: vec![thread(id, status)],
        judgment: j,
        texts: None,
        waiting_days: None,
        from_backlog: false,
        automated_sender: false,
    }
}

fn judged(kind: Kind) -> Judgment {
    Judgment {
        kind: Some(kind),
        automated: Some(0.1),
        ..Default::default()
    }
}

fn due_in(kind: Kind, days: i64) -> Judgment {
    Judgment {
        due: Some((today() + chrono::Duration::days(days)).to_string()),
        ..judged(kind)
    }
}

fn ai() -> HashSet<String> {
    ["acc_work".to_string()].into()
}

// ---- kind ------------------------------------------------------------------

#[rstest]
#[case::waiting_status("waiting", Some(judged(Kind::Reply)), Kind::Waiting)]
#[case::jev_kind("todo", Some(judged(Kind::Decision)), Kind::Decision)]
#[case::jev_reading("unread", Some(judged(Kind::Reading)), Kind::Reading)]
#[case::automated_reply_is_cleanup("unread", Some(Judgment { automated: Some(0.95), ..judged(Kind::Reply) }), Kind::Cleanup)]
#[case::automated_bill_stays_a_bill("unread", Some(Judgment { automated: Some(0.95), ..judged(Kind::Payment) }), Kind::Payment)]
#[case::not_judged("todo", None, Kind::Other)]
fn kind_comes_from_status_then_jev(
    #[case] status: &str,
    #[case] j: Option<Judgment>,
    #[case] want: Kind,
) {
    assert_eq!(effective_kind(&unit("t", status, j)), want);
}

#[rstest]
fn no_reply_sender_is_cleanup_without_jev() {
    let u = FeedUnit {
        automated_sender: true,
        ..unit("t", "unread", None)
    };
    assert_eq!(effective_kind(&u), Kind::Cleanup);
}

// ---- section ---------------------------------------------------------------

#[rstest]
#[case::overdue(-1, Section::Now)]
#[case::today(0, Section::Now)]
#[case::in_two_days(2, Section::Now)]
#[case::in_five_days(5, Section::Today)]
#[case::in_ten_days(10, Section::Week)]
fn deadline_sets_the_section(today: NaiveDate, #[case] days: i64, #[case] want: Section) {
    let u = unit("t", "todo", Some(due_in(Kind::Reply, days)));
    assert_eq!(section(&u, Kind::Reply, today), want);
}

#[rstest]
fn urgent_reply_is_now(today: NaiveDate) {
    let j = Judgment {
        urgency: Some(2.8),
        needs_reply: Some(0.9),
        ..judged(Kind::Reply)
    };
    assert_eq!(
        section(&unit("t", "todo", Some(j)), Kind::Reply, today),
        Section::Now
    );
}

#[rstest]
fn urgent_without_reply_is_today(today: NaiveDate) {
    let j = Judgment {
        urgency: Some(2.8),
        needs_reply: Some(0.1),
        ..judged(Kind::Other)
    };
    assert_eq!(
        section(&unit("t", "todo", Some(j)), Kind::Other, today),
        Section::Today
    );
}

#[rstest]
fn calm_mail_is_this_week(today: NaiveDate) {
    assert_eq!(
        section(
            &unit("t", "todo", Some(judged(Kind::Intro))),
            Kind::Intro,
            today
        ),
        Section::Week
    );
}

#[rstest]
fn urgent_priority_is_today(today: NaiveDate) {
    let mut u = unit("t", "todo", None);
    u.threads[0].priority = 1;
    assert_eq!(section(&u, Kind::Other, today), Section::Today);
}

#[rstest]
fn users_follow_up_date_wins_over_the_mail(today: NaiveDate) {
    let mut u = unit("t", "todo", Some(due_in(Kind::Reply, 20)));
    u.threads[0].due = Some(today.to_string());
    assert_eq!(section(&u, Kind::Reply, today), Section::Now);
}

#[rstest]
#[case::recent(1, Section::Waiting)]
#[case::overdue(6, Section::Today)]
fn waiting_rises_when_overdue(today: NaiveDate, #[case] days: i64, #[case] want: Section) {
    let j = Judgment {
        typical_reply_days: Some(2.0),
        ..Default::default()
    };
    let u = FeedUnit {
        waiting_days: Some(days),
        ..unit("t", "waiting", Some(j))
    };
    assert_eq!(section(&u, Kind::Waiting, today), want);
}

#[rstest]
#[case::cleanup(Kind::Cleanup)]
#[case::reading(Kind::Reading)]
fn automated_mail_can_wait(today: NaiveDate, #[case] kind: Kind) {
    assert_eq!(
        section(&unit("t", "unread", Some(judged(kind))), kind, today),
        Section::Later
    );
}

#[rstest]
#[case::unknown(None, 3)]
#[case::fast(Some(0.5), 2)]
#[case::slow(Some(2.0), 4)]
fn nudge_waits_twice_the_usual_delay(#[case] usual: Option<f64>, #[case] want: i64) {
    assert_eq!(nudge_after_days(usual), want);
}

// ---- Jev questions and answers ----------------------------------------------

fn dates_fixture() -> Vec<dates::DateCandidate> {
    dates::candidates(
        "Réponse avant vendredi 16 octobre à 18h, ou le 20/10",
        today(),
    )
}

#[rstest]
fn no_due_question_without_dates() {
    assert!(questions(&[], &[]).get("due").is_none());
}

#[rstest]
fn due_question_offers_each_date() {
    assert!(questions(&dates_fixture(), &[])["due"]["criteria"]["2026-10-20"].is_string());
}

#[rstest]
fn due_question_offers_none_of_these() {
    assert!(questions(&dates_fixture(), &[])["due"]["criteria"]["none"].is_string());
}

#[rstest]
fn amount_question_offers_the_written_amounts() {
    let q = questions(&[], &["126,00 €".into()]);
    assert!(q["amount"]["criteria"]["126,00 €"].is_string());
}

#[rstest]
fn urgency_is_a_score() {
    assert_eq!(questions(&[], &[])["urgency"]["type"], "score");
}

fn answers(kind: (&str, f64), due: (&str, f64), amount: (&str, f64)) -> Value {
    json!({
        "kind": { "type": "choice", "choice": kind.0, "confidence": kind.1 },
        "needs_reply": { "type": "noul", "noul": 0.9 },
        "urgency": { "type": "score", "score": 2.4, "confidence": 0.7 },
        "automated": { "type": "noul", "noul": 0.05 },
        "due": { "type": "choice", "choice": due.0, "confidence": due.1 },
        "amount": { "type": "choice", "choice": amount.0, "confidence": amount.1 },
    })
}

fn read(a: Value) -> Judgment {
    judgment(&a, &dates_fixture(), &["126,00 €".to_string()])
}

#[rstest]
#[case::confident(("payment", 0.8), Some(Kind::Payment))]
#[case::unsure(("payment", 0.3), None)]
#[case::unknown_kind(("dance", 0.9), None)]
fn kind_needs_confidence(#[case] kind: (&str, f64), #[case] want: Option<Kind>) {
    assert_eq!(read(answers(kind, ("none", 0.9), ("none", 0.9))).kind, want);
}

#[rstest]
#[case::picked(("2026-10-16", 0.8), Some("2026-10-16"))]
#[case::unsure(("2026-10-16", 0.4), None)]
#[case::none(("none", 0.9), None)]
#[case::not_a_candidate(("2026-12-25", 0.9), None)]
fn due_is_one_of_the_written_dates(#[case] due: (&str, f64), #[case] want: Option<&str>) {
    assert_eq!(
        read(answers(("reply", 0.9), due, ("none", 0.9)))
            .due
            .as_deref(),
        want
    );
}

#[rstest]
fn due_keeps_the_written_time() {
    let j = read(answers(("event", 0.9), ("2026-10-16", 0.9), ("none", 0.9)));
    assert_eq!(j.due_time.as_deref(), Some("18:00"));
}

#[rstest]
#[case::picked(("126,00 €", 0.9), Some("126,00 €"))]
#[case::invented(("999 €", 0.9), None)]
fn amount_is_one_of_the_written_amounts(#[case] a: (&str, f64), #[case] want: Option<&str>) {
    assert_eq!(
        read(answers(("payment", 0.9), ("none", 0.9), a))
            .amount
            .as_deref(),
        want
    );
}

#[rstest]
fn urgency_is_read() {
    assert_eq!(
        read(answers(("reply", 0.9), ("none", 0.9), ("none", 0.9))).urgency,
        Some(2.4)
    );
}

// ---- reasons ------------------------------------------------------------------

fn reasons_of(u: &FeedUnit) -> Vec<String> {
    reasons(u, effective_kind(u), today())
}

#[rstest]
fn backlog_cards_say_why_they_came_back() {
    let u = FeedUnit {
        from_backlog: true,
        ..unit("t", "backlog", Some(due_in(Kind::Event, 1)))
    };
    assert_eq!(
        reasons_of(&u)[0],
        "It was in your Backlog, and its date is close."
    );
}

#[rstest]
#[case::tomorrow(1, "It happens tomorrow.")]
#[case::later(4, "It happens on Fri 16 Oct.")]
fn events_say_when(#[case] days: i64, #[case] want: &str) {
    assert_eq!(
        reasons_of(&unit("t", "todo", Some(due_in(Kind::Event, days))))[0],
        want
    );
}

#[rstest]
fn deadlines_say_by_when() {
    assert_eq!(
        reasons_of(&unit("t", "todo", Some(due_in(Kind::Reply, 4))))[0],
        "The mail asks for it by Fri 16 Oct."
    );
}

#[rstest]
fn passed_dates_say_so() {
    assert_eq!(
        reasons_of(&unit("t", "todo", Some(due_in(Kind::Reply, -2))))[0],
        "The date passed 2 days ago."
    );
}

#[rstest]
fn overdue_waiting_compares_with_the_usual_delay() {
    let j = Judgment {
        typical_reply_days: Some(2.0),
        ..Default::default()
    };
    let u = FeedUnit {
        waiting_days: Some(6),
        ..unit("t", "waiting", Some(j))
    };
    assert_eq!(
        reasons_of(&u)[0],
        "No answer for 6 days; they usually answer within 2 days."
    );
}

#[rstest]
fn waiting_leads_with_the_wait_even_with_a_date() {
    let mut u = FeedUnit {
        waiting_days: Some(4),
        ..unit("t", "waiting", None)
    };
    u.threads[0].due = Some("2026-10-09".into());
    assert_eq!(reasons_of(&u)[0], "You wrote last, 4 days ago.");
}

#[rstest]
fn payments_say_how_much() {
    let j = Judgment {
        amount: Some("126,00 €".into()),
        ..judged(Kind::Payment)
    };
    assert_eq!(
        reasons_of(&unit("t", "todo", Some(j)))[0],
        "There is 126,00 € to pay."
    );
}

#[rstest]
fn reasons_never_show_numbers_from_the_model() {
    let j = Judgment {
        needs_reply: Some(0.93),
        urgency: Some(2.7),
        ..judged(Kind::Reply)
    };
    let all = reasons_of(&unit("t", "todo", Some(j))).join(" ");
    assert!(!all.contains("0.9") && !all.contains('%'));
}

#[rstest]
fn a_card_always_has_a_reason() {
    assert_eq!(
        reasons_of(&unit("t", "todo", None)),
        vec!["It is still open in your mail."]
    );
}

// ---- assembly -------------------------------------------------------------------

fn feed(units: Vec<FeedUnit>) -> Feed {
    assemble(units, today(), &ai())
}

fn ids(f: &Feed) -> Vec<String> {
    f.sections
        .iter()
        .flat_map(|s| s.cards.iter().map(|c| c.id.clone()))
        .collect()
}

#[rstest]
fn sections_come_in_order() {
    let f = feed(vec![
        unit("week", "todo", Some(judged(Kind::Intro))),
        unit("now", "todo", Some(due_in(Kind::Reply, 0))),
    ]);
    assert_eq!(f.sections[0].key, Section::Now);
}

#[rstest]
fn empty_sections_are_left_out() {
    let f = feed(vec![unit("now", "todo", Some(due_in(Kind::Reply, 0)))]);
    assert_eq!(f.sections.len(), 1);
}

#[rstest]
fn earliest_deadline_first() {
    let f = feed(vec![
        unit("later", "todo", Some(due_in(Kind::Reply, 2))),
        unit("sooner", "todo", Some(due_in(Kind::Reply, 1))),
    ]);
    assert_eq!(ids(&f), vec!["sooner", "later"]);
}

#[rstest]
fn people_you_answer_come_first() {
    let mut close = judged(Kind::Reply);
    close.sender_weight = 0.9;
    close.urgency = Some(2.0);
    let mut far = judged(Kind::Reply);
    far.urgency = Some(2.0);
    let f = feed(vec![
        unit("far", "todo", Some(far)),
        unit("close", "todo", Some(close)),
    ]);
    assert_eq!(ids(&f), vec!["close", "far"]);
}

#[rstest]
fn automated_mail_is_one_batch() {
    let f = feed(vec![
        unit("a", "unread", Some(judged(Kind::Cleanup))),
        unit("b", "unread", Some(judged(Kind::Cleanup))),
    ]);
    assert_eq!(ids(&f), vec!["batch:cleanup"]);
}

#[rstest]
fn batch_lists_its_threads() {
    let f = feed(vec![
        unit("a", "unread", Some(judged(Kind::Cleanup))),
        unit("b", "unread", Some(judged(Kind::Cleanup))),
    ]);
    assert_eq!(f.sections[0].cards[0].members.len(), 2);
}

#[rstest]
fn batch_title_counts_its_threads() {
    let f = feed(vec![
        unit("a", "unread", Some(judged(Kind::Reading))),
        unit("b", "unread", Some(judged(Kind::Reading))),
    ]);
    assert_eq!(
        f.sections[0].cards[0].title,
        "2 newsletters to read or archive"
    );
}

#[rstest]
fn newsletters_and_clean_up_are_separate_batches() {
    let f = feed(vec![
        unit("a", "unread", Some(judged(Kind::Reading))),
        unit("b", "unread", Some(judged(Kind::Cleanup))),
    ]);
    assert_eq!(ids(&f), vec!["batch:cleanup", "batch:reading"]);
}

#[rstest]
fn title_falls_back_to_the_subject() {
    let f = feed(vec![unit("a", "todo", None)]);
    assert_eq!(f.sections[0].cards[0].title, "Subject a");
}

#[rstest]
fn title_comes_from_the_ai_when_written() {
    let u = FeedUnit {
        texts: Some(Texts {
            title: "Choose the kitchen tiles".into(),
            ..Default::default()
        }),
        ..unit("a", "todo", None)
    };
    assert_eq!(
        feed(vec![u]).sections[0].cards[0].title,
        "Choose the kitchen tiles"
    );
}

#[rstest]
fn texts_are_wanted_when_the_account_allows_an_ai() {
    assert!(feed(vec![unit("a", "todo", None)]).sections[0].cards[0].texts_wanted);
}

#[rstest]
fn texts_are_not_wanted_without_an_ai() {
    let f = assemble(vec![unit("a", "todo", None)], today(), &HashSet::new());
    assert!(!f.sections[0].cards[0].texts_wanted);
}

#[rstest]
fn counts_judged_threads() {
    let f = feed(vec![
        unit("a", "todo", None),
        unit("b", "todo", Some(judged(Kind::Reply))),
    ]);
    assert_eq!(f.judged, 1);
}

// ---- words ----------------------------------------------------------------------------

#[rstest]
#[case::today("2026-10-12", "today")]
#[case::tomorrow("2026-10-13", "tomorrow")]
#[case::past("2026-10-09", "3 days ago")]
#[case::later("2026-10-16", "Fri 16 Oct")]
fn due_in_words(today: NaiveDate, #[case] date: &str, #[case] want: &str) {
    assert_eq!(due_words(date, today), want);
}

#[rstest]
#[case::tomorrow("rendez-vous demain", true)]
#[case::next_month("rendez-vous le 20 novembre", false)]
#[case::past("c'était le 1er octobre", false)]
fn backlog_is_judged_only_when_a_date_is_close(
    today: NaiveDate,
    #[case] text: &str,
    #[case] want: bool,
) {
    assert_eq!(
        backlog_worth_judging(&dates::candidates(text, today), today),
        want
    );
}
