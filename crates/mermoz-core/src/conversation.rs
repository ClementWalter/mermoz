//! Conversations: Gmail threads with the same people about the same matter, read as
//! one chat. Gmail threads stay the storage truth; a conversation only groups them.
//!
//! Code decides who is a person, a feed (automated sender) or a list (mailing list),
//! and which earlier conversations a new thread could continue. Jev judges each
//! candidate pair; the account's AI decides the uncertain band. Measured on a real
//! mailbox: Jev ≥ 0.8 joins 62% of true continuations with 2% wrong joins; asking the
//! AI between 0.5 and 0.8 lifts that to 85% at 3%.

use serde::Serialize;
use serde_json::{Value, json};

/// Jev probability from which a pair joins without asking the AI.
pub const JOIN: f64 = 0.8;
/// Below this a pair stays apart without asking the AI.
pub const APART: f64 = 0.5;
/// Highest join bar a person's manual splits can raise it to.
pub const MAX_JOIN: f64 = 0.95;
/// A thread can continue a conversation active at most this long before it started.
pub const WINDOW_MS: i64 = 30 * 86_400_000;
/// Threads with more counterparts are announcements, never grouped by judgment.
pub const MAX_PEOPLE: usize = 5;
/// Earlier conversations judged against each new thread.
pub const MAX_CANDIDATES: usize = 2;
/// Recent mail is grouped first; older mail follows in the background.
pub const RECENT_MS: i64 = 90 * 86_400_000;

/// What kind of stream a thread belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Someone the user writes to: grouped by judgment.
    Person,
    /// An automated sender, keyed by its address: grouped in code.
    Feed(String),
    /// A mailing list, keyed by its List-Id: grouped in code.
    List(String),
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Person => "person",
            Kind::Feed(_) => "feed",
            Kind::List(_) => "list",
        }
    }

    pub fn key(&self) -> Option<&str> {
        match self {
            Kind::Person => None,
            Kind::Feed(k) | Kind::List(k) => Some(k),
        }
    }
}

/// Bulk mailboxes a person once wrote to (support desks, shops) are still not a person.
pub fn is_bulk(email: &str) -> bool {
    if crate::store::is_automated(email) {
        return true;
    }
    let local = email.split('@').next().unwrap_or("").to_lowercase();
    let domain = email.split('@').nth(1).unwrap_or("").to_lowercase();
    const BULK: &[&str] = &[
        "info",
        "contact",
        "support",
        "hello",
        "team",
        "marketing",
        "billing",
        "mailer",
        "help",
        "service",
        "sales",
        "admin",
        "accounts",
    ];
    BULK.iter().any(|b| {
        local == *b
            || local.starts_with(&format!("{b}."))
            || local.starts_with(&format!("{b}-"))
            || local.starts_with(&format!("{b}_"))
    }) || domain.starts_with("lists.")
        || domain.starts_with("groups.")
        || domain == "googlegroups.com"
}

/// The thread's stream: a list when a message carries a List-Id, a feed when the user
/// never wrote in it and its sender is bulk or judged automated, else a person.
pub fn kind(
    list_id: Option<&str>,
    first_sender: &str,
    user_wrote: bool,
    automated: Option<f64>,
) -> Kind {
    if let Some(l) = list_id.map(str::trim).filter(|l| !l.is_empty()) {
        return Kind::List(l.to_lowercase());
    }
    let sender = first_sender.trim().to_lowercase();
    if !user_wrote && !sender.is_empty() && (is_bulk(&sender) || automated.unwrap_or(0.0) >= 0.5) {
        return Kind::Feed(sender);
    }
    Kind::Person
}

/// Join bar for a person: each manual split with them raises it by 0.1.
pub fn join_threshold(splits: i64) -> f64 {
    (JOIN + 0.1 * splits.max(0) as f64).min(MAX_JOIN)
}

/// What to do with a candidate pair given Jev's probability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Join,
    Apart,
    AskAi,
}

pub fn decide(p: f64, join_at: f64) -> Decision {
    if p >= join_at {
        Decision::Join
    } else if p < APART {
        Decision::Apart
    } else {
        Decision::AskAi
    }
}

/// One message as Jev and the AI see it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PairMessage {
    pub from: String,
    pub date: String,
    pub subject: String,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
}

/// One side of a candidate pair: a thread's subject, size and last two messages.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PairItem {
    pub subject: String,
    pub messages_in_thread: i64,
    pub last_messages: Vec<PairMessage>,
}

