//! Instant routing with TypeSafe's Jev: one fast typed judgment per incoming thread
//! (project, post-read status, priority, automated), turned into a [`Suggestion`].
//!
//! Jev only sees the thread plus two locally built hints: where the user filed earlier
//! threads with the same people, and a short description of every project. On the
//! user's mailbox this took exact project accuracy from 40% to 78% (90% above 0.7
//! confidence) at ~250 ms per thread.
//!
//! Age is told to Jev as words and numbers, and enforced in code: past
//! [`STALE_AFTER_DAYS`] of silence a thread is never suggested as still live.

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
        "The user wrote last and is waiting for an answer or a delivery that could still plausibly come.",
    ),
    (
        "in_progress",
        "The user is actively working on it over several exchanges.",
    ),
    ("backlog", "Worth doing some day, not now."),
    (
        "done",
        "Handled: the conversation reached its end, or has been quiet for months, and nothing is left to do.",
    ),
    (
        "archive",
        "Nothing to do and never was: newsletters, notifications, receipts, confirmations, FYI.",
    ),
];

/// Days of silence after which a thread is over: Todo, Waiting and In Progress are no
/// longer suggested. On the user's mailbox 99.8% of replies to the user came within 90
/// days (p99 19 days) and 98.9% of the user's own replies did.
pub const STALE_AFTER_DAYS: i64 = 90;

/// The only statuses a stale thread may be suggested.
pub const STALE_STATUSES: &[&str] = &["done", "archive"];

pub const DAY_MS: i64 = 86_400_000;

/// Whole days between a millisecond timestamp and `now_ms`.
pub fn days_ago(date_ms: i64, now_ms: i64) -> i64 {
    (now_ms - date_ms) / DAY_MS
}

/// Whether a thread silent for this many days is over.
pub fn is_stale(last_activity_days_ago: i64) -> bool {
    last_activity_days_ago > STALE_AFTER_DAYS
}

/// An age in words: Jev reads named spans more reliably than day counts.
pub fn ago(days: i64) -> String {
    match days {
        ..=0 => "today".into(),
        1 => "yesterday".into(),
        2..14 => format!("{days} days ago"),
        14..60 => format!("{} weeks ago", days / 7),
        60..730 => format!("{} months ago", days / 30),
        _ => format!("{} years ago", days / 365),
    }
}

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
    /// Local date the thread is judged on, `YYYY-MM-DD`.
    pub today: String,
    pub last_activity_days_ago: i64,
    pub thread_started_days_ago: i64,
    pub subject: String,
    pub messages_in_thread: i64,
    pub last_message_from_me: bool,
    pub recent_messages: Vec<RouteMessage>,
    /// "filed in <project>" → how many earlier threads with these people.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sender_history: Vec<(String, i64)>,
    /// Other threads with these people, filed or not.
    pub peer_threads: i64,
    /// Those of them the user never filed in any folder.
    pub unfiled_peers: i64,
    /// The user's earlier yes/no answers for this sender: ("yes: Archive · Voyage", n).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub earlier_answers: Vec<(String, i64)>,
}

