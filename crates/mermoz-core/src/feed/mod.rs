//! The Feed: everything open, as an ordered stream of cards, one thing to do each.
//!
//! One card per conversation (Gmail threads read as one chat), shown as its newest thread.
//! Code decides what is in the feed, in which section and in what order, so the order is
//! stable and every position can be explained. Jev supplies the judgments, one call on a
//! conversation's newest thread (card kind, needs a reply, urgency, the deadline and the
//! amount among those written in the mail); the account's AI only writes words (title,
//! why now, drafts), lazily.

pub mod dates;
pub mod later;
pub mod money;
pub mod texts;

use crate::model::ThreadSummary;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub use texts::Texts;

/// What a card asks of the user; decides its body and primary action in the app.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Reply,
    Decision,
    Sign,
    Promise,
    Event,
    Payment,
    Intro,
    Waiting,
    Cleanup,
    Reading,
    /// Open, but none of the above (or not judged yet).
    Other,
}

impl Kind {
    /// The kinds Jev chooses from, with what each means. `waiting` comes from the status.
    const CRITERIA: &[(Kind, &str, &str)] = &[
        (
            Kind::Reply,
            "reply",
            "Someone asks the user a question or expects a written answer.",
        ),
        (
            Kind::Decision,
            "decision",
            "Someone asks the user to choose between options or to approve something.",
        ),
        (
            Kind::Sign,
            "sign",
            "The user has to sign, review or fill in a document.",
        ),
        (
            Kind::Promise,
            "promise",
            "The user's own last message promises to send or do something, and it is still to do.",
        ),
        (
            Kind::Event,
            "event",
            "It announces or confirms a meeting, appointment or event the user should attend.",
        ),
        (
            Kind::Payment,
            "payment",
            "The user has to pay a bill, an invoice or a fee (not an automatic debit already taken).",
        ),
        (
            Kind::Intro,
            "intro",
            "Someone introduces the user to a new person, or a new person writes for the first time.",
        ),
        (
            Kind::Reading,
            "reading",
            "A newsletter or article to read; nothing to do.",
        ),
        (
            Kind::Cleanup,
            "cleanup",
            "An automated notification, receipt, confirmation or promotion; nothing to do.",
        ),
        (Kind::Other, "other", "None of these."),
    ];

    fn from_key(k: &str) -> Option<Kind> {
        Self::CRITERIA
            .iter()
            .find(|(_, key, _)| *key == k)
            .map(|(kind, ..)| *kind)
    }
}

/// Below these, a judgment is ignored and the card falls back to plain facts.
pub const MIN_KIND_CONFIDENCE: f64 = 0.5;
pub const MIN_PICK_CONFIDENCE: f64 = 0.6;

/// Jev's verdict on one thread, kept until new mail arrives in it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Judgment {
    pub kind: Option<Kind>,
    /// Probability the user has to write back.
    pub needs_reply: Option<f64>,
    /// 0 = nothing to do … 3 = needs action today.
    pub urgency: Option<f64>,
    /// The deadline or event day, among the dates written in the thread ("YYYY-MM-DD").
    pub due: Option<String>,
    /// The time written next to it ("18:30").
    pub due_time: Option<String>,
    /// The amount to pay, exactly as written.
    pub amount: Option<String>,
    pub automated: Option<f64>,
    /// Share of past threads with these people the user wrote in (0–1).
    pub sender_weight: f64,
    /// How many days these people usually take to answer the user.
    pub typical_reply_days: Option<f64>,
}

const URGENCY_LEVELS: &[&str] = &[
    "Nothing for the user to do.",
    "The user can handle it within a few weeks.",
    "The user should handle it within a few days.",
    "The user must act today.",
];