/// "Re: Fwd: Lunch" reads as "Lunch".
pub fn bare_subject(s: &str) -> String {
    let mut s = s.trim();
    loop {
        let lower = s.to_lowercase();
        let Some(p) = ["re:", "fwd:", "fw:", "tr:", "aw:", "sv:", "rv:", "wg:"]
            .iter()
            .find(|p| lower.starts_with(**p))
        else {
            return s.to_string();
        };
        s = s[p.len()..].trim_start();
    }
}

/// A message body for judging: its own words only, whitespace folded, 600 characters.
pub fn excerpt(body: &str) -> String {
    let (new, _) = crate::quote::readable(body);
    new.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(600)
        .collect()
}

pub fn pair_state(earlier: &PairItem, later: &PairItem, days_between: i64) -> Value {
    json!({ "earlier": earlier, "later": later, "days_between": days_between.max(0) })
}

/// One Jev request: same conversation (the decision), relation (a 3-level cross-check)
/// and whether either side is automated (a late feed signal).
pub fn questions() -> Value {
    json!({
        "same": {
            "type": "noul",
            "instructions": "The user corresponds with the same people in `earlier` and `later`. Is `later` part of the same conversation as `earlier`, so the user would want to read both together as one chat?",
            "criteria": {
                "true": "Same matter continued: an answer, reminder, follow-up, next step, document, payment or scheduling about the same request, project, event, deal or topic, even if the subject line changed.",
                "false": "A different matter with the same people, even if the tone, the relationship or the broad area (work, family, a shared hobby) is the same."
            }
        },
        "relation": {
            "type": "score",
            "instructions": "How are the matters discussed in `earlier` and `later` related?",
            "criteria": [
                "Unrelated matters that only share the people.",
                "Same broad area or ongoing relationship, but separate matters.",
                "The same matter continued."
            ]
        },
        "automated": {
            "type": "noul",
            "instructions": "Is `earlier` or `later` sent by an automated system or to a crowd (newsletter, notification, receipt, billing statement, marketing, mailing-list announcement) rather than a person writing to the user?"
        }
    })
}

/// Jev's "same conversation" probability, when it answered.
pub fn same_probability(answers: &Value) -> Option<f64> {
    answers["same"]["noul"].as_f64()
}

pub const AI_SYSTEM: &str = "You group a user's emails into chat-like conversations. You get two email threads \
with the same people. Decide whether the later one continues the same conversation as the earlier one: the same \
matter (request, project, event, deal or topic), not merely the same people or the same broad area. Answer with JSON.";

pub fn ai_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["same_conversation", "reason"],
        "properties": {
            "same_conversation": { "type": "boolean" },
            "reason": { "type": "string" }
        }
    })
}

/// The AI's verdict, when its answer is readable.
pub fn ai_verdict(raw: &str) -> Option<bool> {
    serde_json::from_str::<Value>(raw).ok()?["same_conversation"].as_bool()
}

