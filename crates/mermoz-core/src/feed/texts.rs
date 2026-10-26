//! The words on a feed card, written by the account's AI when the card comes into view:
//! a title, why it matters now, a ready reply for the user to check, options for a
//! decision, key points, a one-line summary. Cached until new mail arrives.

use super::Kind;
use crate::error::{Error, Result};
use crate::model::Message;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One way to answer a decision, with the reply that goes with it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Choice {
    pub label: String,
    pub detail: String,
    /// The AI recommends this one.
    pub recommended: bool,
    /// Why, when recommended.
    pub reason: String,
    /// The reply that picks it, for the user to check before sending.
    pub reply: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Texts {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub why: String,
    /// A reply (or nudge) ready for the user to check; empty when the card needs none.
    #[serde(default)]
    pub draft: String,
    #[serde(default)]
    pub options: Vec<Choice>,
    #[serde(default)]
    pub points: Vec<String>,
    #[serde(default)]
    pub summary: String,
    /// A batch's one-line summaries, one per member in order (empty when not written yet).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
}

/// Only the latest messages matter for what to do next.
const MESSAGES: usize = 4;
/// Per message; long newsletters are cut, the gist is at the top.
const MESSAGE_CHARS: usize = 2_000;

pub const SYSTEM: &str = "You write the cards of a to-do feed in an email client. \
Each card is one email thread and the one thing its reader (\"the user\") has to do about it.\n\n\
Return JSON with:\n\
- title: what the user has to do, as a short imperative phrase in English (at most 8 words), \
naming the person or thing (\"Confirm the 21 Oct talk with Thomas\"). For a newsletter or notification, what it is about.\n\
- why: one English sentence (at most 18 words) on why it matters now, citing a concrete fact from the thread \
(a deadline, who is waiting, an amount, a clash). Never mention AI, models, scores or percentages.\n\
- draft: when the card kind is reply, promise, intro or waiting, the message the user would send next, \
in the language of the thread, short, warm and ready to send: no subject line, no placeholders, signed with the user's first name if known. \
For waiting, a polite follow-up asking where things stand. Otherwise an empty string. \
Write titles in plain everyday words (\"Follow up with…\", never \"Nudge\").\n\
- options: when the kind is decision, the 2 or 3 choices the user is asked to make, each with label (at most 5 words), \
detail (at most 10 words: price, delay, consequence), recommended (true for at most one, only when the thread gives a reason), \
reason (why, or empty) and reply (the message choosing it, in the language of the thread). Otherwise an empty list.\n\
- points: when the kind is sign, payment or event, up to 3 key facts the user must know before acting (amounts, dates, what changes). \
Otherwise an empty list.\n\
- summary: one English sentence with the gist of the thread (for a newsletter, its main news).\n\
Use only facts written in the thread. Never invent names, dates or amounts.";

pub fn schema() -> Value {
    let s = json!({ "type": "string" });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "why", "draft", "options", "points", "summary"],
        "properties": {
            "title": s, "why": s, "draft": s, "summary": s,
            "points": { "type": "array", "items": s },
            "options": { "type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["label", "detail", "recommended", "reason", "reply"],
                "properties": { "label": s, "detail": s, "recommended": { "type": "boolean" }, "reason": s, "reply": s }
            } }
        }
    })
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Reply => "reply",
        Kind::Decision => "decision",
        Kind::Sign => "sign",
        Kind::Promise => "promise",
        Kind::Event => "event",
        Kind::Payment => "payment",
        Kind::Intro => "intro",
        Kind::Waiting => "waiting",
        Kind::Cleanup => "cleanup",
        Kind::Reading => "reading",
        Kind::Other => "other",
    }
}