/// Everything Jev is asked about a thread for the feed, in one call. `dates` and
/// `amounts` are what the code found in the mail: Jev picks, it never writes a value.
pub fn questions(dates: &[dates::DateCandidate], amounts: &[String]) -> Value {
    let kinds: serde_json::Map<String, Value> = Kind::CRITERIA
        .iter()
        .map(|(_, k, d)| (k.to_string(), json!(d)))
        .collect();
    let mut q = json!({
        "kind": { "type": "choice", "criteria": kinds, "instructions":
            "What does this email thread ask of the user (the recipient, shown as \"me\")?" },
        "needs_reply": { "type": "noul", "instructions":
            "Does the user still need to write back in this thread?" },
        "urgency": { "type": "score", "criteria": URGENCY_LEVELS, "instructions":
            "How soon does the user need to act on this thread?" },
        "automated": { "type": "noul", "instructions":
            "Was this thread sent by an automated system (newsletter, notification, receipt, marketing) rather than a person writing to the user?" },
    });
    if !dates.is_empty() {
        let mut c: serde_json::Map<String, Value> = dates
            .iter()
            .map(|d| {
                (
                    d.date.to_string(),
                    json!(format!(
                        "\"{}\", that is {}",
                        d.span,
                        d.date.format("%A %-d %B %Y")
                    )),
                )
            })
            .collect();
        c.insert(
            "none".into(),
            json!("None of these dates is a deadline or an event for the user."),
        );
        q["due"] = json!({ "type": "choice", "criteria": c, "instructions":
            "Which date is the deadline the user must act by, or the day of the event they must attend?" });
    }
    if !amounts.is_empty() {
        let mut c: serde_json::Map<String, Value> = amounts
            .iter()
            .map(|a| (a.clone(), json!(format!("{a}, as written in the mail"))))
            .collect();
        c.insert(
            "none".into(),
            json!("The user does not have to pay any of these amounts."),
        );
        q["amount"] = json!({ "type": "choice", "criteria": c, "instructions":
            "Which amount does the user have to pay?" });
    }
    q
}

fn picked(answers: &Value, id: &str) -> Option<String> {
    let a = &answers[id];
    let choice = a["choice"].as_str()?;
    (choice != "none" && a["confidence"].as_f64()? >= MIN_PICK_CONFIDENCE)
        .then(|| choice.to_string())
}

/// Reads Jev's answers; unsure or invalid parts are left empty.
pub fn judgment(answers: &Value, dates: &[dates::DateCandidate], amounts: &[String]) -> Judgment {
    let kind = answers["kind"]["choice"]
        .as_str()
        .and_then(Kind::from_key)
        .filter(|_| answers["kind"]["confidence"].as_f64().unwrap_or(0.0) >= MIN_KIND_CONFIDENCE);
    let due = picked(answers, "due").and_then(|d| dates.iter().find(|c| c.date.to_string() == d));
    Judgment {
        kind,
        needs_reply: answers["needs_reply"]["noul"].as_f64(),
        urgency: answers["urgency"]["score"].as_f64(),
        due: due.map(|d| d.date.to_string()),
        due_time: due.and_then(|d| d.time.clone()),
        amount: picked(answers, "amount").filter(|a| amounts.contains(a)),
        automated: answers["automated"]["noul"].as_f64(),
        ..Default::default()
    }
}

/// Where a card sits in the feed, top to bottom.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    /// Mail in the Unread status, newest first, whatever its place once read.
    Unread,
    Now,
    Today,
    Week,
    Waiting,
    Later,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Unread,
        Section::Now,
        Section::Today,
        Section::Week,
        Section::Waiting,
        Section::Later,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Unread => "Unread",
            Section::Now => "Now",
            Section::Today => "Today",
            Section::Week => "This week",
            Section::Waiting => "Waiting for answers",
            Section::Later => "Can wait",
        }
    }
}

/// One piece of work the feed can show: a conversation (Gmail threads with the same
/// people about one matter, or one automated sender's mail), with its threads in the
/// feed in `threads`, the most recent first. The store reads one unit per thread;
/// [`fold`] joins them per conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedUnit {
    /// The conversation id (a lone thread's own id).
    pub id: String,
    pub threads: Vec<ThreadSummary>,
    pub judgment: Option<Judgment>,
    pub texts: Option<Texts>,
    /// Days since the user's last message, for threads waiting on someone else.
    pub waiting_days: Option<i64>,
    /// A Backlog thread brought back because its date is close.
    pub from_backlog: bool,
    /// The last sender looks like a machine (no-reply address), known without Jev.
    pub automated_sender: bool,
    /// The user taught Mermoz to archive this sender's mail (the learned
    /// rule): only such mail is grouped into a batch.
    pub learned_noise: bool,
    /// Came back from Later, with what the user had picked ("Friday evening").
    pub back_from_later: Option<String>,
}

impl FeedUnit {
    pub fn primary(&self) -> &ThreadSummary {
        &self.threads[0]
    }

