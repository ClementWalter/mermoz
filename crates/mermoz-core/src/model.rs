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
    /// Left out of every list, count, search and suggestion; still synced.
    #[serde(default)]
    pub hidden: bool,
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
    /// The thread's projects: names of the user's Gmail labels on it.
    pub projects: Vec<String>,
    pub due: Option<String>,
    pub summary: Option<String>,
    pub next_action: Option<String>,
    /// The router's pending prediction, until the thread is read or the user decides.
    pub suggestion: Option<Suggestion>,
    /// The conversation this Gmail thread belongs to (its own id while alone).
    #[serde(default)]
    pub conversation_id: String,
    /// "person", "feed" (automated sender) or "list" (mailing list).
    #[serde(default)]
    pub conversation_kind: String,
    /// Gmail threads in the conversation, this one included.
    #[serde(default)]
    pub conversation_threads: i64,
    /// The user's own wording for the card, shown instead of the subject.
    #[serde(default)]
    pub title: Option<String>,
}

/// The fields of a thread a group accept can change, as they were before it: what
/// `threads.restore` puts back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadState {
    pub id: String,
    pub unread: bool,
    pub status: String,
    pub priority: i64,
    pub projects: Vec<String>,
    pub due: Option<String>,
}

impl From<&ThreadSummary> for ThreadState {
    fn from(t: &ThreadSummary) -> Self {
        Self {
            id: t.id.clone(),
            unread: t.unread,
            status: t.status.clone(),
            priority: t.priority,
            projects: t.projects.clone(),
            due: t.due.clone(),
        }
    }
}

/// What the router predicts for a thread. Fields below the confidence bar are absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Suggestion {
    pub status: Option<String>,
    pub status_confidence: Option<f64>,
    pub projects: Vec<String>,
    pub project_confidence: Option<f64>,
    pub priority: Option<i64>,
    /// Probability the thread was sent by a machine (newsletter, receipt, notification).
    pub automated: Option<f64>,
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
    /// An image placed in the body (shown there), not a file sent along.
    #[serde(default)]
    pub inline: bool,
    /// Decoration (signature logo, social icon, banner, tracking pixel), not content:
    /// never shown as a photo nor counted in Photos. See [`crate::decorative`].
    #[serde(default)]
    pub decorative: bool,
}

fn one() -> u32 {
    1
}

/// A reaction (Outlook/Teams "[heart] Nora reacted to your message") to another message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reaction {
    /// The emoji ("❤️", "👍", "😆", "😮", "😢", "🎉", "😠").
    pub emoji: String,
    /// The message reacted to, when it is in this conversation.
    pub target_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: String,
    /// The Gmail thread it belongs to, inside a conversation of several.
    #[serde(default)]
    pub thread_id: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    /// Blind copies: only known on messages the user sent.
    pub bcc: Vec<Address>,
    /// Where the sender wants replies (`Reply-To`); empty when they want them at `from`.
    #[serde(default)]
    pub reply_to: Vec<Address>,
    /// A delivery failure report (bounce) from the mail system, not a person.
    #[serde(default)]
    pub is_bounce: bool,
    /// An automatic answer (out of office, auto-acknowledgement).
    #[serde(default)]
    pub is_auto_reply: bool,
    /// This message didn't reach someone: what a bounce in the conversation says.
    #[serde(default)]
    pub delivery_failure: Option<DeliveryFailure>,
    pub date: i64,
    pub subject: String,
    pub body_text: String,
    /// What the bubble shows: only what the sender newly wrote (greeting, body, sign-off
    /// name), LF line ends, never empty when the message has text. See
    /// [`crate::reading`].
    pub body_new: String,
    /// Quoted history of earlier messages was folded out of `body_new`.
    pub has_quoted: bool,
    /// A signature, mobile footer or disclaimer was folded out of `body_new`.
    #[serde(default)]
    pub has_signature: bool,
    /// The message forwards an email: `body_new` is the note, then the forwarded text.
    #[serde(default)]
    pub forwarded: bool,
    /// Who wrote the forwarded email.
    #[serde(default)]
    pub forwarded_from: Option<String>,
    /// The message adds no words of its own: `body_new` is the text it quotes.
    #[serde(default)]
    pub quoted_only: bool,
    /// The message is only a reaction to another one (`body_new` is then its emoji).
    #[serde(default)]
    pub reaction: Option<Reaction>,
    /// Another delivery of the same email (a forwarding address, a double import): the
    /// id of the copy kept. A thread hides such copies.
    #[serde(default)]
    pub duplicate_of: Option<String>,
    /// How many times this email was received (1, or more when its copies are hidden).
    #[serde(default = "one")]
    pub copies: u32,
    /// Reading version written by the built-in agent, when it has processed this message.
    pub body_clean: Option<String>,
    pub has_html: bool,
    pub is_from_me: bool,
    pub attachments: Vec<AttachmentInfo>,
    /// Sender verification: "pass", "shared" (real domain on a platform anyone can use),
    /// "lookalike" (a copycat of the brand the name claims), "fail", "none", or null
    /// while not checked yet.
    pub auth: Option<String>,
    /// For a "lookalike", the domain this sender's name usually writes from.
    #[serde(default)]
    pub usual_domain: Option<String>,
    /// For a "new" or "lookalike" sender, what a web search found about who its name
    /// claims to be.
    #[serde(default)]
    pub sender_claim: Option<SenderClaim>,
}

