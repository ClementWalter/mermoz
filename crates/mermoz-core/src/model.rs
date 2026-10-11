//! Protocol objects, serialized exactly as documented in `docs/protocol.md`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncInfo {
    pub state: String,
    pub synced: i64,
    pub total: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub provider: String,
    pub sync: SyncInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadSummary {
    pub id: String,
    pub number: String,
    pub account_id: String,
    pub subject: String,
    pub snippet: String,
    pub participants: Vec<String>,
    pub last_from: String,
    pub last_date: i64,
    pub unread: bool,
    pub message_count: i64,
    pub has_attachments: bool,
    pub status: String,
    pub priority: i64,
    pub project_id: Option<String>,
    pub due: Option<String>,
    pub summary: Option<String>,
    pub next_action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Address {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachmentInfo {
    pub id: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub date: i64,
    pub subject: String,
    pub body_text: String,
    /// `body_text` without the quoted history of earlier messages.
    pub body_new: String,
    pub has_quoted: bool,
    pub has_html: bool,
    pub is_from_me: bool,
    pub attachments: Vec<AttachmentInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Status {
    pub key: String,
    pub name: String,
    pub order: i64,
    pub color: String,
}

/// A Gmail label the user files mail into ("mailbox"); `id` is unique within its account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Label {
    pub account_id: String,
    pub id: String,
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Draft {
    pub id: String,
    pub account_id: String,
    pub thread_id: Option<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub state: String,
    pub origin: String,
    pub created: i64,
    pub updated: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Feedback {
    pub id: String,
    pub text: String,
    pub command: Option<String>,
    pub agent: Option<String>,
    pub created: i64,
}

/// Default workflow, Linear-style. `archived` statuses remove the thread from the Gmail inbox.
pub const DEFAULT_STATUSES: &[(&str, &str, &str, bool)] = &[
    ("triage", "Triage", "#8A6FD1", false),
    ("backlog", "Backlog", "#9AAFC0", false),
    ("todo", "Todo", "#5A7386", false),
    ("in_progress", "In Progress", "#D9902F", false),
    ("waiting", "Waiting", "#2F8FCF", false),
    ("review", "Review", "#C2568C", false),
    ("done", "Done", "#2E9A6B", true),
    ("canceled", "Canceled", "#9AAFC0", true),
];

pub fn priority_name(p: i64) -> Option<&'static str> {
    match p {
        1 => Some("Urgent"),
        2 => Some("High"),
        3 => Some("Normal"),
        4 => Some("Low"),
        _ => None,
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Short random id with a type prefix; ids are opaque to clients.
pub fn new_id(prefix: &str) -> String {
    use rand::Rng;
    let n: u64 = rand::rng().random();
    format!("{prefix}_{n:016x}")
}