    /// One of its threads is in the Unread status.
    pub fn is_unread(&self) -> bool {
        self.threads.iter().any(|t| t.status == "unread")
    }
}

/// Above this, a "yes" from Jev counts (needs a reply, automated).
const LIKELY: f64 = 0.7;

/// Days left before a "YYYY-MM-DD" date; negative once it passed.
pub fn days_until(date: &str, today: NaiveDate) -> Option<i64> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(|d| (d - today).num_days())
}

/// The kind a unit shows as: Waiting comes from the status, automated mail is clean-up
/// or reading, else Jev's kind.
pub fn effective_kind(u: &FeedUnit) -> Kind {
    let t = u.primary();
    if t.status == "waiting" {
        return Kind::Waiting;
    }
    let j = u.judgment.as_ref();
    let kind = j.and_then(|j| j.kind);
    let automated = j
        .and_then(|j| j.automated)
        .map_or(u.automated_sender, |p| p >= LIKELY);
    match kind {
        Some(Kind::Reading) => Kind::Reading,
        Some(Kind::Cleanup) => Kind::Cleanup,
        // An automated bill or event still needs the user; anything else automated is noise.
        Some(k @ (Kind::Payment | Kind::Event | Kind::Sign)) => k,
        _ if automated => Kind::Cleanup,
        Some(k) => k,
        None => Kind::Other,
    }
}

/// The follow-up date the user set on the conversation (the earliest when its threads
/// differ).
pub fn follow_up_of(u: &FeedUnit) -> Option<String> {
    u.threads.iter().filter_map(|t| t.due.clone()).min()
}

/// The deadline a card shows: the user's own follow-up date first, else the one Jev picked.
pub fn due_of(u: &FeedUnit) -> Option<String> {
    follow_up_of(u).or_else(|| u.judgment.as_ref().and_then(|j| j.due.clone()))
}

/// The deadline among several ("YYYY-MM-DD", time): the nearest one still ahead, else
/// the one that passed last.
fn nearest_due(
    dues: impl Iterator<Item = (String, Option<String>)>,
    today: NaiveDate,
) -> Option<(String, Option<String>)> {
    let all: Vec<_> = dues.collect();
    let ahead = all
        .iter()
        .filter(|(d, _)| days_until(d, today).is_some_and(|n| n >= 0))
        .min_by(|a, b| a.0.cmp(&b.0));
    ahead
        .or_else(|| all.iter().max_by(|a, b| a.0.cmp(&b.0)))
        .cloned()
}

/// One unit per conversation, in the order the conversations first appear. A
/// conversation shows as its newest thread (its kind, dates, status, words and sender),
/// judged by its newest judgment; a deadline from any of its threads counts, and it is
/// unread while any of them is.
pub fn fold(units: Vec<FeedUnit>, today: NaiveDate) -> Vec<FeedUnit> {
    let mut order: Vec<String> = vec![];
    let mut groups: HashMap<String, Vec<FeedUnit>> = HashMap::new();
    for u in units {
        let t = u.primary();
        let key = if t.conversation_id.is_empty() {
            t.id.clone()
        } else {
            t.conversation_id.clone()
        };
        groups
            .entry(key.clone())
            .or_insert_with(|| {
                order.push(key);
                vec![]
            })
            .push(u);
    }
    order
        .into_iter()
        .filter_map(|k| groups.remove(&k).map(|g| join(k, g, today)))
        .collect()
}

/// A conversation's units as one (see [`fold`]).
fn join(id: String, mut parts: Vec<FeedUnit>, today: NaiveDate) -> FeedUnit {
    parts.sort_by_key(|u| std::cmp::Reverse(u.primary().last_date));
    let judgments: Vec<&Judgment> = parts.iter().filter_map(|u| u.judgment.as_ref()).collect();
    let mut judgment = judgments.first().map(|j| (*j).clone());
    if let Some(j) = judgment.as_mut() {
        let dues = judgments
            .iter()
            .filter_map(|x| x.due.clone().map(|d| (d, x.due_time.clone())));
        if let Some((due, time)) = nearest_due(dues, today) {
            j.due = Some(due);
            j.due_time = time;
        }
        if j.amount.is_none() {
            j.amount = judgments.iter().find_map(|x| x.amount.clone());
        }
    }
    let back_from_later = parts.iter().find_map(|u| u.back_from_later.clone());
    let threads = parts.iter().flat_map(|u| u.threads.clone()).collect();
    let lead = parts.swap_remove(0);
    FeedUnit {
        id,
        threads,
        judgment,
        back_from_later,
        ..lead
    }
}