/// Web links in a message, in order, without duplicates or trailing punctuation.
pub fn links(text: &str) -> Vec<String> {
    let delim = |c: char| c.is_whitespace() || "<>\"'()[]".contains(c);
    let mut out: Vec<String> = vec![];
    // One scan for "http" over the whole text: long newsletters have thousands of words.
    let mut from = 0;
    while let Some(k) = text[from..].find("http") {
        let i = from + k;
        let rest = &text[i..];
        if !(rest.starts_with("http://") || rest.starts_with("https://")) {
            from = i + 4;
            continue;
        }
        let end = rest.find(delim).map_or(text.len(), |e| i + e);
        let url = text[i..end].trim_end_matches(['.', ',', ';', ':', '!', '?', '*']);
        if url.len() > 10 && !out.iter().any(|u| u == url) {
            out.push(url.to_string());
        }
        from = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::noreply("noreply@bank.example", true)]
    #[case::support_desk("support@shop.example", true)]
    #[case::info_dot("info.clients@shop.example", true)]
    #[case::list_domain("announce@lists.club.example", true)]
    #[case::person("nora.lind@agency.example", false)]
    #[case::name_starting_like_bulk("helena@studio.example", false)]
    fn bulk_senders(#[case] email: &str, #[case] want: bool) {
        assert_eq!(is_bulk(email), want);
    }

    #[rstest]
    fn list_id_makes_a_list() {
        assert_eq!(
            kind(
                Some("Runners <runners.club.example>"),
                "ada@club.example",
                true,
                None
            ),
            Kind::List("runners <runners.club.example>".into())
        );
    }

    #[rstest]
    fn bulk_sender_the_user_never_answered_is_a_feed() {
        assert_eq!(
            kind(None, "NoReply@Bank.example", false, None),
            Kind::Feed("noreply@bank.example".into())
        );
    }

    #[rstest]
    fn automated_sender_is_a_feed() {
        assert_eq!(
            kind(None, "deals@shop.example", false, Some(0.9)),
            Kind::Feed("deals@shop.example".into())
        );
    }

    #[rstest]
    fn a_reply_from_the_user_makes_it_a_person() {
        assert_eq!(
            kind(None, "support@shop.example", true, Some(0.9)),
            Kind::Person
        );
    }

    #[rstest]
    fn unsure_automation_stays_a_person() {
        assert_eq!(
            kind(None, "nora@agency.example", false, Some(0.3)),
            Kind::Person
        );
    }

    #[rstest]
    #[case::no_splits(0, 0.8)]
    #[case::one_split(1, 0.9)]
    #[case::capped(5, MAX_JOIN)]
    fn splits_raise_the_join_bar(#[case] splits: i64, #[case] want: f64) {
        assert!((join_threshold(splits) - want).abs() < 1e-9);
    }

    #[rstest]
    #[case::confident(0.85, Decision::Join)]
    #[case::at_bar(0.8, Decision::Join)]
    #[case::uncertain(0.6, Decision::AskAi)]
    #[case::at_floor(0.5, Decision::AskAi)]
    #[case::apart(0.2, Decision::Apart)]
    fn decisions_follow_the_bands(#[case] p: f64, #[case] want: Decision) {
        assert_eq!(decide(p, JOIN), want);
    }

    #[rstest]
    fn a_raised_bar_sends_more_to_the_ai() {
        assert_eq!(decide(0.85, join_threshold(1)), Decision::AskAi);
    }

    #[rstest]
    #[case::reply("Re: Lunch", "Lunch")]
    #[case::stacked("RE: Fwd: TR: Flat sale", "Flat sale")]
    #[case::plain("Garden fence quote", "Garden fence quote")]
    #[case::word_starting_like_prefix("Review of the deck", "Review of the deck")]
    fn bare_subjects(#[case] s: &str, #[case] want: &str) {
        assert_eq!(bare_subject(s), want);
    }

    #[rstest]
    fn excerpt_drops_quoted_history() {
        let body = "See you Monday.\n\nOn Sun, Oct 4, 2026 at 9:00 Sam Ito <sam@example.com> wrote:\n> Lunch next week?";
        assert_eq!(excerpt(body), "See you Monday.");
    }

    #[rstest]
    fn excerpt_is_capped() {
        assert_eq!(excerpt(&"word ".repeat(400)).chars().count(), 600);
    }

    #[rstest]
    fn pair_state_never_has_negative_gaps() {
        let item = PairItem {
            subject: "Lunch".into(),
            messages_in_thread: 1,
            last_messages: vec![],
        };
        assert_eq!(pair_state(&item, &item, -3)["days_between"], 0);
    }

    #[rstest]
    fn questions_ask_same_conversation_as_a_noul() {
        assert_eq!(questions()["same"]["type"], "noul");
    }

    #[rstest]
    #[case::answered(json!({ "same": { "noul": 0.83 } }), Some(0.83))]
    #[case::missing(json!({}), None)]
    fn same_probability_reads_the_noul(#[case] answers: Value, #[case] want: Option<f64>) {
        assert_eq!(same_probability(&answers), want);
    }

    #[rstest]
    #[case::yes(r#"{"same_conversation": true, "reason": "same sale"}"#, Some(true))]
    #[case::no(
        r#"{"same_conversation": false, "reason": "other topic"}"#,
        Some(false)
    )]
    #[case::garbage("not json", None)]
    fn ai_verdicts(#[case] raw: &str, #[case] want: Option<bool>) {
        assert_eq!(ai_verdict(raw), want);
    }

    #[rstest]
    #[case::plain("Book here https://notary.example/rdv.", vec!["https://notary.example/rdv"])]
    #[case::bracketed("Checklist <https://docs.example/list>", vec!["https://docs.example/list"])]
    #[case::duplicate("https://a.example/x and https://a.example/x", vec!["https://a.example/x"])]
    #[case::none("No link here", vec![])]
    #[case::inside_a_word("url=https://a.example/x;", vec!["https://a.example/x"])]
    #[case::http_word_without_scheme("httpd says hi http://b.example/y", vec!["http://b.example/y"])]
    #[case::two_lines("https://a.example/1\nhttps://a.example/2", vec!["https://a.example/1", "https://a.example/2"])]
    #[case::too_short("http://a.b", vec![])]
    fn links_are_found(#[case] text: &str, #[case] want: Vec<&str>) {
        assert_eq!(links(text), want);
    }
}
