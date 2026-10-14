//! "Ask Mermoz to do": a sentence typed in ⌘K becomes a typed plan of bulk changes.
//!
//! The account's AI only fills the fixed schema below (filters + one action per step);
//! it never executes anything. Every plan is checked against the schema and the user's
//! own statuses, projects and accounts before it is shown, and nothing in the schema
//! can send, reply to or forward mail.

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Steps one sentence may turn into.
pub const MAX_STEPS: usize = 5;

/// What the user's mailbox calls things, to check a plan against.
#[derive(Debug, Clone, Default)]
pub struct Vocab {
    /// (key, display name) of every status.
    pub statuses: Vec<(String, String)>,
    pub projects: Vec<String>,
    /// (id, email) of every account.
    pub accounts: Vec<(String, String)>,
}

impl Vocab {
    fn status_key(&self, s: &str) -> Option<String> {
        let s = s.trim();
        let norm = |x: &str| x.to_lowercase().replace([' ', '-'], "_");
        self.statuses
            .iter()
            .find(|(k, n)| norm(k) == norm(s) || norm(n) == norm(s))
            .map(|(k, _)| k.clone())
    }

    pub fn status_name(&self, key: &str) -> String {
        self.statuses
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| key.to_string())
    }

    fn project(&self, p: &str) -> Option<String> {
        self.projects
            .iter()
            .find(|x| x.eq_ignore_ascii_case(p.trim()))
            .cloned()
    }

    fn account(&self, a: &str) -> Option<String> {
        self.accounts
            .iter()
            .find(|(id, email)| id == a.trim() || email.eq_ignore_ascii_case(a.trim()))
            .map(|(id, _)| id.clone())
    }

    fn account_email(&self, id: &str) -> String {
        self.accounts
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, e)| e.clone())
            .unwrap_or_else(|| id.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Day,
    Week,
    Month,
    Year,
}

/// "5 months", relative to now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Age {
    pub amount: u32,
    pub unit: Unit,
}

impl Age {
    fn words(&self) -> String {
        let unit = match self.unit {
            Unit::Day => "day",
            Unit::Week => "week",
            Unit::Month => "month",
            Unit::Year => "year",
        };
        let s = if self.amount == 1 { "" } else { "s" };
        format!("{} {unit}{s}", self.amount)
    }
}

/// Which threads a step applies to; every field narrows, empty fields don't.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status_in: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status_not_in: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_than: Option<Age>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newer_than: Option<Age>,
    /// Addresses, domains or names; a thread matches when any of them wrote in it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<String>,
    /// In any of these projects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_projects: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachment: Option<bool>,
    /// Account id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Full-text words, as the search box takes them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// The subject starts with this ("Notes:"), case aside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_prefix: Option<String>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Any status but Unread, Review and Deleted (Deleted is [`Action::Delete`]).
    SetStatus {
        status: String,
    },
    AddProject {
        project: String,
    },
    RemoveProject {
        project: String,
    },
    MarkRead,
    MarkUnread,
    /// Later until that day, 08:00 local time.
    Later {
        until: String,
    },
    /// To Gmail Trash; asks for a second confirmation.
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub filter: Filter,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<Step>,
}

impl Plan {
    pub fn deletes(&self) -> bool {
        self.steps.iter().any(|s| s.action == Action::Delete)
    }
}

/// What the AI made of a sentence.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Plan(Plan),
    /// Not something a plan can do, or unclear: what to tell the user.
    Question(String),
}

// ---- the AI's side ---------------------------------------------------------

/// Structured output the AI must fill: strict (every key required, no others), so it
/// works the same on Claude, ChatGPT and OpenAI-compatible providers.
pub fn schema() -> Value {
    let str_list = json!({ "type": "array", "items": { "type": "string" } });
    let opt_bool = json!({ "type": ["boolean", "null"] });
    let opt_str = json!({ "type": ["string", "null"] });
    let age = json!({ "anyOf": [ { "type": "null" }, {
        "type": "object", "additionalProperties": false, "required": ["amount", "unit"],
        "properties": {
            "amount": { "type": "integer" },
            "unit": { "type": "string", "enum": ["day", "week", "month", "year"] }
        } } ] });
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["understood", "question", "steps"],
        "properties": {
            "understood": { "type": "boolean" },
            "question": opt_str,
            "steps": { "type": "array", "items": {
                "type": "object", "additionalProperties": false, "required": ["filter", "action"],
                "properties": {
                    "filter": {
                        "type": "object", "additionalProperties": false,
                        "required": ["status_in", "status_not_in", "older_than", "newer_than", "from", "projects",
                                     "not_projects", "unread", "has_attachment", "account", "query", "subject_prefix"],
                        "properties": {
                            "status_in": str_list, "status_not_in": str_list,
                            "older_than": age, "newer_than": age,
                            "from": str_list, "projects": str_list, "not_projects": str_list,
                            "unread": opt_bool, "has_attachment": opt_bool,
                            "account": opt_str, "query": opt_str, "subject_prefix": opt_str
                        }
                    },
                    "action": {
                        "type": "object", "additionalProperties": false,
                        "required": ["kind", "status", "project", "until"],
                        "properties": {
                            "kind": { "type": "string", "enum": ["set_status", "add_project", "remove_project",
                                                                  "mark_read", "mark_unread", "later", "delete"] },
                            "status": opt_str, "project": opt_str, "until": opt_str
                        }
                    }
                } } }
        }
    })
}