/// How long the user waits before a nudge is due: twice the usual answer time, at least 2 days.
pub fn nudge_after_days(typical_reply_days: Option<f64>) -> i64 {
    typical_reply_days
        .map(|d| (2.0 * d).ceil() as i64)
        .unwrap_or(3)
        .max(2)
}

/// Picks the section a card has once read ([`assemble`] puts unread mail on top of
/// these). Deadlines first, then urgent replies, then the rest.
pub fn section(u: &FeedUnit, kind: Kind, today: NaiveDate) -> Section {
    let j = u.judgment.as_ref();
    if matches!(kind, Kind::Cleanup | Kind::Reading) {
        return Section::Later;
    }
    // The user asked to see it now: it comes first.
    if u.back_from_later.is_some() {
        return Section::Now;
    }
    // Mail nobody touched for months, with no date ahead, is not today's work.
    if is_stale(u, kind, today) {
        return Section::Later;
    }
    if kind == Kind::Waiting {
        let overdue =
            u.waiting_days.unwrap_or(0) >= nudge_after_days(j.and_then(|j| j.typical_reply_days));
        return if overdue {
            Section::Today
        } else {
            Section::Waiting
        };
    }
    let days = due_of(u).and_then(|d| days_until(&d, today));
    let urgency = j.and_then(|j| j.urgency).unwrap_or(0.0);
    let needs_reply = j.and_then(|j| j.needs_reply).unwrap_or(0.0);
    match days {
        Some(d) if d <= 2 => return Section::Now,
        _ if urgency >= 2.5 && needs_reply >= LIKELY => return Section::Now,
        Some(d) if d <= 7 => return Section::Today,
        _ => {}
    }
    if urgency >= 2.0 || u.primary().priority == 1 {
        Section::Today
    } else {
        Section::Week
    }
}

/// Past this many days without news, open mail with no date ahead can wait.
pub const STALE_AFTER_DAYS: i64 = 60;

/// Days since the thread's last message.
fn idle_days(u: &FeedUnit, today: NaiveDate) -> i64 {
    (today - day_of(u.primary().last_date)).num_days().max(0)
}

/// Nothing happened for [`STALE_AFTER_DAYS`] and no date is coming: a wait nobody
/// answered, or mail left open long ago.
pub fn is_stale(u: &FeedUnit, kind: Kind, today: NaiveDate) -> bool {
    let idle = if kind == Kind::Waiting {
        u.waiting_days.unwrap_or(0)
    } else {
        idle_days(u, today)
    };
    let date_ahead = due_of(u)
        .and_then(|d| days_until(&d, today))
        .is_some_and(|d| d >= 0);
    idle > STALE_AFTER_DAYS && !date_ahead
}

/// "3 days", "4 months", "14 years".
pub fn span(days: i64) -> String {
    let (n, unit) = match days {
        d if d < 60 => (d, "day"),
        d if d < 730 => (d / 30, "month"),
        d => (d / 365, "year"),
    };
    if n == 1 {
        format!("1 {unit}")
    } else {
        format!("{n} {unit}s")
    }
}

/// Within a section: earliest deadline first, then how much the user answers these
/// people times urgency, then the most recent first. Returns a sortable key.
fn order_key(u: &FeedUnit, today: NaiveDate) -> (i64, i64, i64) {
    let j = u.judgment.as_ref();
    let days = due_of(u)
        .and_then(|d| days_until(&d, today))
        .unwrap_or(i64::MAX);
    let urgency = j.and_then(|j| j.urgency).unwrap_or(1.0);
    let weight = j.map_or(0.0, |j| j.sender_weight);
    let reply = j.and_then(|j| j.needs_reply).unwrap_or(0.0);
    let score = urgency * (1.0 + weight) + reply;
    (days, -(score * 1000.0) as i64, -u.primary().last_date)
}

/// "Thu 16 Oct".
pub fn short_date(date: &str) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| d.format("%a %-d %b").to_string())
        .unwrap_or_else(|_| date.to_string())
}