/// The thread as the AI reads it: the latest messages without quoted history.
pub fn user_prompt(
    kind: Kind,
    today: &str,
    me: &str,
    subject: &str,
    messages: &[Message],
) -> String {
    let mut out = format!(
        "Card kind: {}\nToday: {today}\nThe user: {me}\nSubject: {subject}\n",
        kind_name(kind)
    );
    let start = messages.len().saturating_sub(MESSAGES);
    for m in &messages[start..] {
        let who = if m.is_from_me {
            "the user".to_string()
        } else if m.from.name.is_empty() {
            m.from.email.clone()
        } else {
            format!("{} <{}>", m.from.name, m.from.email)
        };
        let when = chrono::DateTime::from_timestamp_millis(m.date)
            .map(|d| {
                d.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        let body: String = m
            .body_clean
            .as_deref()
            .unwrap_or(&m.body_new)
            .chars()
            .take(MESSAGE_CHARS)
            .collect();
        let files: Vec<&str> = m.attachments.iter().map(|a| a.filename.as_str()).collect();
        out.push_str(&format!("\n<message from=\"{who}\" date=\"{when}\""));
        if !files.is_empty() {
            out.push_str(&format!(" attachments=\"{}\"", files.join(", ")));
        }
        out.push_str(&format!(">\n{body}\n</message>\n"));
    }
    out
}

/// Reads the AI's answer; at most one option stays recommended.
pub fn parse(raw: &str) -> Result<Texts> {
    let mut t: Texts =
        serde_json::from_str(raw).map_err(|_| Error::other("provider returned malformed JSON"))?;
    let mut seen = false;
    for o in t.options.iter_mut() {
        if o.recommended && seen {
            o.recommended = false;
        }
        seen |= o.recommended;
    }
    t.lines.clear();
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Address, AttachmentInfo};
    use rstest::rstest;

    fn msg(body: &str, mine: bool) -> Message {
        Message {
            id: "m".into(),
            thread_id: "t".into(),
            from: Address {
                name: "Thomas Bernard".into(),
                email: "thomas@acme.example".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            is_bounce: false,
            is_auto_reply: false,
            delivery_failure: None,
            date: 1_791_700_000_000,
            subject: "Offsite".into(),
            body_text: body.into(),
            body_new: body.into(),
            has_quoted: false,
            has_signature: false,
            forwarded: false,
            forwarded_from: None,
            quoted_only: false,
            reaction: None,
            duplicate_of: None,
            copies: 1,
            body_clean: None,
            has_html: false,
            is_from_me: mine,
            attachments: vec![AttachmentInfo {
                id: "a".into(),
                filename: "agenda.pdf".into(),
                mime: "application/pdf".into(),
                size: 1,
                inline: false,
                decorative: false,
            }],
            auth: None,
            usual_domain: None,
        }
    }

    #[rstest]
    fn prompt_names_the_card_kind() {
        let p = user_prompt(Kind::Decision, "2026-10-12", "me@example.com", "Tiles", &[]);
        assert!(p.contains("Card kind: decision"));
    }

    #[rstest]
    fn prompt_shows_the_users_own_messages_as_the_user() {
        let p = user_prompt(
            Kind::Reply,
            "2026-10-12",
            "me@example.com",
            "x",
            &[msg("ok", true)],
        );
        assert!(p.contains("from=\"the user\""));
    }

    #[rstest]
    fn prompt_lists_attachments() {
        let p = user_prompt(
            Kind::Sign,
            "2026-10-12",
            "me@example.com",
            "x",
            &[msg("hi", false)],
        );
        assert!(p.contains("attachments=\"agenda.pdf\""));
    }

    #[rstest]
    fn prompt_keeps_only_the_latest_messages() {
        let msgs: Vec<Message> = (0..6).map(|i| msg(&format!("body{i}"), false)).collect();
        let p = user_prompt(Kind::Reply, "2026-10-12", "me@example.com", "x", &msgs);
        assert!(!p.contains("body1"));
    }

    #[rstest]
    fn prompt_cuts_long_messages() {
        let p = user_prompt(
            Kind::Reading,
            "2026-10-12",
            "me",
            "x",
            &[msg(&"a".repeat(5000), false)],
        );
        assert!(p.len() < 2_500);
    }

    #[rstest]
    fn schema_requires_every_field() {
        assert_eq!(schema()["required"].as_array().unwrap().len(), 6);
    }

    #[rstest]
    fn parse_keeps_one_recommendation() {
        let raw = json!({ "title": "t", "why": "w", "draft": "", "summary": "s", "points": [],
            "options": [
                { "label": "a", "detail": "", "recommended": true, "reason": "r", "reply": "" },
                { "label": "b", "detail": "", "recommended": true, "reason": "r", "reply": "" } ] })
        .to_string();
        let t = parse(&raw).unwrap();
        assert!(!t.options[1].recommended);
    }

    #[rstest]
    fn parse_refuses_non_json() {
        assert!(parse("Sure! Here is the card").is_err());
    }
}
