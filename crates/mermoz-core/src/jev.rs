//! Instant routing with TypeSafe's Jev: one fast typed judgment per incoming thread
//! (project, post-read status, priority, automated), turned into a [`Suggestion`].
//!
//! Jev only sees the thread plus two locally built hints: where the user filed earlier
//! threads with the same people, and a short description of every project. On the
//! user's mailbox this took exact project accuracy from 40% to 78% (90% above 0.7
//! confidence) at ~250 ms per thread.

use crate::model::{ROUTABLE_STATUSES, Suggestion};
use crate::{Error, Result};
use serde::Serialize;
use serde_json::{Value, json};
use std::time::Duration;

const SERVICE: &str = "ai.mermoz.typesafe";
const ACCOUNT: &str = "api_key";
const MODEL: &str = "jev-latest";

/// Below these, a prediction is dropped rather than suggested.
pub const MIN_PROJECT_CONFIDENCE: f64 = 0.7;
pub const MIN_STATUS_CONFIDENCE: f64 = 0.5;
pub const MIN_PRIORITY_CONFIDENCE: f64 = 0.6;

const STATUS_CRITERIA: &[(&str, &str)] = &[
    (
        "todo",
        "The user must act: reply, decide, pay, sign, book or attend.",
    ),
    (
        "waiting",
        "The user wrote last and is waiting for someone else to answer or deliver.",
    ),
    (
        "in_progress",
        "The user is actively working on it over several exchanges.",
    ),
    ("backlog", "Worth doing some day, not now."),
    (
        "done",
        "Handled: the conversation reached its end and nothing is left to do.",
    ),
    (
        "archive",
        "Nothing to do and never was: newsletters, notifications, receipts, confirmations, FYI.",
    ),
];

/// Priority options and the stored value each maps to (see `docs/protocol.md`).
const PRIORITY_CRITERIA: &[(&str, &str, i64)] = &[
    ("urgent", "Needs action today.", 1),
    ("high", "Needs action in the next few days.", 2),
    ("normal", "Needs action eventually.", 3),
    ("low", "Nice to handle, no consequence if it waits.", 4),
    ("none", "No action needed.", 0),
];

pub fn save_key(key: &str) -> Result<()> {
    crate::tokens::secret_set(SERVICE, ACCOUNT, key.trim())
}

pub fn load_key() -> Option<String> {
    crate::tokens::secret_get(SERVICE, ACCOUNT)
        .ok()
        .filter(|k| !k.is_empty())
}

pub fn delete_key() -> Result<()> {
    crate::tokens::secret_delete(SERVICE, ACCOUNT)
}

/// One recent message as Jev sees it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RouteMessage {
    pub from: String,
    pub days_ago: i64,
    pub text: String,
}

/// Everything Jev is told about one thread.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RouteInput {
    pub subject: String,
    pub messages_in_thread: i64,
    pub last_message_from_me: bool,
    pub recent_messages: Vec<RouteMessage>,
    /// "filed in <project>" → how many earlier threads with these people.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sender_history: Vec<(String, i64)>,
}

impl RouteInput {
    fn state(&self) -> Value {
        let mut v = json!({
            "subject": self.subject,
            "messages_in_thread": self.messages_in_thread,
            "last_message_from_me": self.last_message_from_me,
            "recent_messages": self.recent_messages,
        });
        if !self.sender_history.is_empty() {
            let h: serde_json::Map<String, Value> = self
                .sender_history
                .iter()
                .map(|(p, n)| (format!("filed in {p}"), json!(n)))
                .collect();
            v["sender_history"] = Value::Object(h);
        }
        v
    }
}

/// The four routing questions; `projects` pairs each name with its description.
pub fn questions(projects: &[(String, String)]) -> Value {
    let mut folders: serde_json::Map<String, Value> = projects
        .iter()
        .map(|(n, d)| (n.clone(), json!(d)))
        .collect();
    folders.insert(
        "none".into(),
        json!("The thread belongs to none of these folders."),
    );
    let status: serde_json::Map<String, Value> = STATUS_CRITERIA
        .iter()
        .map(|(k, d)| (k.to_string(), json!(d)))
        .collect();
    let priority: serde_json::Map<String, Value> = PRIORITY_CRITERIA
        .iter()
        .map(|(k, d, _)| (k.to_string(), json!(d)))
        .collect();
    json!({
        "project": { "type": "choice", "criteria": folders, "instructions":
            "In which of the user's email folders should this thread be filed? If `sender_history` is present, it lists where the user filed earlier threads with the same people; it is strong evidence but not decisive." },
        "status": { "type": "choice", "criteria": status, "instructions":
            "Where does this thread belong in the user's workflow once they have read it?" },
        "priority": { "type": "choice", "criteria": priority, "instructions":
            "How soon does the user need to act on this thread?" },
        "automated": { "type": "noul", "instructions":
            "Was this thread sent by an automated system (newsletter, notification, receipt, marketing) rather than a person writing to the user?" },
    })
}

fn choice(answers: &Value, id: &str) -> Option<(String, f64)> {
    let a = &answers[id];
    Some((a["choice"].as_str()?.to_string(), a["confidence"].as_f64()?))
}

/// Keeps only the confident parts of Jev's answers.
pub fn suggestion(answers: &Value, known_projects: &[String]) -> Suggestion {
    let mut s = Suggestion::default();
    if let Some((p, c)) = choice(answers, "project")
        && c >= MIN_PROJECT_CONFIDENCE
        && known_projects.contains(&p)
    {
        s.projects = vec![p];
        s.project_confidence = Some(c);
    }
    if let Some((st, c)) = choice(answers, "status")
        && c >= MIN_STATUS_CONFIDENCE
        && ROUTABLE_STATUSES.contains(&st.as_str())
    {
        s.status = Some(st);
        s.status_confidence = Some(c);
    }
    if let Some((p, c)) = choice(answers, "priority")
        && c >= MIN_PRIORITY_CONFIDENCE
    {
        s.priority = PRIORITY_CRITERIA
            .iter()
            .find(|(k, ..)| *k == p)
            .map(|(.., v)| *v);
    }
    s.automated = answers["automated"]["noul"].as_f64();
    s
}