/// The deadline in words: "today", "tomorrow", "Thu 16 Oct", "2 days ago".
pub fn due_words(date: &str, today: NaiveDate) -> String {
    match days_until(date, today) {
        Some(0) => "today".into(),
        Some(1) => "tomorrow".into(),
        Some(-1) => "yesterday".into(),
        Some(d) if d < 0 => format!("{} days ago", -d),
        _ => short_date(date),
    }
}

/// "Why this card?": the facts that put it here, in plain words, without numbers from
/// the model. The AI's own sentence (`Texts::why`) comes on top when there is one.
pub fn reasons(u: &FeedUnit, kind: Kind, today: NaiveDate) -> Vec<String> {
    let j = u.judgment.as_ref();
    let mut out = vec![];
    if let Some(when) = &u.back_from_later {
        out.push(format!("You set this for {when}."));
    }
    if u.from_backlog {
        out.push("It was in your Backlog, and its date is close.".to_string());
    }
    // A Waiting card is one line: how long the wait has lasted comes first.
    if kind == Kind::Waiting {
        let days = u.waiting_days.unwrap_or(0);
        let usual = j.and_then(|j| j.typical_reply_days);
        out.push(match usual {
            _ if days > STALE_AFTER_DAYS => format!(
                "No answer for {}. Archive it if it no longer matters.",
                span(days)
            ),
            Some(u) if days >= nudge_after_days(Some(u)) => format!(
                "No answer for {days} days; they usually answer within {}.",
                plural_days(u.ceil() as i64)
            ),
            _ => format!("You wrote last, {} ago.", plural_days(days)),
        });
    } else if is_stale(u, kind, today) {
        out.push(format!(
            "Nothing new for {}. Archive it if it no longer matters.",
            span(idle_days(u, today))
        ));
    }
    if let Some(due) = due_of(u) {
        let words = due_words(&due, today);
        let mine = follow_up_of(u).is_some();
        out.push(match (kind, days_until(&due, today)) {
            (_, Some(d)) if d < 0 => format!("The date passed {words}."),
            (Kind::Event, _) => format!("It happens {}.", on(&words)),
            _ if mine => format!("You set a follow-up date: {words}."),
            _ => format!("The mail asks for it by {words}."),
        });
    }
    match kind {
        Kind::Promise => out.push("You said you would do it.".into()),
        Kind::Payment => out.push(match j.and_then(|j| j.amount.as_deref()) {
            Some(a) => format!("There is {a} to pay."),
            None => "There is something to pay.".into(),
        }),
        Kind::Cleanup => out.push(
            "Looks like automatic mail. Archive it: after three from this sender, they are cleared together."
                .into(),
        ),
        Kind::Reading => out.push(
            "A newsletter. Archive it: after three from this sender, they are grouped.".into(),
        ),
        Kind::Intro => out.push("Someone new for you.".into()),
        _ => {}
    }
    if kind != Kind::Waiting && j.and_then(|j| j.needs_reply).unwrap_or(0.0) >= LIKELY {
        out.push("Someone is waiting for your answer.".into());
    }
    if j.and_then(|j| j.urgency).unwrap_or(0.0) >= 2.5 {
        out.push("It looks urgent.".into());
    }
    if j.is_some_and(|j| j.sender_weight >= 0.5) {
        out.push("You usually answer these people.".into());
    }
    if u.threads.iter().any(|t| t.unread) {
        out.push("You haven't read it yet.".into());
    }
    if out.is_empty() {
        out.push("It is still open in your mail.".into());
    }
    out
}

fn on(words: &str) -> String {
    if words.contains(' ') && !words.ends_with("ago") {
        format!("on {words}")
    } else {
        words.to_string()
    }
}

fn plural_days(n: i64) -> String {
    if n == 1 {
        "1 day".into()
    } else {
        format!("{n} days")
    }
}