/// Instructions for the AI: the vocabulary of this mailbox and the rules of the schema.
pub fn system_prompt(v: &Vocab, today: NaiveDate) -> String {
    let statuses = v
        .statuses
        .iter()
        .map(|(k, n)| format!("{k} ({n})"))
        .collect::<Vec<_>>()
        .join(", ");
    let projects = if v.projects.is_empty() {
        "(none)".to_string()
    } else {
        v.projects.join(", ")
    };
    let accounts = v
        .accounts
        .iter()
        .map(|(_, e)| e.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "You turn one instruction about the user's email into a plan for the Mermoz mail client.\n\
You never act: the user reviews the plan and Mermoz runs it. Today is {today} ({weekday}).\n\n\
Each step has a filter (which email threads) and one action.\n\
Filter fields (leave empty / null when not said):\n\
- status_in / status_not_in: status keys. Statuses: {statuses}. \"done\" means handled, \"archive\" filed away, \"deleted\" is Gmail Trash.\n\
- older_than / newer_than: age of the thread's latest email, e.g. {{\"amount\": 5, \"unit\": \"month\"}}.\n\
- from: who wrote: an email address, a domain (example.com) or a person's or organization's name, \
never a description of the mail. Spread the words: \"notes from gemini in google meet\" is from Gemini \
(its address among the candidates) with subject_prefix \"Notes:\", not from \"gemini notes from google meet\". \
Words may be misspelled: match them to the candidates.\n\
- projects / not_projects: the user's projects (Gmail labels): {projects}.\n\
- unread: true for unread threads, false for read ones. has_attachment: true / false.\n\
- account: one of {accounts}. query: words the emails must contain (only when the user names words to find).\n\
- subject_prefix: the beginning of the subject, e.g. \"Notes:\" (for mail that always starts its subject the same way).\n\
Actions (kind):\n\
- set_status with status (a status key; archive and done included, never unread, review or deleted)\n\
- add_project / remove_project with project\n\
- mark_read, mark_unread\n\
- later with until (YYYY-MM-DD, a day after today): hide the threads until that morning\n\
- delete: move to Gmail Trash\n\
Use several steps only when the user asks for several things.\n\
The message may list candidates Mermoz found in the mailbox for the instruction's words: sender \
addresses and subject beginnings, with thread counts. When the instruction points at one of them, \
use it exactly (from with the address, or subject_prefix) rather than asking. It may also hold earlier \
rounds: your question and the user's answer; use the answer.\n\
There is no action to send, reply, forward or write email, and none to unsubscribe or create rules. \
If the instruction asks for anything the actions above can't do, or is too unclear to act on safely, \
set understood to false, steps to [], and put in question one short sentence, in the user's language, \
saying what you can't do or what to make clear. Otherwise understood is true and question is null.",
        weekday = today.weekday(),
    )
}

/// A sender found locally for the instruction's words.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateSender {
    pub email: String,
    pub name: String,
    pub threads: i64,
    /// How its threads' subjects usually begin ("Notes:"), most common first.
    #[serde(default)]
    pub subjects: Vec<String>,
}

impl CandidateSender {
    /// "Gemini <gemini@google.example>", or the address alone.
    pub fn display(&self) -> String {
        if self.name.is_empty() {
            self.email.clone()
        } else {
            format!("{} <{}>", self.name, self.email)
        }
    }
}

/// A subject beginning many threads share ("Notes:").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateSubject {
    pub prefix: String,
    pub threads: i64,
}