/// Who a sender's name claims to be, and the domains that organisation really sends
/// email from, as found on the web.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SenderClaim {
    /// "organisation", "person" or "unknown".
    pub kind: String,
    pub organisation: String,
    /// Registered domains the organisation sends email from, most used first.
    pub domains: Vec<String>,
    /// Pages the domains were found on.
    pub sources: Vec<String>,
    /// Whether the sender's own domain is one of `domains`.
    #[serde(default)]
    pub matches: bool,
}

/// A recipient a sent message didn't reach, as a bounce reported it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliveryFailure {
    pub recipient: String,
    /// RFC 3463 status code, e.g. "5.1.1".
    pub status: String,
    /// Why, in plain words ("address doesn't exist").
    pub reason: String,
    /// The bounce message that reported it.
    pub bounce_message_id: String,
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

/// A project is one of the user's Gmail labels; `id` is its name, shared across accounts.
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
    #[serde(default)]
    pub attachments: Vec<DraftAttachment>,
    /// The message this reply answers: it threads onto it, is addressed from it and
    /// quotes it. None: the conversation's reply target ([`crate::reply::target`]).
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
}

/// How a draft carries something: the file's bytes, or a link to it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    #[default]
    File,
    Link,
}

/// A file (copied into Mermoz's data folder) or a link going out with a draft.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DraftAttachment {
    pub id: String,
    pub kind: AttachmentKind,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    /// The stored copy, for files.
    pub path: Option<String>,
    /// Where the item lives, for links.
    pub url: Option<String>,
    /// Shown in the message by mail apps rather than offered as a download.
    pub inline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Feedback {
    pub id: String,
    pub text: String,
    pub command: Option<String>,
    pub agent: Option<String>,
    pub created: i64,
}

/// Default workflow: Unread first, then what to do with read mail. `archived` statuses
/// remove the thread from the Gmail inbox.
pub const DEFAULT_STATUSES: &[(&str, &str, &str, bool)] = &[
    ("unread", "Unread", "#3D7FE0", false),
    ("todo", "Todo", "#5A7386", false),
    ("waiting", "Waiting", "#2F8FCF", false),
    ("in_progress", "In Progress", "#D9902F", false),
    ("backlog", "Backlog", "#9AAFC0", false),
    ("review", "Review", "#C2568C", false),
    ("done", "Done", "#2E9A6B", true),
    ("archive", "Archive", "#8FA3B5", true),
    ("deleted", "Deleted", "#C0645A", true),
];

/// Statuses a read thread can be routed to; Unread, Review and Deleted are never predicted.
pub const ROUTABLE_STATUSES: &[&str] = &[
    "todo",
    "waiting",
    "in_progress",
    "backlog",
    "done",
    "archive",
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

/// Someone in a conversation, with what the directory knows about them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationPerson {
    pub name: String,
    pub email: String,
    pub title: String,
    pub company: String,
    /// Messages they wrote in the conversation.
    pub messages: i64,
}

/// A web link shared in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SharedLink {
    pub url: String,
    pub message_id: String,
    pub from: String,
    pub date: i64,
}

/// A file shared in a conversation (or anywhere, for the Docs list).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SharedFile {
    pub id: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub message_id: String,
    pub thread_id: String,
    pub from: String,
    pub date: i64,
}

/// Everything the reading pane shows about one conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Conversation {
    pub id: String,
    pub kind: String,
    /// Its Gmail threads, oldest first.
    pub threads: Vec<ThreadSummary>,
    pub people: Vec<ConversationPerson>,
    pub files: Vec<SharedFile>,
    pub links: Vec<SharedLink>,
}