/// One card of the feed: a conversation, or a batch (clean-up, reading) of
/// conversations, each as its newest thread in `members`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Card {
    /// The conversation id, or `batch:cleanup` / `batch:reading`.
    pub id: String,
    pub kind: Kind,
    pub section: Section,
    /// The conversation's newest thread (a batch's most recent one).
    pub thread: ThreadSummary,
    /// The newest thread of each conversation a batch covers; empty for single cards.
    pub members: Vec<ThreadSummary>,
    /// What to do, as a short phrase: the AI's when written, else the subject.
    pub title: String,
    /// The AI's one line on why it matters now.
    pub why: Option<String>,
    /// "Why this card?" in plain words.
    pub reasons: Vec<String>,
    pub due: Option<String>,
    pub due_time: Option<String>,
    pub amount: Option<String>,
    pub from_backlog: bool,
    pub waiting_days: Option<i64>,
    pub texts: Option<Texts>,
    /// The account's AI could write this card's words and hasn't yet: ask with `feed.texts`.
    pub texts_wanted: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FeedSection {
    pub key: Section,
    pub title: String,
    pub cards: Vec<Card>,
    /// Cards left out past [`MAX_CARDS_PER_SECTION`]; the List shows them all.
    pub more: usize,
}

/// A section shows its most pressing cards; past this the feed stops being a to-do list.
pub const MAX_CARDS_PER_SECTION: usize = 40;
/// A batch lists its most recent mails; archiving them brings the next ones.
pub const MAX_BATCH_MEMBERS: usize = 50;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Feed {
    pub sections: Vec<FeedSection>,
    /// Open conversations in the feed (a batch counts each of its conversations).
    pub open: usize,
    /// How many of them Jev has judged; the rest show plain facts until it has.
    pub judged: usize,
}

fn single(u: FeedUnit, kind: Kind, section: Section, today: NaiveDate, ai: bool) -> Card {
    let j = u.judgment.clone().unwrap_or_default();
    let reasons = reasons(&u, kind, today);
    let due = due_of(&u);
    let t = u.primary().clone();
    let title = u
        .texts
        .as_ref()
        .map(|x| x.title.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if t.subject.is_empty() {
                "(no subject)".into()
            } else {
                t.subject.clone()
            }
        });
    Card {
        id: u.id.clone(),
        kind,
        section,
        title,
        why: u
            .texts
            .as_ref()
            .map(|x| x.why.clone())
            .filter(|s| !s.is_empty()),
        reasons,
        due_time: if due == j.due { j.due_time } else { None },
        due,
        amount: j.amount,
        from_backlog: u.from_backlog,
        waiting_days: u.waiting_days,
        texts_wanted: ai && u.texts.is_none(),
        texts: u.texts,
        members: vec![],
        thread: t,
    }
}

fn batch(kind: Kind, mut units: Vec<FeedUnit>, ai: &HashSet<String>) -> Card {
    units.sort_by_key(|u| std::cmp::Reverse(u.primary().last_date));
    units.truncate(MAX_BATCH_MEMBERS);
    let members: Vec<ThreadSummary> = units.iter().map(|u| u.primary().clone()).collect();
    let n = members.len();
    let (id, title, reason) = match kind {
        Kind::Reading => (
            "batch:reading",
            if n == 1 {
                "1 newsletter to read or archive".to_string()
            } else {
                format!("{n} newsletters to read or archive")
            },
            "You taught Mermoz to archive these newsletters.",
        ),
        _ => (
            "batch:cleanup",
            if n == 1 {
                "1 automatic mail to archive".to_string()
            } else {
                format!("{n} automatic mails to archive")
            },
            "You taught Mermoz to archive mail from these senders.",
        ),
    };
    // A batch's words are the one-line summaries of its members.
    let wanted = units
        .iter()
        .any(|u| u.texts.is_none() && ai.contains(&u.primary().account_id));
    let summaries: Vec<String> = units
        .iter()
        .map(|u| {
            u.texts
                .as_ref()
                .map(|t| t.summary.clone())
                .unwrap_or_default()
        })
        .collect();
    Card {
        id: id.into(),
        kind,
        section: Section::Later,
        thread: members[0].clone(),
        title,
        why: None,
        reasons: vec![reason.into()],
        due: None,
        due_time: None,
        amount: None,
        from_backlog: false,
        waiting_days: None,
        texts: summaries.iter().any(|s| !s.is_empty()).then(|| Texts {
            lines: summaries,
            ..Default::default()
        }),
        texts_wanted: wanted,
        members,
    }
}

/// A card before it is built: a unit with its kind, or a batch already built.
enum Item {
    Single(Kind, Box<FeedUnit>),
    Batch(Box<Card>),
}

/// An item with its section and sort key.
type Placed = (Section, (i64, i64, i64), Item);