/// What the mailbox holds for the instruction's words: aggregates only (sender
/// addresses and names, subject beginnings, counts), never bodies.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Candidates {
    pub senders: Vec<CandidateSender>,
    pub subjects: Vec<CandidateSubject>,
}

impl Candidates {
    pub fn is_empty(&self) -> bool {
        self.senders.is_empty() && self.subjects.is_empty()
    }
}

/// A filter the user picked among the candidates; it replaces the plan's own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pick {
    From(String),
    SubjectPrefix(String),
}

impl Pick {
    fn words(&self) -> String {
        match self {
            Pick::From(e) => format!("sender {e}"),
            Pick::SubjectPrefix(p) => format!("subject starting with \"{p}\""),
        }
    }

    /// Puts the picked filter in every step, in place of the sender, words and
    /// subject the AI chose, so the plan uses exactly what the user picked.
    pub fn apply(&self, plan: &mut Plan) {
        for s in &mut plan.steps {
            s.filter.from.clear();
            s.filter.query = None;
            s.filter.subject_prefix = None;
            match self {
                Pick::From(e) => s.filter.from = vec![e.clone()],
                Pick::SubjectPrefix(p) => s.filter.subject_prefix = Some(p.clone()),
            }
        }
    }
}

/// The user's turn for the AI: the instruction, earlier (question, answer) rounds,
/// the local candidates and any pick.
pub fn user_prompt(
    sentence: &str,
    rounds: &[(String, String)],
    c: &Candidates,
    pick: Option<&Pick>,
) -> String {
    let mut out = format!("Instruction: {sentence}");
    for (q, a) in rounds {
        out.push_str(&format!("\nYou asked: {q}\nThe user answered: {a}"));
    }
    if !c.is_empty() {
        out.push_str("\n\nCandidates in the mailbox for these words:");
        for s in &c.senders {
            let subjects = if s.subjects.is_empty() {
                String::new()
            } else {
                let quoted: Vec<String> = s.subjects.iter().map(|p| format!("\"{p}\"")).collect();
                format!(" · subjects {}", quoted.join(", "))
            };
            out.push_str(&format!(
                "\n- sender {} · {} threads{subjects}",
                s.display(),
                s.threads
            ));
        }
        for s in &c.subjects {
            out.push_str(&format!(
                "\n- subject starts with \"{}\" · {} threads",
                s.prefix, s.threads
            ));
        }
    }
    if let Some(p) = pick {
        out.push_str(&format!(
            "\n\nThe user picked the {}: use it as the filter.",
            p.words()
        ));
    }
    out
}

/// The action an instruction's verb names, for when the user picked a filter but the
/// AI still asks: "delete …", "archive …", "mark … read".
pub fn action_from_sentence(sentence: &str) -> Option<Action> {
    let w = senders::words(sentence);
    let has = |xs: &[&str]| w.iter().any(|x| xs.contains(&x.as_str()));
    if has(&[
        "delete",
        "trash",
        "remove",
        "supprime",
        "supprimer",
        "efface",
        "effacer",
        "jette",
    ]) {
        return Some(Action::Delete);
    }
    if has(&["archive", "archiver", "archives"]) {
        return Some(Action::SetStatus {
            status: "archive".into(),
        });
    }
    if has(&["unread"]) || senders::fold(sentence).contains("non lu") {
        return Some(Action::MarkUnread);
    }
    if has(&["read", "lu", "lus"]) {
        return Some(Action::MarkRead);
    }
    if has(&["done", "termine", "fini"]) {
        return Some(Action::SetStatus {
            status: "done".into(),
        });
    }
    None
}