impl RouteInput {
    fn state(&self) -> Value {
        let mut v = json!({
            "today": self.today,
            "last_activity": ago(self.last_activity_days_ago),
            "last_activity_days_ago": self.last_activity_days_ago,
            "thread_started_days_ago": self.thread_started_days_ago,
            "subject": self.subject,
            "messages_in_thread": self.messages_in_thread,
            "last_message_from_me": self.last_message_from_me,
            "recent_messages": self.recent_messages,
        });
        if !self.sender_history.is_empty() {
            let mut h: serde_json::Map<String, Value> = self
                .sender_history
                .iter()
                .map(|(p, n)| (format!("filed in {p}"), json!(n)))
                .collect();
            // Without this, "filed in X: 88" reads as a habit even when the user left
            // 1,000 other threads with the same people unfiled.
            h.insert("not filed in any folder".into(), json!(self.unfiled_peers));
            v["sender_history"] = Value::Object(h);
        }
        if !self.earlier_answers.is_empty() {
            let a: serde_json::Map<String, Value> = self
                .earlier_answers
                .iter()
                .map(|(k, n)| (k.clone(), json!(n)))
                .collect();
            v["user_answers_for_this_sender"] = Value::Object(a);
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
            "In which of the user's email folders should this thread be filed? Most of the user's mail is never filed: answer `none` unless the thread clearly belongs in a folder. If `sender_history` is present, it counts earlier threads with the same people per folder and those never filed; a folder is good evidence only when it holds most of them. `user_answers_for_this_sender` lists suggestions the user accepted (yes) or turned down (no) for this sender." },
        "status": { "type": "choice", "criteria": status, "instructions":
            "Where does this thread belong in the user's workflow once they have read it? `last_activity` says how long ago anyone last wrote in the thread." },
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

/// Drops what cannot hold for a thread silent past [`STALE_AFTER_DAYS`]: any status but
/// Done or Archive, and priority. Projects stay, since filing does not age.
pub fn guard_stale(mut sg: Suggestion, last_activity_days_ago: i64) -> Suggestion {
    if !is_stale(last_activity_days_ago) {
        return sg;
    }
    if !sg
        .status
        .as_deref()
        .is_some_and(|s| STALE_STATUSES.contains(&s))
    {
        sg.status = None;
        sg.status_confidence = None;
    }
    sg.priority = None;
    sg
}

/// A suggested folder must hold at least this share of the threads the user had with
/// the same people. On the user's mailbox the flooded suggestions went to folders
/// holding a median 6% of those threads.
pub const MIN_FOLDER_SHARE: f64 = 0.5;

/// Below this many earlier threads with the same people the share says too little;
/// Jev's answer from the folder descriptions stands.
pub const MIN_PEERS_FOR_SHARE: i64 = 3;

/// Drops a suggested folder that holds less than [`MIN_FOLDER_SHARE`] of the threads
/// the user had with the same people.
pub fn guard_folder_share(mut sg: Suggestion, history: &[(String, i64)], peers: i64) -> Suggestion {
    if peers < MIN_PEERS_FOR_SHARE || sg.projects.is_empty() {
        return sg;
    }
    let filed_there = |p: &String| history.iter().find(|(n, _)| n == p).map_or(0, |(_, c)| *c);
    if sg
        .projects
        .iter()
        .any(|p| (filed_there(p) as f64) / (peers as f64) < MIN_FOLDER_SHARE)
    {
        sg.projects.clear();
        sg.project_confidence = None;
    }
    sg
}

/// Bar for suggestions on threads already sorted into a column: below it the thread
/// keeps its place silently, so the user only reviews moves Jev is sure about.
pub const COLUMN_MIN_CONFIDENCE: f64 = 0.7;

/// Safety net on background calls per day (~$6 at measured prices), not a pacing tool.
pub const DEFAULT_DAILY_CAP: i64 = 20_000;

/// Keeps what a column suggestion would actually change: a confident status other than
/// the current one, and confident projects the thread is not in yet. Priority is left
/// out: chips and groups show status and projects, and accepting applies only what is
/// shown. `None` means "no change".
pub fn column_suggestion(
    sg: &Suggestion,
    current_status: &str,
    current_projects: &[String],
) -> Option<Suggestion> {
    let status = sg
        .status
        .clone()
        .filter(|s| s != current_status)
        .filter(|_| sg.status_confidence.unwrap_or(0.0) >= COLUMN_MIN_CONFIDENCE);
    let projects: Vec<String> = if sg.project_confidence.unwrap_or(0.0) >= COLUMN_MIN_CONFIDENCE {
        sg.projects
            .iter()
            .filter(|p| !current_projects.contains(p))
            .cloned()
            .collect()
    } else {
        vec![]
    };
    if status.is_none() && projects.is_empty() {
        return None;
    }
    Some(Suggestion {
        status_confidence: status.as_ref().and(sg.status_confidence),
        project_confidence: (!projects.is_empty())
            .then_some(sg.project_confidence)
            .flatten(),
        status,
        projects,
        priority: None,
        automated: sg.automated,
    })
}

/// How many more background calls fit today's cap.
pub fn calls_left(used_today: i64, cap: i64) -> i64 {
    (cap - used_today).max(0)
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
        Ok(self.ask_metered(state, questions).await?.0)
    }

    /// `systemone` call returning the answers and the input tokens billed, when reported.
    async fn ask_metered(&self, state: &Value, questions: &Value) -> Result<(Value, Option<i64>)> {
        let resp = self
            .http
            .post(format!("{}/v1/systemone", self.base))
            .bearer_auth(&self.key)
            .json(&json!({ "state": state, "model": MODEL, "questions": questions }))
            .send()
            .await?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let retry = retry_after(
                resp.headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok()),
            );
            let body = resp.text().await.unwrap_or_default();
            return Err(match status {
                401 | 403 => Error::InvalidParams(
                    "The TypeSafe key was refused. Check it in Settings › Developer.".into(),
                ),
                402 => Error::OutOfCredits,
                429 => Error::RateLimited { retry_after: retry },
                _ => Error::Upstream { status, body },
            });
        }
        let v: Value = resp.json().await?;
        Ok((v["answers"].clone(), v["usage"]["input_tokens"].as_i64()))
    }

    pub async fn route(
        &self,
        input: &RouteInput,
        questions: &Value,
        known: &[String],
    ) -> Result<Routed> {
        let (answers, input_tokens) = self.ask_metered(&input.state(), questions).await?;
        Ok(Routed {
            suggestion: guard_folder_share(
                guard_stale(suggestion(&answers, known), input.last_activity_days_ago),
                &input.sender_history,
                input.peer_threads,
            ),
            input_tokens,
        })
    }
}