/// The Unread section's order: mail just back from Later (the user asked to see it
/// then), then newest first.
fn newest_first(back_from_later: bool, last_date: i64) -> (i64, i64, i64) {
    (i64::from(!back_from_later), -last_date, 0)
}

/// Places a batch: on top while it holds unread mail (at its newest unread mail's
/// date), else after the single cards of the mail that can wait.
fn place_batch(kind: Kind, units: Vec<FeedUnit>, ai: &HashSet<String>) -> Placed {
    let newest_unread = units
        .iter()
        .filter(|u| u.is_unread())
        .map(|u| u.primary().last_date)
        .max();
    let mut card = batch(kind, units, ai);
    match newest_unread {
        Some(d) => {
            card.section = Section::Unread;
            (
                Section::Unread,
                newest_first(false, d),
                Item::Batch(Box::new(card)),
            )
        }
        None => (
            Section::Later,
            (i64::MAX, 0, 0),
            Item::Batch(Box::new(card)),
        ),
    }
}

/// Builds the feed: kind and section per unit, batches for automated mail and
/// newsletters, sections in order, cards in order within each. Unread mail is on top,
/// newest first; the rest follows [`section`] and [`order_key`]. `ai_accounts` are the
/// accounts whose mail may go to an AI.
pub fn assemble(units: Vec<FeedUnit>, today: NaiveDate, ai_accounts: &HashSet<String>) -> Feed {
    let open = units.len();
    let judged = units.iter().filter(|u| u.judgment.is_some()).count();
    // Cards are built only for the units shown: ordering needs every unit, words don't.
    let mut placed: Vec<Placed> = vec![];
    let (mut cleanup, mut reading) = (vec![], vec![]);
    for u in units {
        let kind = effective_kind(&u);
        // Batches only hold what the user's own history vouches for; anything else
        // automated is a card of its own, decided one at a time.
        match kind {
            Kind::Cleanup if u.learned_noise => cleanup.push(u),
            Kind::Reading if u.learned_noise => reading.push(u),
            _ if u.is_unread() => {
                let key = newest_first(u.back_from_later.is_some(), u.primary().last_date);
                placed.push((Section::Unread, key, Item::Single(kind, Box::new(u))));
            }
            _ => {
                let sec = section(&u, kind, today);
                let key = order_key(&u, today);
                placed.push((sec, key, Item::Single(kind, Box::new(u))));
            }
        }
    }
    if !cleanup.is_empty() {
        placed.push(place_batch(Kind::Cleanup, cleanup, ai_accounts));
    }
    if !reading.is_empty() {
        placed.push(place_batch(Kind::Reading, reading, ai_accounts));
    }
    // Stable: a batch tying with single cards comes after them.
    placed.sort_by_key(|a| (a.0, a.1));
    let mut sections: Vec<FeedSection> = Section::ALL
        .iter()
        .map(|s| FeedSection {
            key: *s,
            title: s.title().into(),
            cards: vec![],
            more: 0,
        })
        .collect();
    for (sec, _, item) in placed {
        let s = &mut sections[sec as usize];
        match item {
            // Never left out: a batch is how the noise gets cleared.
            Item::Batch(card) => s.cards.push(*card),
            Item::Single(..) if s.cards.len() >= MAX_CARDS_PER_SECTION => s.more += 1,
            Item::Single(kind, u) => {
                let ai = ai_accounts.contains(&u.primary().account_id);
                s.cards.push(single(*u, kind, sec, today, ai));
            }
        }
    }
    sections.retain(|s| !s.cards.is_empty());
    Feed {
        sections,
        open,
        judged,
    }
}

/// The local calendar day, which sections are computed against.
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// The local day a timestamp falls on.
pub fn day_of(ms: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).date_naive())
        .unwrap_or_else(today)
}

/// A Backlog thread comes back when the date it mentions is within this many days.
pub const BACKLOG_HORIZON_DAYS: i64 = 2;

/// Whether a Backlog thread mentions a date close enough to be worth a Jev call.
pub fn backlog_worth_judging(candidates: &[dates::DateCandidate], today: NaiveDate) -> bool {
    candidates.iter().any(|c| {
        let d = (c.date - today).num_days();
        (0..=BACKLOG_HORIZON_DAYS).contains(&d)
    })
}

#[cfg(test)]
mod tests;