// ---- checking a plan -------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlan {
    #[serde(default)]
    understood: Option<bool>,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    steps: Vec<RawStep>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    #[serde(default)]
    filter: Option<RawFilter>,
    action: RawAction,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawFilter {
    #[serde(default)]
    status_in: Option<Vec<String>>,
    #[serde(default)]
    status_not_in: Option<Vec<String>>,
    #[serde(default)]
    older_than: Option<RawAge>,
    #[serde(default)]
    newer_than: Option<RawAge>,
    #[serde(default)]
    from: Option<Vec<String>>,
    #[serde(default)]
    projects: Option<Vec<String>>,
    #[serde(default)]
    not_projects: Option<Vec<String>>,
    #[serde(default)]
    unread: Option<bool>,
    #[serde(default)]
    has_attachment: Option<bool>,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    subject_prefix: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAge {
    amount: i64,
    unit: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAction {
    kind: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    until: Option<String>,
}

/// Words an action kind may not contain: Mermoz never writes mail on its own.
const OUTGOING: &[&str] = &[
    "send", "reply", "forward", "draft", "compose", "email", "mail",
];

/// Checks a plan (the AI's answer, or one passed back by a client) against the schema
/// and this mailbox. Anything unknown is refused with one plain sentence.
pub fn validate(v: &Value, vocab: &Vocab, today: NaiveDate) -> Result<Outcome, String> {
    let raw: RawPlan = serde_json::from_value(v.clone()).map_err(|e| {
        let e = e.to_string();
        match e.split_once("unknown field ") {
            Some((_, rest)) => format!(
                "The plan has a field Mermoz doesn't know: {}",
                rest.split(',').next().unwrap_or(rest)
            ),
            None => format!("The plan doesn't follow Mermoz's format: {e}"),
        }
    })?;
    if raw.understood == Some(false) || raw.steps.is_empty() {
        let q = raw
            .question
            .filter(|q| !q.trim().is_empty())
            .unwrap_or_else(|| "I couldn't turn that into changes to your mail.".into());
        return Ok(Outcome::Question(q.trim().to_string()));
    }
    if raw.steps.len() > MAX_STEPS {
        return Err(format!(
            "That is {} steps; Mermoz runs at most {MAX_STEPS} at once.",
            raw.steps.len()
        ));
    }
    let steps = raw
        .steps
        .into_iter()
        .map(|s| {
            Ok(Step {
                filter: filter(s.filter.unwrap_or_default(), vocab)?,
                action: action(s.action, vocab, today)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Outcome::Plan(Plan { steps }))
}

fn statuses(list: Option<Vec<String>>, vocab: &Vocab) -> Result<Vec<String>, String> {
    let mut out = vec![];
    for s in list.unwrap_or_default() {
        let k = vocab
            .status_key(&s)
            .ok_or_else(|| format!("There is no status called '{s}'."))?;
        if !out.contains(&k) {
            out.push(k);
        }
    }
    Ok(out)
}

fn projects(list: Option<Vec<String>>, vocab: &Vocab) -> Result<Vec<String>, String> {
    list.unwrap_or_default()
        .into_iter()
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            vocab
                .project(&p)
                .ok_or_else(|| format!("There is no project called '{p}'."))
        })
        .collect()
}

fn age(a: Option<RawAge>) -> Result<Option<Age>, String> {
    let Some(a) = a else { return Ok(None) };
    let unit = match a.unit.trim().trim_end_matches('s') {
        "day" => Unit::Day,
        "week" => Unit::Week,
        "month" => Unit::Month,
        "year" => Unit::Year,
        other => return Err(format!("'{other}' is not a unit of time Mermoz knows.")),
    };
    if !(1..=1200).contains(&a.amount) {
        return Err(format!("An age of {} {} makes no sense.", a.amount, a.unit));
    }
    Ok(Some(Age {
        amount: a.amount as u32,
        unit,
    }))
}

fn filter(f: RawFilter, vocab: &Vocab) -> Result<Filter, String> {
    let nonempty = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let account = match nonempty(f.account) {
        Some(a) => Some(
            vocab
                .account(&a)
                .ok_or_else(|| format!("There is no account '{a}' in Mermoz."))?,
        ),
        None => None,
    };
    Ok(Filter {
        status_in: statuses(f.status_in, vocab)?,
        status_not_in: statuses(f.status_not_in, vocab)?,
        older_than: age(f.older_than)?,
        newer_than: age(f.newer_than)?,
        from: f
            .from
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .map(sender_entry)
            .collect::<Result<_, _>>()?,
        projects: projects(f.projects, vocab)?,
        not_projects: projects(f.not_projects, vocab)?,
        unread: f.unread,
        has_attachment: f.has_attachment,
        account,
        query: nonempty(f.query),
        subject_prefix: nonempty(f.subject_prefix),
    })
}

/// A sender is an address, a domain, or a name of at most four words with no linking
/// words: "gemini notes from google meet" describes mail and is refused.
fn sender_entry(s: String) -> Result<String, String> {
    let linking = [
        "from", "of", "the", "in", "on", "about", "like", "de", "du", "des", "sur",
    ];
    if let senders::Entry::Words(_) = senders::entry(&s) {
        let w = senders::words(&s);
        if w.len() > 4 || w.iter().any(|x| linking.contains(&x.as_str())) {
            return Err(format!(
                "'{s}' is not a sender: put a person's or organization's name or an address in from, \
                 and words about the mail in subject_prefix or query."
            ));
        }
    }
    Ok(s)
}

fn action(a: RawAction, vocab: &Vocab, today: NaiveDate) -> Result<Action, String> {
    let kind = a.kind.trim().to_lowercase();
    let need = |v: Option<String>, what: &str| {
        v.map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("'{kind}' needs a {what}."))
    };
    match kind.as_str() {
        "set_status" => {
            let s = need(a.status, "status")?;
            let key = vocab
                .status_key(&s)
                .ok_or_else(|| format!("There is no status called '{s}'."))?;
            match key.as_str() {
                "deleted" => Ok(Action::Delete),
                "unread" | "review" => Err(format!(
                    "Mermoz can't move mail to {}.",
                    vocab.status_name(&key)
                )),
                _ => Ok(Action::SetStatus { status: key }),
            }
        }
        "add_project" => {
            let p = need(a.project, "project")?;
            Ok(Action::AddProject {
                project: vocab.project(&p).unwrap_or(p),
            })
        }
        "remove_project" => {
            let p = need(a.project, "project")?;
            Ok(Action::RemoveProject {
                project: vocab
                    .project(&p)
                    .ok_or_else(|| format!("There is no project called '{p}'."))?,
            })
        }
        "mark_read" => Ok(Action::MarkRead),
        "mark_unread" => Ok(Action::MarkUnread),
        "later" => {
            let u = need(a.until, "date")?;
            let d = NaiveDate::parse_from_str(&u, "%Y-%m-%d")
                .map_err(|_| format!("'{u}' is not a date (YYYY-MM-DD)."))?;
            if d <= today {
                return Err(format!("{u} is not after today."));
            }
            Ok(Action::Later { until: u })
        }
        "delete" => Ok(Action::Delete),
        k if OUTGOING.iter().any(|w| k.contains(w)) => {
            Err("Mermoz never sends, replies to or forwards mail for you.".into())
        }
        k => Err(format!("'{k}' is not something Mermoz can do in bulk.")),
    }
}

// ---- dates -----------------------------------------------------------------

/// The instant `age` before `now_ms` (calendar months and years).
pub fn cutoff(now_ms: i64, age: Age) -> i64 {
    let now = DateTime::<Utc>::from_timestamp_millis(now_ms).unwrap_or_default();
    let n = age.amount as i64;
    let back = match age.unit {
        Unit::Day => now.checked_sub_signed(chrono::Duration::days(n)),
        Unit::Week => now.checked_sub_signed(chrono::Duration::weeks(n)),
        Unit::Month => now.checked_sub_months(chrono::Months::new(age.amount)),
        Unit::Year => now.checked_sub_months(chrono::Months::new(age.amount.saturating_mul(12))),
    };
    back.map(|d| d.timestamp_millis()).unwrap_or(0)
}

/// Later's wake-up time: that day at 08:00, local time.
pub fn later_ms(until: &str) -> Option<i64> {
    let d = NaiveDate::parse_from_str(until, "%Y-%m-%d").ok()?;
    let at = d.and_hms_opt(8, 0, 0)?;
    Local
        .from_local_datetime(&at)
        .earliest()
        .map(|t| t.timestamp_millis())
}

// ---- plain words -----------------------------------------------------------

/// 1842 → "1,842".
pub fn count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn or_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// Today, local time: the day a plan's dates are checked against.
pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// "2026-10-20" → "Tue 20 Oct".
pub fn day_words(until: &str) -> String {
    NaiveDate::parse_from_str(until, "%Y-%m-%d")
        .map(|d| d.format("%a %-d %b").to_string())
        .unwrap_or_else(|_| until.to_string())
}

/// "Archive 1,842 threads older than 5 months that aren't Done".
pub fn sentence(step: &Step, n: usize, vocab: &Vocab) -> String {
    let what = format!("{} thread{}", count(n), if n == 1 { "" } else { "s" });
    let words = filter_words(&step.filter, vocab);
    let tail = if words.is_empty() {
        String::new()
    } else {
        format!(" {words}")
    };
    match &step.action {
        Action::SetStatus { status } if status == "archive" => format!("Archive {what}{tail}"),
        Action::SetStatus { status } => {
            format!("Move {what}{tail} to {}", vocab.status_name(status))
        }
        Action::AddProject { project } => format!("Add {what}{tail} to {project}"),
        Action::RemoveProject { project } => format!("Remove {what}{tail} from {project}"),
        Action::MarkRead => format!("Mark {what}{tail} as read"),
        Action::MarkUnread => format!("Mark {what}{tail} as unread"),
        Action::Later { until } => format!("Put off {what}{tail} until {}", day_words(until)),
        Action::Delete => format!("Delete {what}{tail} (to Gmail Trash)"),
    }
}

/// The filter as a phrase after "threads": "from nora@example.com older than 5 months
/// that aren't Done".
pub fn filter_words(f: &Filter, vocab: &Vocab) -> String {
    let names =
        |keys: &[String]| -> Vec<String> { keys.iter().map(|k| vocab.status_name(k)).collect() };
    let mut parts: Vec<String> = vec![];
    if let Some(u) = f.unread {
        parts.push(if u { "unread" } else { "already read" }.into());
    }
    if !f.from.is_empty() {
        parts.push(format!("from {}", or_list(&f.from)));
    }
    if !f.status_in.is_empty() {
        parts.push(format!("in {}", or_list(&names(&f.status_in))));
    }
    if !f.projects.is_empty() {
        parts.push(format!("in {}", or_list(&f.projects)));
    }
    if let Some(a) = f.older_than {
        parts.push(format!("older than {}", a.words()));
    }
    if let Some(a) = f.newer_than {
        parts.push(format!("from the last {}", a.words()));
    }
    if f.has_attachment == Some(true) {
        parts.push("with attachments".into());
    }
    if f.has_attachment == Some(false) {
        parts.push("without attachments".into());
    }
    if let Some(q) = &f.query {
        parts.push(format!("mentioning “{q}”"));
    }
    if let Some(p) = &f.subject_prefix {
        parts.push(format!("whose subject starts with “{p}”"));
    }
    if let Some(a) = &f.account {
        parts.push(format!("on {}", vocab.account_email(a)));
    }
    if !f.status_not_in.is_empty() {
        parts.push(format!("that aren't {}", or_list(&names(&f.status_not_in))));
    }
    if !f.not_projects.is_empty() {
        parts.push(format!("not in {}", or_list(&f.not_projects)));
    }
    parts.join(" ")
}

/// The filter as conditions: "status ≠ Done AND older than 5 months".
pub fn interpretation(f: &Filter, vocab: &Vocab) -> String {
    let names = |keys: &[String]| -> String {
        keys.iter()
            .map(|k| vocab.status_name(k))
            .collect::<Vec<_>>()
            .join(" or ")
    };
    let mut c: Vec<String> = vec![];
    if !f.status_in.is_empty() {
        c.push(format!("status = {}", names(&f.status_in)));
    }
    if !f.status_not_in.is_empty() {
        c.push(format!("status ≠ {}", names(&f.status_not_in)));
    }
    if let Some(a) = f.older_than {
        c.push(format!("older than {}", a.words()));
    }
    if let Some(a) = f.newer_than {
        c.push(format!("newer than {}", a.words()));
    }
    if !f.from.is_empty() {
        c.push(format!("from {}", f.from.join(" or ")));
    }
    if !f.projects.is_empty() {
        c.push(format!("project = {}", f.projects.join(" or ")));
    }
    if !f.not_projects.is_empty() {
        c.push(format!("project ≠ {}", f.not_projects.join(" or ")));
    }
    if let Some(u) = f.unread {
        c.push(if u { "unread" } else { "read" }.into());
    }
    if let Some(h) = f.has_attachment {
        c.push(if h { "has attachment" } else { "no attachment" }.into());
    }
    if let Some(a) = &f.account {
        c.push(format!("account = {}", vocab.account_email(a)));
    }
    if let Some(q) = &f.query {
        c.push(format!("contains “{q}”"));
    }
    if let Some(p) = &f.subject_prefix {
        c.push(format!("subject starts with “{p}”"));
    }
    if c.is_empty() {
        "every email in Mermoz".into()
    } else {
        c.join(" AND ")
    }
}

/// "Archive" — the action's name on its own, for a button or the Undo label.
pub fn verb(a: &Action, vocab: &Vocab) -> String {
    match a {
        Action::SetStatus { status } if status == "archive" => "Archive".into(),
        Action::SetStatus { status } => format!("Move to {}", vocab.status_name(status)),
        Action::AddProject { project } => format!("Add to {project}"),
        Action::RemoveProject { project } => format!("Remove from {project}"),
        Action::MarkRead => "Mark as read".into(),
        Action::MarkUnread => "Mark as unread".into(),
        Action::Later { .. } => "Later".into(),
        Action::Delete => "Delete".into(),
    }
}

pub mod senders;

#[cfg(test)]
mod tests;