/// A routed thread and what the call cost, for the logs.
#[derive(Debug, Clone, PartialEq)]
pub struct Routed {
    pub suggestion: Suggestion,
    pub input_tokens: Option<i64>,
}

/// TypeSafe's `Retry-After` is in milliseconds; anything unreadable means "not said".
pub fn retry_after(header: Option<&str>) -> Option<Duration> {
    header?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_millis)
}

/// Wait after a rate limit or outage: what the server asked for, else double the last
/// wait (from 2 s, up to 5 min) so a struggling service is not hammered.
pub fn backoff(asked: Option<Duration>, previous: Option<Duration>) -> Duration {
    const FIRST: Duration = Duration::from_secs(2);
    const MAX: Duration = Duration::from_secs(300);
    asked
        .unwrap_or_else(|| previous.map_or(FIRST, |p| p * 2))
        .min(MAX)
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

    /// A one-message thread last written to `days` ago.
    fn route_input(days: i64) -> RouteInput {
        RouteInput {
            today: "2026-10-12".into(),
            last_activity_days_ago: days,
            thread_started_days_ago: days + 10,
            subject: "Devis".into(),
            messages_in_thread: 1,
            last_message_from_me: false,
            recent_messages: vec![],
            sender_history: vec![],
            peer_threads: 0,
            unfiled_peers: 0,
            earlier_answers: vec![],
        }
    }

    fn filed(project: &str) -> Suggestion {
        Suggestion {
            projects: vec![project.into()],
            project_confidence: Some(0.95),
            ..Default::default()
        }
    }

    #[rstest]
    #[case::folder_holds_most(vec![("Voyage", 8)], 10, vec!["Voyage"])]
    #[case::folder_holds_a_few(vec![("Voyage", 2)], 40, vec![])]
    #[case::exactly_half(vec![("Voyage", 5)], 10, vec!["Voyage"])]
    #[case::people_never_filed_there(vec![("Immo", 9)], 10, vec![])]
    #[case::too_few_threads_to_judge(vec![], 2, vec!["Voyage"])]
    fn folder_must_hold_most_of_these_people_s_threads(
        #[case] history: Vec<(&str, i64)>,
        #[case] peers: i64,
        #[case] want: Vec<&str>,
    ) {
        let history: Vec<(String, i64)> = history.into_iter().map(|(p, n)| (p.into(), n)).collect();
        assert_eq!(
            guard_folder_share(filed("Voyage"), &history, peers).projects,
            want
        );
    }

    #[rstest]
    fn dropped_folder_keeps_the_status() {
        let sg = Suggestion {
            status: Some("archive".into()),
            ..filed("Voyage")
        };
        assert_eq!(
            guard_folder_share(sg, &[], 40).status.as_deref(),
            Some("archive")
        );
    }

    #[rstest]
    fn sender_history_tells_the_unfiled_threads() {
        let input = RouteInput {
            sender_history: vec![("Immo".into(), 4)],
            peer_threads: 40,
            unfiled_peers: 36,
            ..route_input(4)
        };
        assert_eq!(
            input.state()["sender_history"]["not filed in any folder"],
            36
        );
    }

    #[rstest]
    fn earlier_answers_reach_jev() {
        let input = RouteInput {
            earlier_answers: vec![("no: Done · Mariage".into(), 2)],
            ..route_input(4)
        };
        assert_eq!(
            input.state()["user_answers_for_this_sender"]["no: Done · Mariage"],
            2
        );
    }

    #[rstest]
    fn project_question_says_most_mail_is_not_filed() {
        assert!(
            questions(&[])["project"]["instructions"]
                .as_str()
                .unwrap()
                .contains("never filed")
        );
    }

    #[rstest]
    fn sender_history_reads_as_filed_in() {
        let input = RouteInput {
            sender_history: vec![("Immo/Houlgate".into(), 4)],
            ..route_input(4)
        };
        assert_eq!(input.state()["sender_history"]["filed in Immo/Houlgate"], 4);
    }

    #[rstest]
    fn empty_sender_history_is_omitted() {
        assert!(route_input(4).state().get("sender_history").is_none());
    }

    #[rstest]
    #[case::today("today", json!("2026-10-12"))]
    #[case::last_activity_in_words("last_activity", json!("10 years ago"))]
    #[case::last_activity_in_days("last_activity_days_ago", json!(3650))]
    #[case::thread_start("thread_started_days_ago", json!(3660))]
    fn state_tells_the_thread_age(#[case] field: &str, #[case] want: Value) {
        assert_eq!(route_input(3650).state()[field], want);
    }

    #[rstest]
    fn status_question_points_at_the_age() {
        assert!(
            questions(&[])["status"]["instructions"]
                .as_str()
                .unwrap()
                .contains("`last_activity`")
        );
    }

    #[rstest]
    #[case::same_day(0, "today")]
    #[case::one_day(1, "yesterday")]
    #[case::days(9, "9 days ago")]
    #[case::weeks(20, "2 weeks ago")]
    #[case::months(100, "3 months ago")]
    #[case::years(3700, "10 years ago")]
    fn age_reads_as_words(#[case] days: i64, #[case] want: &str) {
        assert_eq!(ago(days), want);
    }

    #[rstest]
    #[case::exact_day(10 * DAY_MS, 10)]
    #[case::partial_day_rounds_down(10 * DAY_MS - 1, 9)]
    fn days_ago_counts_whole_days(#[case] age_ms: i64, #[case] want: i64) {
        assert_eq!(days_ago(1_000 * DAY_MS - age_ms, 1_000 * DAY_MS), want);
    }

    fn live(status: &str) -> Suggestion {
        Suggestion {
            status: Some(status.into()),
            status_confidence: Some(0.9),
            projects: vec!["Immo".into()],
            project_confidence: Some(0.9),
            priority: Some(1),
            automated: Some(0.1),
        }
    }

    #[rstest]
    #[case::waiting_on_a_recent_thread("waiting", 30, Some("waiting"))]
    #[case::waiting_at_the_threshold("waiting", STALE_AFTER_DAYS, Some("waiting"))]
    #[case::waiting_past_the_threshold("waiting", STALE_AFTER_DAYS + 1, None)]
    #[case::todo_ten_years_on("todo", 3650, None)]
    #[case::in_progress_ten_years_on("in_progress", 3650, None)]
    #[case::backlog_ten_years_on("backlog", 3650, None)]
    #[case::done_ten_years_on("done", 3650, Some("done"))]
    #[case::archive_ten_years_on("archive", 3650, Some("archive"))]
    fn stale_threads_only_get_closing_statuses(
        #[case] status: &str,
        #[case] days: i64,
        #[case] want: Option<&str>,
    ) {
        assert_eq!(guard_stale(live(status), days).status.as_deref(), want);
    }

    #[rstest]
    fn dropped_status_drops_its_confidence() {
        assert_eq!(guard_stale(live("waiting"), 3650).status_confidence, None);
    }

    #[rstest]
    fn stale_threads_keep_their_project() {
        assert_eq!(guard_stale(live("waiting"), 3650).projects, vec!["Immo"]);
    }

    #[rstest]
    #[case::recent(30, Some(1))]
    #[case::stale(3650, None)]
    fn stale_threads_have_no_priority(#[case] days: i64, #[case] want: Option<i64>) {
        assert_eq!(guard_stale(live("todo"), days).priority, want);
    }

    fn column(status: (&str, f64), project: (&str, f64)) -> Suggestion {
        Suggestion {
            status: Some(status.0.into()),
            status_confidence: Some(status.1),
            projects: vec![project.0.into()],
            project_confidence: Some(project.1),
            priority: Some(2),
            automated: Some(0.9),
        }
    }

    #[rstest]
    fn same_status_and_project_is_no_change() {
        let sg = column(("backlog", 0.9), ("Immo", 0.9));
        assert_eq!(column_suggestion(&sg, "backlog", &["Immo".into()]), None);
    }

    #[rstest]
    #[case::new_status("backlog", Some("archive"))]
    #[case::same_status("archive", None)]
    fn column_status_is_kept_only_when_it_moves(#[case] current: &str, #[case] want: Option<&str>) {
        let sg = column(("archive", 0.9), ("Immo", 0.9));
        let got = column_suggestion(&sg, current, &[]).unwrap();
        assert_eq!(got.status.as_deref(), want);
    }

    #[rstest]
    #[case::not_filed_yet(vec![], vec!["Immo"])]
    #[case::already_filed(vec!["Immo"], vec![])]
    fn column_projects_are_only_new_ones(#[case] current: Vec<&str>, #[case] want: Vec<&str>) {
        let sg = column(("archive", 0.9), ("Immo", 0.9));
        let current: Vec<String> = current.into_iter().map(String::from).collect();
        let got = column_suggestion(&sg, "backlog", &current).unwrap();
        assert_eq!(got.projects, want);
    }

    #[rstest]
    #[case::confident(0.7, Some("archive"))]
    #[case::unsure(0.69, None)]
    fn column_status_needs_the_column_bar(#[case] confidence: f64, #[case] want: Option<&str>) {
        let sg = column(("archive", confidence), ("Immo", 0.9));
        let got = column_suggestion(&sg, "backlog", &[]).unwrap();
        assert_eq!(got.status.as_deref(), want);
    }

    #[rstest]
    fn unsure_status_alone_is_no_change() {
        let sg = column(("archive", 0.6), ("Immo", 0.9));
        assert_eq!(column_suggestion(&sg, "backlog", &["Immo".into()]), None);
    }

    #[rstest]
    fn column_suggestion_never_carries_priority() {
        let sg = column(("archive", 0.9), ("Immo", 0.9));
        assert_eq!(
            column_suggestion(&sg, "backlog", &[]).unwrap().priority,
            None
        );
    }

    #[rstest]
    #[case::millis(Some("1500"), Some(Duration::from_millis(1500)))]
    #[case::missing(None, None)]
    #[case::garbage(Some("soon"), None)]
    fn retry_after_reads_milliseconds(#[case] h: Option<&str>, #[case] want: Option<Duration>) {
        assert_eq!(retry_after(h), want);
    }

    #[rstest]
    #[case::server_asked(
        Some(Duration::from_millis(800)),
        Some(Duration::from_secs(8)),
        Duration::from_millis(800)
    )]
    #[case::first_failure(None, None, Duration::from_secs(2))]
    #[case::doubles(None, Some(Duration::from_secs(4)), Duration::from_secs(8))]
    #[case::capped(None, Some(Duration::from_secs(250)), Duration::from_secs(300))]
    fn backoff_honours_the_server_then_doubles(
        #[case] asked: Option<Duration>,
        #[case] previous: Option<Duration>,
        #[case] want: Duration,
    ) {
        assert_eq!(backoff(asked, previous), want);
    }

    #[rstest]
    #[case::fresh_day(0, 2000, 2000)]
    #[case::partly_used(1500, 2000, 500)]
    #[case::at_cap(2000, 2000, 0)]
    #[case::cap_lowered_below_use(2500, 2000, 0)]
    fn calls_left_never_goes_negative(#[case] used: i64, #[case] cap: i64, #[case] want: i64) {
        assert_eq!(calls_left(used, cap), want);
    }

    #[rstest]
    fn description_lists_people_and_subjects() {
        assert_eq!(
            describe("Immo", &["Oralia".into()], &["PV assemblée".into()]),
            "Folder 'Immo'. Frequent correspondents: Oralia. Example subjects: PV assemblée."
        );
    }
}