/// One-line project description built from the mail already filed there.
pub fn describe(name: &str, correspondents: &[String], subjects: &[String]) -> String {
    let who = if correspondents.is_empty() {
        "unknown".to_string()
    } else {
        correspondents.join(", ")
    };
    let what = if subjects.is_empty() {
        "none".to_string()
    } else {
        subjects
            .iter()
            .map(|s| s.chars().take(60).collect::<String>())
            .collect::<Vec<_>>()
            .join(" | ")
    };
    format!("Folder '{name}'. Frequent correspondents: {who}. Example subjects: {what}.")
}

pub struct Client {
    http: reqwest::Client,
    base: String,
    key: String,
}

impl Client {
    pub fn new(key: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("http client");
        let base =
            std::env::var("MERMOZ_JEV_BASE").unwrap_or_else(|_| "https://api.typesafe.ai".into());
        Self { http, base, key }
    }

    /// Raw `systemone` call; returns the `answers` object.
    pub async fn ask(&self, state: &Value, questions: &Value) -> Result<Value> {
        let resp = self
            .http
            .post(format!("{}/v1/systemone", self.base))
            .bearer_auth(&self.key)
            .json(&json!({ "state": state, "model": MODEL, "questions": questions }))
            .send()
            .await?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(match status {
                401 | 403 => Error::InvalidParams(
                    "The TypeSafe key was refused. Check it in Settings › Developer.".into(),
                ),
                _ => Error::Upstream { status, body },
            });
        }
        let v: Value = resp.json().await?;
        Ok(v["answers"].clone())
    }

    pub async fn route(
        &self,
        input: &RouteInput,
        questions: &Value,
        known: &[String],
    ) -> Result<Suggestion> {
        let answers = self.ask(&input.state(), questions).await?;
        Ok(suggestion(&answers, known))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn answers(project: (&str, f64), status: (&str, f64), priority: (&str, f64)) -> Value {
        json!({
            "project": { "type": "choice", "choice": project.0, "confidence": project.1 },
            "status": { "type": "choice", "choice": status.0, "confidence": status.1 },
            "priority": { "type": "choice", "choice": priority.0, "confidence": priority.1 },
            "automated": { "type": "noul", "noul": 0.97 },
        })
    }

    fn known() -> Vec<String> {
        vec!["Receipts".into(), "Immo".into()]
    }

    #[rstest]
    #[case::confident_project(("Receipts", 0.9), vec!["Receipts"])]
    #[case::unsure_project(("Receipts", 0.5), vec![])]
    #[case::none_is_not_a_project(("none", 0.99), vec![])]
    #[case::unknown_project(("Gone", 0.99), vec![])]
    fn project_needs_confidence(#[case] project: (&str, f64), #[case] want: Vec<&str>) {
        let s = suggestion(&answers(project, ("archive", 0.9), ("none", 0.9)), &known());
        assert_eq!(s.projects, want);
    }

    #[rstest]
    #[case::confident(("archive", 0.8), Some("archive"))]
    #[case::unsure(("archive", 0.3), None)]
    fn status_needs_confidence(#[case] status: (&str, f64), #[case] want: Option<&str>) {
        let s = suggestion(&answers(("Immo", 0.9), status, ("none", 0.9)), &known());
        assert_eq!(s.status.as_deref(), want);
    }

    #[rstest]
    #[case::urgent("urgent", Some(1))]
    #[case::none("none", Some(0))]
    #[case::normal("normal", Some(3))]
    fn priority_maps_to_stored_values(#[case] p: &str, #[case] want: Option<i64>) {
        let s = suggestion(&answers(("Immo", 0.9), ("todo", 0.9), (p, 0.9)), &known());
        assert_eq!(s.priority, want);
    }

    #[rstest]
    fn automated_probability_is_kept() {
        let s = suggestion(
            &answers(("Immo", 0.9), ("todo", 0.9), ("none", 0.9)),
            &known(),
        );
        assert_eq!(s.automated, Some(0.97));
    }

    #[rstest]
    fn questions_offer_none_of_these() {
        let q = questions(&[("Immo".into(), "Folder 'Immo'.".into())]);
        assert!(q["project"]["criteria"]["none"].is_string());
    }

    #[rstest]
    fn sender_history_reads_as_filed_in() {
        let input = RouteInput {
            subject: "Devis".into(),
            messages_in_thread: 1,
            last_message_from_me: false,
            recent_messages: vec![],
            sender_history: vec![("Immo/Houlgate".into(), 4)],
        };
        assert_eq!(input.state()["sender_history"]["filed in Immo/Houlgate"], 4);
    }

    #[rstest]
    fn empty_sender_history_is_omitted() {
        let input = RouteInput {
            subject: "Devis".into(),
            messages_in_thread: 1,
            last_message_from_me: false,
            recent_messages: vec![],
            sender_history: vec![],
        };
        assert!(input.state().get("sender_history").is_none());
    }

    #[rstest]
    fn description_lists_people_and_subjects() {
        assert_eq!(
            describe("Immo", &["Oralia".into()], &["PV assemblée".into()]),
            "Folder 'Immo'. Frequent correspondents: Oralia. Example subjects: PV assemblée."
        );
    }
}
