//! SQLite store: accounts, threads-as-issues, messages, FTS index, drafts and the
//! offline write-back queue. Reads use their own WAL connection so a long sync
//! transaction never stalls the UI.

use crate::error::{Error, Result};
use crate::mime::ParsedMessage;
use crate::model::*;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

mod addresses;
mod bodies;
mod conversations;
mod counts;
pub use conversations::{
    GroupingPlan, MergeUndo, MergedPair, MergedThread, PairCandidate, PairRecord,
};

mod duplicates;
mod feed;
pub mod gmail_drafts;
mod images;
mod local;
mod plan;
mod push;
mod reactions;
mod readings;
mod semantic;
mod sign_in;
mod visibility;
pub use feed::FEED_STATUSES;
pub use plan::{BulkOp, PlanMatch};

/// Writer settings. Each ingested message runs in a savepoint whose sub-journal stays in
/// memory, and a 32 MB page cache keeps a sync batch's pages from being evicted and
/// read back mid-transaction (SQLite's default is 2 MB). No commit checkpoints: copying
/// the WAL back runs after the write lock is released ([`Store::checkpoint`]), and the
/// WAL file is trimmed back to 64 MB whenever it starts over.
const PRAGMAS: &str =
    "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;
     PRAGMA temp_store = MEMORY; PRAGMA cache_size = -32768; PRAGMA wal_autocheckpoint = 0;
     PRAGMA journal_size_limit = 67108864;";

/// Reader settings: 16 MB of cache each keeps the hot list and index pages resident.
const READER_PRAGMAS: &str =
    "PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY; PRAGMA cache_size = -16384;";

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
  id TEXT PRIMARY KEY, email TEXT NOT NULL UNIQUE, provider TEXT NOT NULL,
  history_id TEXT, cursor TEXT, sync_state TEXT NOT NULL DEFAULT 'initial',
  synced INTEGER NOT NULL DEFAULT 0, total INTEGER NOT NULL DEFAULT 0, error TEXT,
  created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS labels (
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  label_id TEXT NOT NULL, name TEXT NOT NULL, PRIMARY KEY (account_id, label_id));
CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, color TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS statuses (key TEXT PRIMARY KEY, name TEXT NOT NULL, ord INTEGER NOT NULL,
  color TEXT NOT NULL, archived INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS threads (
  id TEXT PRIMARY KEY, account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  provider_thread_id TEXT NOT NULL, number INTEGER NOT NULL UNIQUE,
  subject TEXT NOT NULL DEFAULT '', snippet TEXT NOT NULL DEFAULT '',
  participants TEXT NOT NULL DEFAULT '[]', last_from TEXT NOT NULL DEFAULT '',
  last_date INTEGER NOT NULL DEFAULT 0, unread INTEGER NOT NULL DEFAULT 0,
  message_count INTEGER NOT NULL DEFAULT 0, has_attachments INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL, priority INTEGER NOT NULL DEFAULT 0, project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  due TEXT, summary TEXT, next_action TEXT,
  UNIQUE (account_id, provider_thread_id));
CREATE INDEX IF NOT EXISTS threads_status ON threads(status, last_date DESC);
CREATE INDEX IF NOT EXISTS threads_project ON threads(project_id, last_date DESC);
CREATE INDEX IF NOT EXISTS threads_account ON threads(account_id, last_date DESC);
CREATE INDEX IF NOT EXISTS threads_date ON threads(last_date DESC);
-- Covering indexes so sidebar counts only touch open threads' index entries.
CREATE INDEX IF NOT EXISTS threads_status_account ON threads(status, account_id);
CREATE INDEX IF NOT EXISTS threads_status_project ON threads(status, project_id);
CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY, account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  provider_id TEXT NOT NULL, thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  from_name TEXT NOT NULL, from_email TEXT NOT NULL, to_json TEXT NOT NULL, cc_json TEXT NOT NULL,
  date INTEGER NOT NULL, subject TEXT NOT NULL, body_text TEXT NOT NULL, body_html TEXT,
  labels_json TEXT NOT NULL, is_from_me INTEGER NOT NULL, unread INTEGER NOT NULL,
  message_id_header TEXT, references_json TEXT NOT NULL DEFAULT '[]',
  -- FTS rowid encodes the date, so FTS5 can return newest matches without sorting.
  fts_rowid INTEGER UNIQUE,
  UNIQUE (account_id, provider_id));
CREATE INDEX IF NOT EXISTS messages_thread ON messages(thread_id, date);
CREATE TABLE IF NOT EXISTS attachments (
  id TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  filename TEXT NOT NULL, mime TEXT NOT NULL, size INTEGER NOT NULL, path TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS attachments_message ON attachments(message_id);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  subject, sender, recipients, body, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3 4');
CREATE TABLE IF NOT EXISTS drafts (
  id TEXT PRIMARY KEY, account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  thread_id TEXT REFERENCES threads(id) ON DELETE SET NULL,
  to_json TEXT NOT NULL, cc_json TEXT NOT NULL, subject TEXT NOT NULL, body TEXT NOT NULL,
  state TEXT NOT NULL, origin TEXT NOT NULL, created INTEGER NOT NULL, updated INTEGER NOT NULL,
  prev_status TEXT);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS feedback (id TEXT PRIMARY KEY, text TEXT NOT NULL, command TEXT, agent TEXT, created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS ops (
  id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL, key TEXT NOT NULL, payload TEXT NOT NULL, seq INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT, created INTEGER NOT NULL, UNIQUE (kind, key));
"#;

/// (from_name, from_email, is_from_me, unread, date, subject, body_text, attachment count)
type MsgAgg = (String, String, bool, bool, i64, String, String, i64);

/// Ordered schema migrations; `PRAGMA user_version` records how many ran.
/// Append new steps, never edit shipped ones.
const MIGRATIONS: &[&str] = &[
    SCHEMA,
    THREAD_LABELS,
    SYNC_FAILURES,
    AGENT,
    PROVIDER_AUTH,
    PERSON_OVERRIDES,
    WORKFLOW_V2,
    FIRST_DATE,
    SENDER_AUTH,
    BCC,
    SUGGESTION_STATUS,
    CHAT,
    RECHECK_SENDER_FAILS,
    conversations::MIGRATION,
    DRAFT_ATTACHMENTS,
    SORT_FEEDBACK,
    CLEAR_FLOOD,
    FEED,
    INLINE_PARTS,
    counts::MIGRATION,
    bodies::MIGRATION,
    feed::LATER_MIGRATION,
    addresses::MIGRATION,
    readings::MIGRATION,
    plan::MIGRATION,
    images::MIGRATION,
    push::MIGRATION,
    REPLY_TO,
    MACHINE_MAIL,
    duplicates::MIGRATION,
    plan::SNOOZE_MIGRATION,
    visibility::MIGRATION,
    gmail_drafts::MIGRATION,
];

/// Bounces and auto-replies (`auto_kind`: "bounce", "auto_reply", "" for a person, null
/// until known), what a bounce reports, and the message a draft answers.
const MACHINE_MAIL: &str = "ALTER TABLE messages ADD COLUMN auto_kind TEXT;
ALTER TABLE messages ADD COLUMN dsn_json TEXT;
ALTER TABLE drafts ADD COLUMN reply_to_message_id TEXT;";

/// `Reply-To` per message; null until known (older mail learns it when its thread opens).
const REPLY_TO: &str = "ALTER TABLE messages ADD COLUMN reply_to_json TEXT;";

/// Which parts are images placed in the body (`cid:` links), so they render there and
/// not as attachment chips. Older mail learns its Content-IDs when first shown.
const INLINE_PARTS: &str = "ALTER TABLE attachments ADD COLUMN content_id TEXT;
ALTER TABLE attachments ADD COLUMN inline INTEGER NOT NULL DEFAULT 0;";

/// Files and links going out with a draft; files are copies under `blobs/drafts/`.
const DRAFT_ATTACHMENTS: &str = r#"
CREATE TABLE IF NOT EXISTS draft_attachments (
  id TEXT PRIMARY KEY, draft_id TEXT NOT NULL REFERENCES drafts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL, filename TEXT NOT NULL, mime TEXT NOT NULL, size INTEGER NOT NULL,
  path TEXT, url TEXT, inline INTEGER NOT NULL DEFAULT 0, ord INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS draft_attachments_draft ON draft_attachments(draft_id, ord);
"#;

/// Failures stored under an older, stricter rule are checked again when their
/// thread is opened, so a forwarded copy stops reading as an impersonation.
const RECHECK_SENDER_FAILS: &str = "UPDATE messages SET auth = NULL WHERE auth = 'fail';";

/// The user's yes/no answers to suggestions, per sender: what suggestions learn from.
const SORT_FEEDBACK: &str = r#"
CREATE TABLE IF NOT EXISTS sort_feedback (
  id INTEGER PRIMARY KEY AUTOINCREMENT, thread_id TEXT NOT NULL, sender TEXT NOT NULL,
  status TEXT, projects_json TEXT NOT NULL DEFAULT '[]', verdict TEXT NOT NULL, created INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS sort_feedback_sender ON sort_feedback(sender, verdict);
CREATE INDEX IF NOT EXISTS sort_feedback_thread ON sort_feedback(thread_id);
"#;

/// Machine verdicts on read threads that nobody reviewed: the flood of suggestions from
/// whole-column passes, and "no change" verdicts (some recorded for failed calls).
/// Accepted and rejected suggestions, and Unread's, stay.
const MACHINE_VERDICTS: &str =
    "DELETE FROM suggestions WHERE (decision IS NULL OR decision IN ('unchanged', 'moved'))
  AND thread_id IN (SELECT id FROM threads WHERE status != 'unread')";

/// Clears the whole-column passes once: suggestions are asked again, a few cards at a time.
const CLEAR_FLOOD: &str =
    "DELETE FROM suggestions WHERE (decision IS NULL OR decision IN ('unchanged', 'moved'))
  AND thread_id IN (SELECT id FROM threads WHERE status != 'unread');
DELETE FROM settings WHERE key = 'resort';";

/// The status a suggestion was made for: once the thread sits elsewhere, the
/// suggestion is stale, hidden, and the thread is classified again.
const SUGGESTION_STATUS: &str = "ALTER TABLE suggestions ADD COLUMN for_status TEXT;";

/// Inserts or replaces a thread's suggestion; ?11 is the decision (null = pending).
/// `for_status` is the thread's status at save time.
const UPSERT_SUGGESTION: &str = "INSERT INTO suggestions (thread_id, status, status_confidence, projects_json, project_confidence, priority, automated, model, for_date, created, decision, decided, for_status)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, CASE WHEN ?11 IS NULL THEN NULL ELSE ?10 END, (SELECT status FROM threads WHERE id = ?1))
     ON CONFLICT(thread_id) DO UPDATE SET status = excluded.status, status_confidence = excluded.status_confidence,
       projects_json = excluded.projects_json, project_confidence = excluded.project_confidence, priority = excluded.priority,
       automated = excluded.automated, model = excluded.model, for_date = excluded.for_date, created = excluded.created,
       decision = excluded.decision, decided = excluded.decided, for_status = excluded.for_status";

/// A suggestion still waiting for the user: undecided and made for the thread's
/// current status (alias `s` for suggestions, `t` for threads).
const PENDING_SUGGESTION: &str =
    "s.decision IS NULL AND (s.for_status IS NULL OR s.for_status = t.status)";

/// "Chat about this": each thread's conversation with the AI, kept to show it again.
/// The feed: Jev's per-thread judgments, the AI's card words and snoozes, each made for
/// the thread's latest message (`for_date`) and redone when new mail arrives.
const FEED: &str = r#"
CREATE TABLE IF NOT EXISTS feed_judgments (
  thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
  judged INTEGER NOT NULL, kind TEXT, needs_reply REAL, urgency REAL, due TEXT, due_time TEXT, amount TEXT,
  automated REAL, sender_weight REAL NOT NULL DEFAULT 0, typical_reply_days REAL,
  for_date INTEGER NOT NULL, checked_day TEXT NOT NULL, created INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS feed_judgments_due ON feed_judgments(judged, due);
CREATE TABLE IF NOT EXISTS feed_texts (
  thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
  for_date INTEGER NOT NULL, json TEXT, error TEXT, provider_id TEXT, created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS feed_snoozes (
  thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
  until INTEGER NOT NULL, for_date INTEGER NOT NULL);
"#;

const CHAT: &str = r#"
CREATE TABLE IF NOT EXISTS chat_messages (
  id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  role TEXT NOT NULL, text TEXT NOT NULL, actions_json TEXT NOT NULL DEFAULT '[]',
  searched INTEGER NOT NULL DEFAULT 0, research_unavailable INTEGER NOT NULL DEFAULT 0,
  created INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS chat_messages_thread ON chat_messages(thread_id, created);
"#;

/// Blind copies of messages the user sent (their own copy carries the Bcc header).
const BCC: &str = "ALTER TABLE messages ADD COLUMN bcc_json TEXT NOT NULL DEFAULT '[]';";

/// Sender verification verdict per message (null until checked; "self" for mine).
const SENDER_AUTH: &str = "ALTER TABLE messages ADD COLUMN auth TEXT;";

/// When a conversation started, for the "Created" ordering.
const FIRST_DATE: &str = r#"
ALTER TABLE threads ADD COLUMN first_date INTEGER NOT NULL DEFAULT 0;
UPDATE threads SET first_date = COALESCE((SELECT MIN(m.date) FROM messages m WHERE m.thread_id = threads.id), 0);
CREATE INDEX IF NOT EXISTS threads_first_date ON threads(first_date DESC);
"#;

/// Workflow v2: Unread is the entry status, Done/Canceled collapse into Archive (the old
/// Done carried no Gmail label, so it cannot be told apart from archived mail), Gmail
/// labels become the projects, and the router's suggestions get a home.
const WORKFLOW_V2: &str = r#"
DELETE FROM statuses;
UPDATE threads SET status = 'archive' WHERE status IN ('done', 'canceled');
UPDATE threads SET status = CASE WHEN unread = 1 THEN 'unread' ELSE 'todo' END WHERE status = 'triage';
ALTER TABLE threads ADD COLUMN prev_status TEXT;
CREATE TABLE IF NOT EXISTS thread_projects (
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE, name TEXT NOT NULL,
  PRIMARY KEY (thread_id, name));
CREATE INDEX IF NOT EXISTS thread_projects_name ON thread_projects(name, thread_id);
INSERT OR IGNORE INTO thread_projects (thread_id, name)
  SELECT tl.thread_id, l.name FROM thread_labels tl JOIN labels l ON l.account_id = tl.account_id AND l.label_id = tl.label_id
  WHERE l.label_id NOT IN ('INBOX','UNREAD','SENT','DRAFT','SPAM','TRASH','STARRED','IMPORTANT','CHAT')
    AND l.label_id NOT LIKE 'CATEGORY\_%' ESCAPE '\' AND l.label_id NOT LIKE '%\_STAR' ESCAPE '\'
    AND l.name NOT LIKE 'Mermoz/%' AND l.name != 'Mermoz';
CREATE TABLE IF NOT EXISTS project_colors (name TEXT PRIMARY KEY, color TEXT NOT NULL);
INSERT OR IGNORE INTO project_colors (name, color) SELECT name, color FROM projects;
INSERT OR IGNORE INTO thread_projects (thread_id, name) SELECT t.id, p.name FROM threads t JOIN projects p ON p.id = t.project_id;
INSERT INTO ops (account_id, kind, key, payload, created)
  SELECT t.account_id, 'projects', t.id, json_object('add', json_array(p.name), 'remove', json_array()), CAST(strftime('%s','now') AS INTEGER) * 1000
  FROM threads t JOIN projects p ON p.id = t.project_id WHERE true
  ON CONFLICT(kind, key) DO NOTHING;
INSERT INTO ops (account_id, kind, key, payload, created)
  SELECT account_id, 'labels', id, '{}', CAST(strftime('%s','now') AS INTEGER) * 1000 FROM threads WHERE project_id IS NOT NULL AND true
  ON CONFLICT(kind, key) DO NOTHING;
UPDATE threads SET project_id = NULL;
CREATE TABLE IF NOT EXISTS suggestions (
  thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
  status TEXT, status_confidence REAL, projects_json TEXT NOT NULL DEFAULT '[]', project_confidence REAL,
  priority INTEGER, automated REAL, model TEXT NOT NULL, for_date INTEGER NOT NULL, created INTEGER NOT NULL,
  decision TEXT, decided INTEGER);
"#;

/// The AI's verdict on who is who: each address mapped to one person with a clean name.
const PERSON_OVERRIDES: &str = r#"
CREATE TABLE IF NOT EXISTS person_overrides (
  email TEXT PRIMARY KEY, person_id TEXT NOT NULL, name TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'person', updated INTEGER NOT NULL);
"#;

/// Providers signed in through Anthropic's OAuth login instead of a pasted key.
const PROVIDER_AUTH: &str = "ALTER TABLE providers ADD COLUMN auth TEXT NOT NULL DEFAULT 'key'; ALTER TABLE providers ADD COLUMN profile TEXT;";

/// Built-in agent: providers, per-account routing, cleaned messages and the contact directory.
const AGENT: &str = r#"
CREATE TABLE IF NOT EXISTS providers (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL, base_url TEXT, model TEXT NOT NULL, created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS account_providers (
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  provider_id TEXT NOT NULL, PRIMARY KEY (account_id, provider_id));
CREATE TABLE IF NOT EXISTS digests (
  message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  status TEXT NOT NULL, clean_body TEXT, card_json TEXT, provider_id TEXT, error TEXT, created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS contacts (
  email TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '', title TEXT NOT NULL DEFAULT '', company TEXT NOT NULL DEFAULT '',
  phones_json TEXT NOT NULL DEFAULT '[]', address TEXT NOT NULL DEFAULT '', website TEXT NOT NULL DEFAULT '',
  socials_json TEXT NOT NULL DEFAULT '[]', last_seen INTEGER NOT NULL DEFAULT 0, updated INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS messages_from_lower ON messages(lower(from_email));
"#;

/// Messages that could not be imported, kept visible instead of silently dropped.
const SYNC_FAILURES: &str = r#"
CREATE TABLE IF NOT EXISTS sync_failures (
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  provider_id TEXT NOT NULL, error TEXT NOT NULL, at INTEGER NOT NULL,
  PRIMARY KEY (account_id, provider_id));
"#;

/// Per-thread label index so label views ("mailboxes") are an index lookup, not a JSON scan.
const THREAD_LABELS: &str = r#"
CREATE TABLE IF NOT EXISTS thread_labels (
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  account_id TEXT NOT NULL, label_id TEXT NOT NULL, PRIMARY KEY (thread_id, label_id));
CREATE INDEX IF NOT EXISTS thread_labels_label ON thread_labels(account_id, label_id);
INSERT OR IGNORE INTO thread_labels (thread_id, account_id, label_id)
  SELECT DISTINCT m.thread_id, m.account_id, j.value FROM messages m, json_each(m.labels_json) j;
"#;

/// Gmail system labels that are views, not mailboxes the user files mail into.
/// Stars (`YELLOW_STAR`…) and `CATEGORY_*` tabs are excluded by [`is_user_label`] too.
const SYSTEM_LABELS: &[&str] = &[
    "INBOX",
    "UNREAD",
    "SENT",
    "DRAFT",
    "SPAM",
    "TRASH",
    "STARRED",
    "IMPORTANT",
    "CHAT",
];

fn migrate(conn: &Connection) -> Result<()> {
    let done: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, step) in MIGRATIONS.iter().enumerate().skip(done as usize) {
        conn.execute_batch(&format!(
            "BEGIN; {step}; PRAGMA user_version = {}; COMMIT;",
            i + 1
        ))?;
    }
    Ok(())
}

/// Prefix of Mermoz's own status/priority labels in Gmail.
pub const LABEL_PREFIX: &str = "Mermoz/";

/// Whether a Gmail label is one the user files mail into, i.e. a project.
pub fn is_user_label(id: &str, name: &str) -> bool {
    !SYSTEM_LABELS.contains(&id)
        && !id.starts_with("CATEGORY_")
        && !id.ends_with("_STAR")
        && !name.starts_with(LABEL_PREFIX)
        && name != "Mermoz"
}

/// Gmail rejects empty names and these reserved ones; "/" nests projects.
fn validate_project_name(name: &str) -> Result<()> {
    let n = name.trim();
    let reserved = SYSTEM_LABELS.iter().any(|s| s.eq_ignore_ascii_case(n));
    if n.is_empty()
        || n.starts_with('/')
        || n.ends_with('/')
        || n.contains("//")
        || reserved
        || n.starts_with(LABEL_PREFIX)
        || n == "Mermoz"
    {
        return Err(Error::InvalidParams(format!(
            "'{name}' cannot be used as a project name"
        )));
    }
    Ok(())
}

/// Stable color for a project without a chosen one.
pub fn default_color(name: &str) -> &'static str {
    const COLORS: &[&str] = &[
        "#5B8DEF", "#E0782F", "#2E9A6B", "#8A6FD1", "#C2568C", "#2F8FCF",
    ];
    let h = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    COLORS[h as usize % COLORS.len()]
}

/// Open statuses a thread returns to after an interruption by new mail.
const RESUMABLE: &[&str] = &["todo", "waiting", "in_progress", "backlog"];

/// How a message arrives: the initial import must not reopen old threads, live mail must.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    Initial,
    Incremental,
}

#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub provider_id: String,
    pub provider_thread_id: String,
    pub label_ids: Vec<String>,
    pub internal_date: i64,
    pub parsed: ParsedMessage,
}

/// Rows of `threads t LEFT JOIN suggestions s` that may still show a suggestion:
/// never judged, new mail since, or a suggestion still pending.
const SORT_DUE: &str = "(s.thread_id IS NULL OR s.for_date < t.last_date OR s.decision IS NULL)";

/// `?,?` for the open columns that get suggestions on read threads.
fn open_columns() -> (String, Vec<rusqlite::types::Value>) {
    (
        vec!["?"; RESUMABLE.len()].join(","),
        RESUMABLE.iter().map(|s| s.to_string().into()).collect(),
    )
}

fn thread_sender_tx(c: &Connection, thread_id: &str) -> Result<String> {
    let from: Option<String> = c
        .query_row(
            "SELECT lower(from_email) FROM messages WHERE thread_id = ?1 AND is_from_me = 0 ORDER BY date DESC LIMIT 1",
            [thread_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(f) = from {
        return Ok(f);
    }
    Ok(c.query_row(
        "SELECT lower(json_extract(m.to_json, '$[0].email')) FROM messages m WHERE m.thread_id = ?1 ORDER BY m.date DESC LIMIT 1",
        [thread_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()?
    .flatten()
    .unwrap_or_default())
}

fn refused_tx(c: &Connection, sender: &str, target: &SuggestionGroup) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS (SELECT 1 FROM sort_feedback WHERE sender = ?1 AND verdict = 'no'
           AND status IS ?2 AND projects_json = ?3)",
        params![
            sender,
            target.status,
            serde_json::to_string(&target.projects)?
        ],
        |r| r.get(0),
    )?)
}

fn record_feedback_tx(
    c: &Connection,
    thread_id: &str,
    sender: &str,
    target: &SuggestionGroup,
    verdict: &str,
) -> Result<()> {
    if sender.is_empty() || (target.status.is_none() && target.projects.is_empty()) {
        return Ok(());
    }
    c.execute(
        "INSERT INTO sort_feedback (thread_id, sender, status, projects_json, verdict, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![thread_id, sender, target.status, serde_json::to_string(&target.projects)?, verdict, now_ms()],
    )?;
    Ok(())
}

/// Settings key counting today's background router calls.
fn background_calls_key() -> String {
    format!("jev_calls:{}", chrono::Utc::now().format("%Y-%m-%d"))
}

/// Every System One call to one provider today (UTC), foreground and background.
fn provider_calls_key(p: crate::jev::Provider) -> String {
    format!(
        "jev_provider_calls:{}:{}",
        p.id(),
        chrono::Utc::now().format("%Y-%m-%d")
    )
}

const PROVIDER_SETTING: &str = "jev_provider";

/// Restricts a column to the board or list on screen: one project and/or one account.
#[derive(Debug, Clone, Default)]
pub struct ColumnScope {
    pub project: Option<String>,
    pub account_id: Option<String>,
}

impl ColumnScope {
    /// Extra `AND …` conditions on `t`, and their arguments.
    fn clause(&self) -> (String, Vec<rusqlite::types::Value>) {
        let mut sql = String::new();
        let mut args: Vec<rusqlite::types::Value> = vec![];
        if let Some(p) = &self.project {
            sql.push_str(
                " AND t.id IN (SELECT tp.thread_id FROM thread_projects tp WHERE tp.name = ?)",
            );
            args.push(p.clone().into());
        }
        if let Some(a) = &self.account_id {
            sql.push_str(" AND t.account_id = ?");
            args.push(a.clone().into());
        }
        (sql, args)
    }
}

/// Threads a column's suggestions would send to the same place.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SuggestionGroup {
    pub status: Option<String>,
    pub projects: Vec<String>,
    #[serde(default)]
    pub count: i64,
}

impl SuggestionGroup {
    /// Where a suggestion would send its thread.
    pub fn of(sg: &Suggestion) -> Self {
        Self {
            status: sg.status.clone(),
            projects: sg.projects.clone(),
            count: 0,
        }
    }
}

/// A column's pending suggestions, grouped by destination.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ColumnSuggestions {
    pub status: String,
    pub pending: i64,
    pub groups: Vec<SuggestionGroup>,
}

/// A thread that may get a suggestion; `ready` when it is already computed.
#[derive(Debug, Clone, PartialEq)]
pub struct SortCandidate {
    pub id: String,
    pub last_date: i64,
    pub ready: bool,
}

/// What the user answered to a card.
#[derive(Debug, Clone, PartialEq)]
pub enum SortAnswer {
    Yes,
    No,
    /// Another destination: a No for the suggestion and a Yes for this one.
    Change {
        status: Option<String>,
        projects: Vec<String>,
    },
}

/// Puts a thread and its card back as before an answer.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SortUndo {
    pub thread: ThreadState,
    /// Answers recorded from this time on are forgotten.
    pub since: i64,
    /// The other open threads of its conversation, which followed the answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub siblings: Vec<ThreadState>,
}

/// Something suggestions learned from the user's answers.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Learned {
    pub sender: String,
    pub name: String,
    pub status: Option<String>,
    pub projects: Vec<String>,
    /// "rule": suggested from now on without asking the AI; "avoid": never suggested again.
    pub kind: String,
    pub count: i64,
}

/// (sender, status, projects json, verdict, count, sender's display name)
type FeedbackRow = (String, Option<String>, String, String, i64, Option<String>);

/// Consistent yes answers after which a sender's destination is suggested without the AI.
pub const TRUST_AFTER: i64 = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum SuggestedFilter {
    /// Any pending suggestion.
    Any,
    /// Only those sending threads to this destination.
    To(SuggestionGroup),
}

#[derive(Debug, Clone, Default)]
pub struct ThreadFilter {
    /// Only threads with a pending suggestion (optionally to one destination).
    pub suggested: Option<SuggestedFilter>,
    pub status: Option<Vec<String>>,
    /// Project (label) name.
    pub project: Option<String>,
    pub account_id: Option<String>,
    pub priority: Option<Vec<i64>>,
    pub label_id: Option<String>,
    /// Threads with at least one message from any of these addresses.
    pub participants: Option<Vec<String>>,
    /// "last_date" (default), "priority", "due" or "created".
    pub order: Option<String>,
    /// Oldest / least urgent / latest first instead.
    pub reverse: bool,
    pub limit: i64,
    pub offset: i64,
    /// One row per conversation: only the thread leading each one.
    pub conversations: bool,
    /// Only these streams: "person", "feed", "list".
    pub kinds: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct ThreadPatch {
    pub status: Option<String>,
    pub priority: Option<i64>,
    /// Replaces the thread's projects.
    pub projects: Option<Vec<String>>,
    pub add_projects: Vec<String>,
    pub remove_projects: Vec<String>,
    pub due: Option<Option<String>>,
}

impl ThreadPatch {
    fn touches_projects(&self) -> bool {
        self.projects.is_some() || !self.add_projects.is_empty() || !self.remove_projects.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Op {
    pub id: i64,
    pub account_id: String,
    pub kind: String,
    pub key: String,
    pub payload: serde_json::Value,
    pub attempts: i64,
    /// Bumped on every re-enqueue, so an in-flight push can tell it was superseded.
    pub seq: i64,
    /// Queued by a bulk run: it waits behind the user's own edits.
    pub bulk: bool,
}

/// Status a thread gets when first seen, from its Gmail state.
pub fn initial_status(
    in_inbox: bool,
    trashed: bool,
    unread: bool,
    status_label: Option<&str>,
    last_date: i64,
    now: i64,
    mode: SyncMode,
) -> String {
    if trashed {
        return "deleted".into();
    }
    if let Some(s) = status_label {
        return s.to_string();
    }
    if !in_inbox {
        return "archive".into();
    }
    // Old inbox mail imported on first sync goes to Backlog so Unread starts manageable.
    const THIRTY_DAYS: i64 = 30 * 24 * 3600 * 1000;
    if mode == SyncMode::Initial && now - last_date > THIRTY_DAYS {
        "backlog".into()
    } else if unread {
        "unread".into()
    } else {
        "todo".into()
    }
}

/// Someone else writing into the inbox puts the thread back in Unread, unless a draft
/// awaits review there.
pub fn status_after_new_message(
    current: &str,
    from_me: bool,
    in_inbox: bool,
) -> Option<&'static str> {
    if !from_me && in_inbox && current != "unread" && current != "review" {
        Some("unread")
    } else {
        None
    }
}

/// Where a thread goes once read: back to the open status new mail interrupted (a
/// Waiting thread has its answer, so To do), else the router's suggestion, else the
/// sender's habit, else To do.
pub fn status_after_read(
    prev: Option<&str>,
    suggested: Option<&str>,
    habit: Option<&str>,
) -> String {
    match prev {
        Some("waiting") => "todo".into(),
        Some(p) if RESUMABLE.contains(&p) => p.into(),
        _ => suggested.or(habit).unwrap_or("todo").into(),
    }
}

/// Gmail labels (by name) a thread should carry for its status and priority; INBOX
/// included. Deleted threads carry nothing: they are moved to Trash instead.
pub fn desired_labels(
    status: &str,
    archived: bool,
    status_name: &str,
    priority: i64,
) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    if status == "deleted" {
        return s;
    }
    if !archived {
        s.insert("INBOX".to_string());
    }
    if !matches!(status, "unread" | "archive") {
        s.insert(format!("{LABEL_PREFIX}{status_name}"));
    }
    if let Some(p) = priority_name(priority).filter(|_| priority != 3) {
        s.insert(format!("{LABEL_PREFIX}Priority/{p}"));
    }
    s
}

/// Date in the high bits, a slice of the message rowid in the low 20 bits for uniqueness.
pub fn fts_rowid(date_ms: i64, rowid: i64) -> i64 {
    (date_ms.max(0) << 20) | (rowid & 0xF_FFFF)
}

/// Filesystem-safe attachment name: no separators or control characters, and at most
/// 120 bytes (APFS caps names at 255) with the extension kept so files still open.
pub fn safe_filename(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    if clean.is_empty() {
        return "attachment".into();
    }
    const MAX: usize = 120;
    if clean.len() <= MAX {
        return clean;
    }
    let (stem, ext) = match clean.rfind('.') {
        Some(i) if clean.len() - i <= 16 => (&clean[..i], &clean[i..]),
        _ => (clean.as_str(), ""),
    };
    let mut end = MAX - ext.len();
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &stem[..end.min(stem.len())], ext)
}

/// ORDER BY for a thread list; unknown values fall back to most recent activity.
/// SQL ORDER BY for an ordering, newest / most urgent / soonest first unless `reverse`.
/// Threads without a priority or follow-up date stay last either way, and the thread
/// number breaks ties so equal rows never swap between refreshes.
pub fn order_clause(order: Option<&str>, reverse: bool) -> String {
    let (desc, asc) = if reverse {
        ("ASC", "DESC")
    } else {
        ("DESC", "ASC")
    };
    match order {
        Some("priority") => {
            format!("t.priority = 0, t.priority {asc}, t.last_date DESC, t.number DESC")
        }
        Some("due") => format!("t.due IS NULL, t.due {asc}, t.last_date DESC, t.number DESC"),
        Some("created") => format!("t.first_date {desc}, t.number {desc}"),
        _ => format!("t.last_date {desc}, t.number {desc}"),
    }
}

/// Lowercase, accents folded, "Last, First" turned around: the key that spots one person
/// writing from several addresses.
pub fn person_key(name: &str) -> String {
    let n = match name.split_once(',') {
        Some((last, first)) if !first.trim().is_empty() => {
            format!("{} {}", first.trim(), last.trim())
        }
        _ => name.to_string(),
    };
    let folded: String = n
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => {
                'a'
            }
            'ç' | 'Ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
            'ñ' | 'Ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
            '-' | '_' | '.' | '\'' | '"' => ' ',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Address-book order: family name, then the rest. "(Airbnb)"-style suffixes are ignored,
/// a leading ALL-CAPS word is the family name ("NAGEOTTE Pierre"), and entries that are
/// just an address sort last.
pub fn sort_key(name: &str) -> (bool, bool, String, String) {
    let mut clean = String::new();
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            c if depth == 0 => clean.push(c),
            _ => {}
        }
    }
    let raw: Vec<&str> = clean.split_whitespace().collect();
    let caps_first = raw.len() > 1
        && raw[0].chars().filter(|c| c.is_alphabetic()).count() > 1
        && raw[0].chars().all(|c| !c.is_lowercase())
        && raw[1..].iter().any(|w| w.chars().any(|c| c.is_lowercase()));
    let key = person_key(&clean);
    let words: Vec<&str> = key.split(' ').collect();
    // Initials and suffixes like "S.A." or "Jr" are not family names: skip 1-letter words.
    let family = if caps_first {
        words.first().copied()
    } else {
        words
            .iter()
            .rev()
            .find(|w| w.chars().count() > 1)
            .or(words.last())
            .copied()
    }
    .unwrap_or("")
    .to_string();
    // Entries not starting with a letter ("09filali", "ICASP 12") follow the alphabet.
    let not_alpha = !family.chars().next().is_some_and(|c| c.is_alphabetic());
    (name.contains('@'), not_alpha, family, key)
}

/// Automated senders never belong in a people directory.
pub fn is_automated(email: &str) -> bool {
    let local = email.split('@').next().unwrap_or("").to_lowercase();
    [
        "noreply",
        "no-reply",
        "no_reply",
        "donotreply",
        "do-not-reply",
        "notification",
        "mailer-daemon",
        "bounce",
        "postmaster",
        "newsletter",
        "news",
        "alerts",
        "digest",
        "updates",
    ]
    .iter()
    .any(|p| local.contains(p))
}

/// "Last, First" display names read as "First Last".
fn display_person(name: &str) -> String {
    match name.split_once(',') {
        Some((last, first)) if !first.trim().is_empty() && !last.contains(' ') => {
            format!("{} {}", first.trim(), last.trim())
        }
        _ => name.trim().to_string(),
    }
}

/// A search as FTS5 queries: every term must match (`all`); mail with the words in a
/// row (`phrase`) ranks first, then mail with them all in its subject or sender
/// (`headline`). Both are None while a long word is typed (see [`fts_query`]).
#[derive(Debug, Clone, PartialEq)]
pub struct FtsQuery {
    pub all: String,
    pub phrase: Option<String>,
    pub headline: Option<String>,
}

/// Turns free text into safe FTS5 queries: every word must match, as a whole word,
/// accents and case aside; `from:x` / `to:x` / `subject:x` restrict to a column. While
/// `typing`, the word being typed (the last, with no space after it) also matches as a
/// prefix once it has 3 letters: a finished word matching longer ones ("bee" in "been")
/// let unrelated mail in.
pub fn fts_query(q: &str, typing: bool) -> Option<FtsQuery> {
    let mut terms: Vec<(Option<&str>, String)> = vec![];
    for raw in q.split_whitespace() {
        let (col, word) = match raw.split_once(':') {
            Some(("from", w)) => (Some("sender"), w),
            Some(("to", w)) => (Some("recipients"), w),
            Some(("subject", w)) => (Some("subject"), w),
            _ => (None, raw),
        };
        let clean: String = word
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '@' || *c == '.' || *c == '-' || *c == '_')
            .collect();
        if !clean.is_empty() {
            terms.push((col, clean));
        }
    }
    let last = terms.len().checked_sub(1)?;
    let open = typing && !q.ends_with(char::is_whitespace);
    let prefix = |i: usize, w: &str| open && i == last && w.chars().count() >= 3;
    let all = terms
        .iter()
        .enumerate()
        .map(|(i, (col, w))| {
            let star = if prefix(i, w) { "*" } else { "" };
            match col {
                Some(c) => format!("{c}:\"{w}\"{star}"),
                None => format!("\"{w}\"{star}"),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let words: Vec<(usize, &str)> = terms
        .iter()
        .enumerate()
        .filter(|(_, (col, _))| col.is_none())
        .map(|(i, (_, w))| (i, w.as_str()))
        .collect();
    // A typed word of 5+ letters has no prefix index: FTS5 merges every word it starts,
    // ~20 ms at 500k, so ranking (more such merges) waits for the word to finish.
    let ranked = !(prefix(last, &terms[last].1) && terms[last].1.chars().count() >= 5);
    let phrase = match words.as_slice() {
        [_, .., (i, w)] if ranked => {
            let star = if prefix(*i, w) { "*" } else { "" };
            let joined = words.iter().map(|(_, w)| *w).collect::<Vec<_>>().join(" ");
            Some(format!("\"{joined}\"{star}"))
        }
        _ => None,
    };
    let headline = ranked.then(|| format!("{{subject sender}} : ({all})"));
    Some(FtsQuery {
        all,
        phrase,
        headline,
    })
}

fn fmt_addr(a: &Address) -> String {
    if a.name.is_empty() {
        a.email.clone()
    } else {
        format!("{} {}", a.name, a.email)
    }
}

fn display_name(name: &str, email: &str) -> String {
    if !name.is_empty() {
        return name.to_string();
    }
    email.split('@').next().unwrap_or(email).to_string()
}

pub struct Store {
    w: Mutex<Connection>,
    r: Mutex<Connection>,
    /// Reads for the background router, so its history scans never queue the UI's reads.
    bg: Mutex<Connection>,
    /// Checkpoints, with the database version they last saw and the database file they
    /// sync.
    ckpt: Mutex<(Connection, i64, std::fs::File)>,
    blobs: PathBuf,
    counts_cache: Mutex<counts::CountsCache>,
    /// Whether any account is hidden, so queries skip the visibility filter otherwise.
    any_hidden: std::sync::atomic::AtomicBool,
}

const THREAD_COLS: &str = "t.id, t.number, t.account_id, t.subject, t.snippet, t.participants, t.last_from, t.last_date, \
    t.unread, t.message_count, t.has_attachments, t.status, t.priority, t.due, t.summary, t.next_action, \
    COALESCE(t.conversation_id, t.id), t.conv_kind";

fn thread_row(r: &Row) -> rusqlite::Result<ThreadSummary> {
    let participants: String = r.get(5)?;
    Ok(ThreadSummary {
        id: r.get(0)?,
        number: format!("M-{}", r.get::<_, i64>(1)?),
        account_id: r.get(2)?,
        subject: r.get(3)?,
        snippet: r.get(4)?,
        participants: serde_json::from_str(&participants).unwrap_or_default(),
        last_from: r.get(6)?,
        last_date: r.get(7)?,
        unread: r.get::<_, i64>(8)? != 0,
        message_count: r.get(9)?,
        has_attachments: r.get::<_, i64>(10)? != 0,
        status: r.get(11)?,
        priority: r.get(12)?,
        projects: vec![],
        due: r.get(13)?,
        summary: r.get(14)?,
        next_action: r.get(15)?,
        suggestion: None,
        conversation_id: r.get(16)?,
        conversation_kind: r.get(17)?,
        conversation_threads: 1,
    })
}

/// Fills each thread's projects and pending suggestion in two indexed lookups.
fn attach(conn: &Connection, rows: &mut [ThreadSummary]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let ids: Vec<rusqlite::types::Value> = rows.iter().map(|t| t.id.clone().into()).collect();
    let marks = vec!["?"; ids.len()].join(",");
    let mut projects: std::collections::HashMap<String, Vec<String>> = Default::default();
    let mut st = conn.prepare(&format!(
        "SELECT thread_id, name FROM thread_projects WHERE thread_id IN ({marks}) ORDER BY name COLLATE NOCASE"
    ))?;
    for row in st.query_map(params_from_iter(ids.iter()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (t, n) = row?;
        projects.entry(t).or_default().push(n);
    }
    let mut suggestions: std::collections::HashMap<String, Suggestion> = Default::default();
    let mut st = conn.prepare(&format!(
        "SELECT s.thread_id, s.status, s.status_confidence, s.projects_json, s.project_confidence, s.priority, s.automated
         FROM suggestions s JOIN threads t ON t.id = s.thread_id WHERE {PENDING_SUGGESTION} AND s.thread_id IN ({marks})"
    ))?;
    for row in st.query_map(params_from_iter(ids.iter()), |r| {
        let pj: String = r.get(3)?;
        Ok((
            r.get::<_, String>(0)?,
            Suggestion {
                status: r.get(1)?,
                status_confidence: r.get(2)?,
                projects: serde_json::from_str(&pj).unwrap_or_default(),
                project_confidence: r.get(4)?,
                priority: r.get(5)?,
                automated: r.get(6)?,
            },
        ))
    })? {
        let (t, sg) = row?;
        suggestions.insert(t, sg);
    }
    let sizes = conversations::sizes(conn, rows)?;
    for t in rows.iter_mut() {
        t.projects = projects.remove(&t.id).unwrap_or_default();
        t.suggestion = suggestions.remove(&t.id);
        t.conversation_threads = sizes.get(&t.conversation_id).copied().unwrap_or(1);
    }
    Ok(())
}

/// The messages matching `filter` (one `?1` parameter), oldest first, in one pass, with
/// their stored reading (`body_new` and its flags). A message not read yet is read now;
/// with `latest`, only the last that many are, the earlier ones keeping `body_new`
/// empty (read for the conversation's people, files and links only).
fn messages_where(
    conn: &Connection,
    filter: &str,
    arg: &str,
    latest: Option<usize>,
) -> Result<Vec<Message>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT m.id, m.thread_id, from_name, from_email, to_json, cc_json, date, subject, body_text,
           {}, is_from_me,
           (SELECT d.clean_body FROM digests d WHERE d.message_id = m.id AND d.status = 'ok'), auth, bcc_json,
           m.list_id IS NOT NULL, r.text, r.flags, r.forwarded_from, r.reaction, r.reaction_quote,
           m.message_id_header, m.references_json, m.reply_to_json, m.auto_kind, m.dsn_json, m.duplicate_of
         FROM messages m {}{}
         WHERE {filter} ORDER BY date, m.rowid",
        bodies::HAS_HTML_OF_M,
        readings::JOIN_READING_OF_M,
        crate::reading::VERSION,
    ))?;
    // (Message-ID, References) of each message, to find what a reaction is to.
    let mut ids: Vec<(String, Vec<String>)> = vec![];
    // What each bounce reports, to mark the message that failed.
    let mut dsns: Vec<Option<crate::dsn::Dsn>> = vec![];
    // (listed, stored reading, message)
    let mut msgs = st
        .query_map([arg], |r| {
            let to: String = r.get(4)?;
            let cc: String = r.get(5)?;
            let email: String = r.get(3)?;
            let listed = r.get::<_, i64>(14)? != 0;
            let stored = r
                .get::<_, Option<String>>(15)?
                .map(|text| -> rusqlite::Result<_> {
                    let mut reading =
                        crate::reading::Reading::from_parts(text, r.get(16)?, r.get(17)?);
                    reading.reaction = r.get(18)?;
                    reading.reaction_quote = r.get(19)?;
                    Ok(reading)
                })
                .transpose()?;
            let stored_kind = r.get::<_, Option<String>>(23)?;
            let subject: String = r.get(7)?;
            // Mail not classified yet: what its sender and subject say.
            let kind = match stored_kind.as_deref() {
                Some(k) => crate::dsn::AutoKind::from_stored(k),
                None => crate::dsn::classify(&email, &subject, |_| None),
            };
            dsns.push(
                r.get::<_, Option<String>>(24)?
                    .and_then(|j| serde_json::from_str(&j).ok()),
            );
            ids.push((
                r.get::<_, Option<String>>(20)?.unwrap_or_default(),
                serde_json::from_str::<Vec<String>>(&r.get::<_, String>(21)?).unwrap_or_default(),
            ));
            Ok((
                listed,
                stored,
                Message {
                    id: r.get(0)?,
                    thread_id: r.get(1)?,
                    to: serde_json::from_str(&to).unwrap_or_default(),
                    cc: serde_json::from_str(&cc).unwrap_or_default(),
                    bcc: serde_json::from_str(&r.get::<_, String>(13)?).unwrap_or_default(),
                    reply_to: r
                        .get::<_, Option<String>>(22)?
                        .and_then(|j| serde_json::from_str(&j).ok())
                        .unwrap_or_default(),
                    is_bounce: kind == crate::dsn::AutoKind::Bounce,
                    is_auto_reply: kind == crate::dsn::AutoKind::AutoReply,
                    delivery_failure: None,
                    date: r.get(6)?,
                    subject: r.get(7)?,
                    body_new: String::new(),
                    has_quoted: false,
                    has_signature: false,
                    forwarded: false,
                    forwarded_from: None,
                    quoted_only: false,
                    reaction: None,
                    duplicate_of: r.get(25)?,
                    copies: 1,
                    body_clean: r.get(11)?,
                    body_text: r.get(8)?,
                    has_html: r.get::<_, i64>(9)? != 0,
                    is_from_me: r.get::<_, i64>(10)? != 0,
                    attachments: vec![],
                    auth: crate::auth::shown(r.get::<_, Option<String>>(12)?.as_deref(), &email),
                    from: Address {
                        name: r.get(2)?,
                        email,
                    },
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let read_from = latest.map_or(0, |n| msgs.len().saturating_sub(n));
    // Mail stored before its reading was cached (or by an older pipeline) is read now;
    // its HTML is fetched only for those.
    let todo: Vec<usize> = (read_from..msgs.len())
        .filter(|&i| msgs[i].1.is_none())
        .collect();
    let html: Vec<Option<String>> = todo
        .iter()
        .map(|&i| -> Result<Option<String>> {
            if !msgs[i].2.has_html {
                return Ok(None);
            }
            Ok(conn
                .query_row(
                    &format!("SELECT {} FROM messages m WHERE id = ?1", bodies::HTML_OF_M),
                    [&msgs[i].2.id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten())
        })
        .collect::<Result<_>>()?;
    // Pure CPU per message: a long conversation spreads it on cores.
    let jobs: Vec<(usize, &Option<String>)> = todo.iter().copied().zip(&html).collect();
    let fresh = par_map(&jobs, |(i, h)| {
        let (listed, _, m) = &msgs[*i];
        readings::read(&m.subject, &m.body_text, h.as_deref(), *listed)
    });
    for (&i, r) in todo.iter().zip(fresh) {
        msgs[i].1 = Some(r);
    }
    let mut st = conn.prepare_cached(
        "SELECT id, filename, mime, size, inline, decorative FROM attachments WHERE message_id = ?1",
    )?;
    // Quoted openings of reactions, by message, to find their target below.
    let mut reaction_quotes: Vec<Option<String>> = vec![];
    // A stored reading costs nothing to show, even before `latest`.
    for (_, stored, m) in msgs.iter_mut() {
        let mut quote = None;
        if let Some(r) = stored.take() {
            m.body_new = r.text;
            m.has_quoted = r.has_quoted;
            m.has_signature = r.has_signature;
            m.forwarded = r.forwarded;
            m.forwarded_from = r.forwarded_from;
            m.quoted_only = r.quoted_only;
            m.reaction = r.reaction.map(|emoji| crate::model::Reaction {
                emoji,
                target_message_id: None,
            });
            quote = r.reaction_quote;
        }
        reaction_quotes.push(quote);
        m.attachments = st
            .query_map([&m.id], |r| {
                let (filename, mime): (String, String) = (r.get(1)?, r.get(2)?);
                let (size, inline): (i64, bool) = (r.get(3)?, r.get::<_, i64>(4)? != 0);
                // Mail stored before images were classified: what its name, type and
                // size say, until the background pass looks at the image itself.
                let decorative = match r.get::<_, Option<i64>>(5)? {
                    Some(d) => d != 0,
                    None => crate::decorative::is_decorative(
                        &crate::decorative::Image {
                            filename: &filename,
                            mime: &mime,
                            size,
                            inline,
                            ..Default::default()
                        },
                        None,
                        0,
                        0,
                    ),
                };
                Ok(AttachmentInfo {
                    id: r.get(0)?,
                    filename,
                    mime,
                    size,
                    inline,
                    decorative,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
    }
    let msgs: Vec<Message> = msgs.into_iter().map(|(_, _, m)| m).collect();
    // A second delivery of an email is hidden; the kept copy counts it. The headers and
    // quotes read alongside stay in step with the messages.
    let shown: std::collections::HashSet<String> = msgs
        .iter()
        .filter(|m| m.duplicate_of.is_none())
        .map(|m| m.id.clone())
        .collect();
    let mut copies: std::collections::HashMap<String, u32> = Default::default();
    let (mut kept, mut ids_kept, mut quotes_kept) = (vec![], vec![], vec![]);
    for ((m, id), quote) in msgs.into_iter().zip(ids).zip(reaction_quotes) {
        match &m.duplicate_of {
            Some(of) if shown.contains(of) => *copies.entry(of.clone()).or_default() += 1,
            _ => {
                kept.push(m);
                ids_kept.push(id);
                quotes_kept.push(quote);
            }
        }
    }
    for m in kept.iter_mut() {
        m.copies = 1 + copies.get(&m.id).copied().unwrap_or(0);
    }
    let (mut msgs, ids, reaction_quotes) = (kept, ids_kept, quotes_kept);
    images::mark_repeated(&mut msgs);
    reactions::find_targets(&mut msgs, &ids, &reaction_quotes);
    mark_delivery_failures(&mut msgs, &ids, &dsns);
    Ok(msgs)
}

/// Puts what each bounce reports on the sent message it reports on: the one whose
/// Message-ID it names, else the user's latest earlier message to that recipient.
fn mark_delivery_failures(
    msgs: &mut [Message],
    ids: &[(String, Vec<String>)],
    dsns: &[Option<crate::dsn::Dsn>],
) {
    let bare = |s: &str| {
        s.trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_string()
    };
    for (b, dsn) in dsns.iter().enumerate() {
        let Some(dsn) = dsn.as_ref().filter(|d| !d.recipient.is_empty()) else {
            continue;
        };
        let by_id = dsn.original_message_id.as_ref().and_then(|want| {
            ids.iter()
                .position(|(mid, _)| !mid.is_empty() && bare(mid) == bare(want))
        });
        let failed = by_id.or_else(|| {
            (0..b).rev().find(|&i| {
                msgs[i].is_from_me
                    && msgs[i]
                        .to
                        .iter()
                        .chain(&msgs[i].cc)
                        .any(|a| a.email.eq_ignore_ascii_case(&dsn.recipient))
            })
        });
        if let Some(i) = failed.filter(|&i| i != b) {
            msgs[i].delivery_failure = Some(crate::model::DeliveryFailure {
                recipient: dsn.recipient.clone(),
                status: dsn.status.clone(),
                reason: crate::dsn::reason(dsn),
                bounce_message_id: msgs[b].id.clone(),
            });
        }
    }
}

/// What an incoming message's bubble shows.
fn reading_of(m: &IncomingMessage) -> crate::reading::Reading {
    let p = &m.parsed;
    readings::read(
        &p.subject,
        &p.body_text,
        p.body_html.as_deref(),
        p.list_id.is_some(),
    )
}

/// `f` over `items`, in order; on several threads once there are enough to pay for them.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    const MIN_PER_THREAD: usize = 32;
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(8)
        .min(items.len() / MIN_PER_THREAD);
    if threads <= 1 {
        return items.iter().map(&f).collect();
    }
    let chunk = items.len().div_ceil(threads);
    std::thread::scope(|s| {
        let parts: Vec<_> = items
            .chunks(chunk)
            .map(|c| s.spawn(|| c.iter().map(&f).collect::<Vec<R>>()))
            .collect();
        parts
            .into_iter()
            .flat_map(|p| p.join().expect("reading worker"))
            .collect()
    })
}

fn draft_row(r: &Row) -> rusqlite::Result<Draft> {
    let to: String = r.get(3)?;
    let cc: String = r.get(4)?;
    Ok(Draft {
        id: r.get(0)?,
        account_id: r.get(1)?,
        thread_id: r.get(2)?,
        to: serde_json::from_str(&to).unwrap_or_default(),
        cc: serde_json::from_str(&cc).unwrap_or_default(),
        subject: r.get(5)?,
        body: r.get(6)?,
        state: r.get(7)?,
        origin: r.get(8)?,
        created: r.get(9)?,
        updated: r.get(10)?,
        attachments: vec![],
        reply_to_message_id: r.get(11)?,
    })
}

/// A received message whose `Reply-To` and kind are not known yet.
#[derive(Debug, Clone, PartialEq)]
pub struct MissingHeaders {
    pub id: String,
    pub provider_id: String,
    pub from: String,
    pub subject: String,
}

/// (message id, from name, from email, subject, body) awaiting the agent.
pub type PendingDigest = (String, String, String, String, String);

/// Someone the user corresponds with: every address they use, plus what signatures said.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Person {
    pub email: String,
    pub emails: Vec<String>,
    #[serde(flatten)]
    pub card: crate::agent::Card,
    /// Conversations (threads) with them, whoever wrote.
    pub conversations: i64,
    pub last_seen: i64,
    /// "person", "organization" or "list" (set by the AI clean-up; "person" until then).
    pub kind: String,
}

/// A directory entry: what signatures taught us about one address.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Contact {
    pub email: String,
    #[serde(flatten)]
    pub card: crate::agent::Card,
    pub last_seen: i64,
}

fn contact_row(r: &Row) -> rusqlite::Result<Contact> {
    let phones: String = r.get(4)?;
    let socials: String = r.get(7)?;
    Ok(Contact {
        email: r.get(0)?,
        card: crate::agent::Card {
            name: r.get(1)?,
            title: r.get(2)?,
            company: r.get(3)?,
            phones: serde_json::from_str(&phones).unwrap_or_default(),
            address: r.get(5)?,
            website: r.get(6)?,
            socials: serde_json::from_str(&socials).unwrap_or_default(),
        },
        last_seen: r.get(8)?,
    })
}

/// Where a new draft attachment comes from.
#[derive(Debug, Clone)]
pub enum AttachmentSource {
    /// A file on this Mac; it is copied, so moving the original later is harmless.
    Path(PathBuf),
    /// An attachment of a message already in Mermoz (forwarding, drag between messages).
    Message(String),
    /// A link shared instead of the bytes (e.g. a Drive file).
    Link(String),
}

#[derive(Debug, Clone)]
pub struct NewAttachment {
    pub source: AttachmentSource,
    /// Name shown to the recipient; defaults to the source's own name.
    pub filename: Option<String>,
    pub inline: bool,
}

fn draft_attachment_row(r: &Row) -> rusqlite::Result<DraftAttachment> {
    let kind: String = r.get(1)?;
    Ok(DraftAttachment {
        id: r.get(0)?,
        kind: if kind == "link" {
            AttachmentKind::Link
        } else {
            AttachmentKind::File
        },
        filename: r.get(2)?,
        mime: r.get(3)?,
        size: r.get(4)?,
        path: r.get(5)?,
        url: r.get(6)?,
        inline: r.get::<_, i64>(7)? != 0,
    })
}

/// Flushes a file's written pages to the disk, as SQLite does with `fullfsync` off.
fn sync_file(file: &std::fs::File) -> Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: fsync only reads the descriptor, which `file` keeps open for the call.
    if unsafe { libc::fsync(file.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// A message's full-text row, written after the rest of its batch.
struct FtsRow<'m> {
    rowid: i64,
    subject: &'m str,
    sender: String,
    recipients: String,
    body: &'m str,
}

const DRAFT_COLS: &str = "id, account_id, thread_id, to_json, cc_json, subject, body, state, origin, created, updated, reply_to_message_id";

impl Store {
    pub fn open(path: &Path, blobs: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::create_dir_all(blobs)?;
        let t0 = std::time::Instant::now();
        let w = Connection::open(path)?;
        w.execute_batch(PRAGMAS)?;
        tracing::debug!(ms = t0.elapsed().as_millis() as u64, "store: pragmas");
        migrate(&w)?;
        tracing::debug!(ms = t0.elapsed().as_millis() as u64, "store: migrated");
        w.busy_timeout(std::time::Duration::from_secs(5))?;
        let reader = || -> Result<Connection> {
            let r = Connection::open(path)?;
            r.execute_batch(READER_PRAGMAS)?;
            r.busy_timeout(std::time::Duration::from_secs(5))?;
            Ok(r)
        };
        let store = Store {
            w: Mutex::new(w),
            r: Mutex::new(reader()?),
            bg: Mutex::new(reader()?),
            ckpt: Mutex::new((reader()?, -1, std::fs::File::open(path)?)),
            blobs: blobs.to_path_buf(),
            counts_cache: Default::default(),
            any_hidden: Default::default(),
        };
        store.refresh_hidden()?;
        store.seed()?;
        tracing::debug!(ms = t0.elapsed().as_millis() as u64, "store: seeded");
        store.backfill_snippets()?;
        tracing::debug!(ms = t0.elapsed().as_millis() as u64, "store: snippets");
        let adopted = store.adopt_stored_gmail_drafts()?;
        if adopted > 0 {
            tracing::info!(adopted, "Gmail drafts stored as mail are drafts again");
        }
        let dropped = store.drop_stale_suggestions(now_ms())?;
        if dropped > 0 {
            tracing::info!(dropped, "dropped live-status suggestions on stale threads");
        }
        tracing::info!(ms = t0.elapsed().as_millis() as u64, "store opened");
        Ok(store)
    }

    /// Copies committed WAL pages into the database file, when anything was written since
    /// the last call and no write is in flight. Run off the write path: SQLite's own
    /// checkpoint runs inside the commit that fills the WAL, holding the write lock for the
    /// whole copy. A write in flight skips it: the copy's fsyncs would flush the WAL the
    /// writer is appending to, stretching both (by 100s of ms on a slow disk) and every
    /// click queued on the lock; the batch copies its own pages after its commit and the
    /// next call catches up the rest. Returns the pages copied.
    pub fn checkpoint(&self) -> Result<i64> {
        if matches!(self.w.try_lock(), Err(std::sync::TryLockError::WouldBlock)) {
            return Ok(0);
        }
        self.copy_wal()
    }

    /// [`Self::checkpoint`] whether or not a write is in flight. Passive, so it never
    /// waits on (or blocks) a writer or a reader.
    fn copy_wal(&self) -> Result<i64> {
        let mut g = self.ckpt.lock().unwrap();
        let (conn, seen, file) = &mut *g;
        let version: i64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        if version == *seen {
            return Ok(0);
        }
        *seen = version;
        let (log, copied): (i64, i64) =
            conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| {
                Ok((r.get(1)?, r.get(2)?))
            })?;
        // SQLite syncs the database file only when no commit lands during the copy, which
        // under a sync with clicks is almost never: the copied pages would pile up for
        // the rewind below to flush under the write lock (~1 s at 500k). Synced here, each
        // flush is one batch's pages, off the lock. Plain fsync, as SQLite's own.
        if copied > 0 {
            sync_file(file)?;
        }
        // The WAL only starts over when a write finds no reader on it; under steady reads
        // it keeps growing. Past 64 MB it is rewound by the writer itself (other writes
        // queue on its lock, never fail busy), giving up after 50 ms of waiting on readers.
        if log > 16_384 && copied == log {
            let w = self.w.lock().unwrap();
            w.busy_timeout(std::time::Duration::from_millis(50))?;
            let _ = w.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
            w.busy_timeout(std::time::Duration::from_secs(5))?;
        }
        Ok(copied)
    }

    /// Recomputes every list preview once when the preview rules change.
    fn backfill_snippets(&self) -> Result<()> {
        const VERSION: &str = "2";
        let mut w = self.w.lock().unwrap();
        let done: Option<String> = w
            .query_row(
                "SELECT value FROM meta WHERE key = 'snippet_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if done.as_deref() == Some(VERSION) {
            return Ok(());
        }
        let tx = w.transaction()?;
        let rows: Vec<(String, String)> = {
            let mut st = tx.prepare(
                "SELECT t.id, COALESCE((SELECT body_text FROM messages m WHERE m.thread_id = t.id ORDER BY date DESC LIMIT 1), '') FROM threads t",
            )?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, body) in rows {
            tx.execute(
                "UPDATE threads SET snippet = ?2 WHERE id = ?1",
                params![id, crate::quote::snippet(&body)],
            )?;
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('snippet_version', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [VERSION],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn seed(&self) -> Result<()> {
        let w = self.w.lock().unwrap();
        for (i, (key, name, color, archived)) in DEFAULT_STATUSES.iter().enumerate() {
            w.execute(
                "INSERT OR IGNORE INTO statuses (key, name, ord, color, archived) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![key, name, i as i64, color, *archived as i64],
            )?;
        }
        w.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('next_number', '1')",
            [],
        )?;
        Ok(())
    }

    // ---- accounts -------------------------------------------------------

    pub fn add_account(&self, email: &str, provider: &str) -> Result<Account> {
        let w = self.w.lock().unwrap();
        let existing: Option<String> = w
            .query_row("SELECT id FROM accounts WHERE email = ?1", [email], |r| {
                r.get(0)
            })
            .optional()?;
        let id = match existing {
            Some(id) => id,
            None => {
                let id = new_id("acc");
                w.execute(
                    "INSERT INTO accounts (id, email, provider, created) VALUES (?1, ?2, ?3, ?4)",
                    params![id, email, provider, now_ms()],
                )?;
                id
            }
        };
        drop(w);
        self.account(&id)
    }

    pub fn account(&self, id: &str) -> Result<Account> {
        self.accounts()?
            .into_iter()
            .find(|a| a.id == id || a.email == id)
            .ok_or_else(|| Error::NotFound(format!("account {id}")))
    }

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare("SELECT id, email, provider, sync_state, synced, total, error, hidden FROM accounts ORDER BY created")?;
        let rows = st.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                email: r.get(1)?,
                provider: r.get(2)?,
                sync: SyncInfo {
                    state: r.get(3)?,
                    synced: r.get(4)?,
                    total: r.get(5)?,
                    error: r.get(6)?,
                },
                hidden: r.get::<_, i64>(7)? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_account(&self, id: &str) -> Result<()> {
        let acc = self.account(id)?;
        let w = self.w.lock().unwrap();
        w.execute(
            "DELETE FROM messages_fts WHERE rowid IN (SELECT fts_rowid FROM messages WHERE account_id = ?1)",
            [&acc.id],
        )?;
        w.execute("DELETE FROM accounts WHERE id = ?1", [&acc.id])?;
        drop(w);
        self.refresh_hidden()
    }

    pub fn set_sync(
        &self,
        id: &str,
        state: &str,
        synced: Option<i64>,
        total: Option<i64>,
        error: Option<&str>,
    ) -> Result<()> {
        let w = self.w.lock().unwrap();
        w.execute(
            "UPDATE accounts SET sync_state = ?2, synced = COALESCE(?3, synced), total = COALESCE(?4, total), error = ?5 WHERE id = ?1",
            params![id, state, synced, total, error],
        )?;
        Ok(())
    }

    pub fn history_id(&self, id: &str) -> Result<Option<String>> {
        let r = self.r.lock().unwrap();
        Ok(
            r.query_row("SELECT history_id FROM accounts WHERE id = ?1", [id], |r| {
                r.get(0)
            })?,
        )
    }

    pub fn set_history_id(&self, id: &str, h: &str) -> Result<()> {
        self.w.lock().unwrap().execute(
            "UPDATE accounts SET history_id = ?2 WHERE id = ?1",
            params![id, h],
        )?;
        Ok(())
    }

    /// Initial-sync page token, so a restart resumes instead of starting over.
    pub fn cursor(&self, id: &str) -> Result<Option<String>> {
        let r = self.r.lock().unwrap();
        Ok(
            r.query_row("SELECT cursor FROM accounts WHERE id = ?1", [id], |r| {
                r.get(0)
            })?,
        )
    }

    pub fn set_cursor(&self, id: &str, c: Option<&str>) -> Result<()> {
        self.w.lock().unwrap().execute(
            "UPDATE accounts SET cursor = ?2 WHERE id = ?1",
            params![id, c],
        )?;
        Ok(())
    }

    // ---- labels -----------------------------------------------------------

    pub fn set_labels(&self, account_id: &str, labels: &[(String, String)]) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        for (id, name) in labels {
            tx.execute(
                "INSERT INTO labels (account_id, label_id, name) VALUES (?1, ?2, ?3)
                 ON CONFLICT(account_id, label_id) DO UPDATE SET name = excluded.name",
                params![account_id, id, name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn label_id(&self, account_id: &str, name: &str) -> Result<Option<String>> {
        let r = self.r.lock().unwrap();
        Ok(r.query_row(
            "SELECT label_id FROM labels WHERE account_id = ?1 AND name = ?2",
            params![account_id, name],
            |r| r.get(0),
        )
        .optional()?)
    }

    fn label_names(
        conn: &Connection,
        account_id: &str,
        ids: &[String],
    ) -> rusqlite::Result<Vec<String>> {
        let mut st =
            conn.prepare_cached("SELECT name FROM labels WHERE account_id = ?1 AND label_id = ?2")?;
        let mut out = vec![];
        for id in ids {
            match st
                .query_row(params![account_id, id], |r| r.get::<_, String>(0))
                .optional()?
            {
                Some(n) => out.push(n),
                // System labels (INBOX, UNREAD…) are their own names.
                None => out.push(id.clone()),
            }
        }
        Ok(out)
    }

    // ---- sync ingestion ------------------------------------------------

    pub fn has_message(&self, account_id: &str, provider_id: &str) -> Result<bool> {
        let r = self.r.lock().unwrap();
        // A Gmail draft is held as a draft, never as a message: known all the same.
        Ok(r.query_row(
            "SELECT 1 FROM messages WHERE account_id = ?1 AND provider_id = ?2
             UNION ALL SELECT 1 FROM drafts WHERE account_id = ?1 AND provider_id = ?2 LIMIT 1",
            params![account_id, provider_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
    }

    /// Inserts a new message (or refreshes labels of a known one) and keeps its
    /// thread's aggregates and issue status consistent. Returns the thread id.
    pub fn ingest(
        &self,
        account_id: &str,
        me: &str,
        msg: &IncomingMessage,
        mode: SyncMode,
    ) -> Result<String> {
        let reading = reading_of(msg);
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let mut fts = vec![];
        let thread_id = self.ingest_tx(&tx, account_id, me, (msg, &reading), mode, &mut fts)?;
        Self::index_fts(&tx, &fts)?;
        tx.commit()?;
        Ok(thread_id)
    }

    /// Batch variant: one transaction for many messages keeps initial sync fast.
    pub fn ingest_many(
        &self,
        account_id: &str,
        me: &str,
        msgs: &[IncomingMessage],
        mode: SyncMode,
    ) -> Result<Vec<String>> {
        // Parsed before the write lock is taken: clicks never wait on the parsing.
        let readings = par_map(msgs, reading_of);
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let mut ids = vec![];
        // Full-text rows are written once every savepoint is released: FTS5 flushes its
        // pending index on each savepoint, which wrote one tiny segment per message.
        let mut fts = vec![];
        for (m, reading) in msgs.iter().zip(&readings) {
            // One malformed message must not stall the mailbox: isolate each in a savepoint,
            // record failures for a later retry, and keep importing.
            tx.execute_batch("SAVEPOINT msg")?;
            let indexed = fts.len();
            match self.ingest_tx(&tx, account_id, me, (m, reading), mode, &mut fts) {
                Ok(id) => {
                    tx.execute_batch("RELEASE msg")?;
                    // Empty: a Gmail draft answering no stored thread.
                    if !id.is_empty() {
                        ids.push(id);
                    }
                }
                Err(e) => {
                    fts.truncate(indexed);
                    tx.execute_batch("ROLLBACK TO msg; RELEASE msg")?;
                    tracing::warn!(provider_id = %m.provider_id, "skipping message: {e}");
                    tx.execute(
                        "INSERT INTO sync_failures (account_id, provider_id, error, at) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(account_id, provider_id) DO UPDATE SET error = excluded.error, at = excluded.at",
                        params![account_id, m.provider_id, e.to_string(), now_ms()],
                    )?;
                }
            }
        }
        Self::index_fts(&tx, &fts)?;
        tx.commit()?;
        drop(w);
        // A sync writes batch after batch: the WAL is copied back after each, with the
        // write lock already released, so clicks never wait on the copy.
        self.copy_wal()?;
        Ok(ids)
    }

    fn index_fts(tx: &Connection, rows: &[FtsRow]) -> Result<()> {
        let mut st = tx.prepare_cached(
            "INSERT INTO messages_fts (rowid, subject, sender, recipients, body) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for f in rows {
            st.execute(params![f.rowid, f.subject, f.sender, f.recipients, f.body])?;
        }
        Ok(())
    }

    /// Stores one message with its reading (worked out before the write lock); its
    /// full-text row is queued in `fts` for [`Self::index_fts`].
    fn ingest_tx<'m>(
        &self,
        tx: &Connection,
        account_id: &str,
        me: &str,
        (msg, reading): (&'m IncomingMessage, &crate::reading::Reading),
        mode: SyncMode,
        fts: &mut Vec<FtsRow<'m>>,
    ) -> Result<String> {
        // A Gmail draft is a draft, not mail; its thread (if stored) shows it.
        if gmail_drafts::is_draft(&msg.label_ids) {
            return Ok(gmail_drafts::upsert_tx(tx, account_id, msg)?.unwrap_or_default());
        }
        // A Gmail draft sent under its own message id is mail from now on.
        gmail_drafts::remove_tx(tx, account_id, &msg.provider_id)?;
        let labels_json = serde_json::to_string(&msg.label_ids)?;
        let unread = msg.label_ids.iter().any(|l| l == "UNREAD");
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT id, thread_id FROM messages WHERE account_id = ?1 AND provider_id = ?2",
                params![account_id, msg.provider_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((mid, tid)) = existing {
            let was_unread: bool =
                tx.query_row("SELECT unread FROM threads WHERE id = ?1", [&tid], |r| {
                    Ok(r.get::<_, i64>(0)? != 0)
                })?;
            // A local read/unread not yet pushed wins over the server's older view.
            tx.execute(
                "UPDATE messages SET labels_json = ?2,
                   unread = CASE WHEN EXISTS (SELECT 1 FROM ops WHERE kind = 'read' AND key = ?4) THEN unread ELSE ?3 END
                 WHERE id = ?1",
                params![mid, labels_json, unread as i64, tid],
            )?;
            Self::refresh_thread(tx, &tid)?;
            if mode == SyncMode::Incremental {
                Self::adopt_remote_labels(tx, account_id, &tid, was_unread)?;
            }
            return Ok(tid);
        }

        let p = &msg.parsed;
        let from = p.from.clone().unwrap_or(Address {
            name: String::new(),
            email: String::new(),
        });
        let is_from_me =
            msg.label_ids.iter().any(|l| l == "SENT") || from.email.eq_ignore_ascii_case(me);
        let date = if msg.internal_date > 0 {
            msg.internal_date
        } else {
            p.date.unwrap_or_else(now_ms)
        };

        let thread: Option<(String, String)> = tx
            .query_row(
                "SELECT id, status FROM threads WHERE account_id = ?1 AND provider_thread_id = ?2",
                params![account_id, msg.provider_thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (thread_id, new_thread) = match thread {
            Some((tid, status)) => {
                let in_inbox = msg.label_ids.iter().any(|l| l == "INBOX");
                if mode == SyncMode::Incremental
                    && status_after_new_message(&status, is_from_me, in_inbox).is_some()
                {
                    Self::enter_unread_tx(tx, account_id, &tid, &status)?;
                }
                (tid, false)
            }
            None => {
                let tid = new_id("thr");
                let number: i64 = tx
                    .query_row(
                        "SELECT value FROM meta WHERE key = 'next_number'",
                        [],
                        |r| r.get::<_, String>(0),
                    )?
                    .parse()
                    .unwrap_or(1);
                tx.execute(
                    "UPDATE meta SET value = ?1 WHERE key = 'next_number'",
                    [(number + 1).to_string()],
                )?;
                tx.execute(
                    "INSERT INTO threads (id, account_id, provider_thread_id, number, status, conversation_id) VALUES (?1, ?2, ?3, ?4, 'unread', ?1)",
                    params![tid, account_id, msg.provider_thread_id, number],
                )?;
                gmail_drafts::attach_tx(tx, account_id, &msg.provider_thread_id, &tid)?;
                (tid, true)
            }
        };

        let mid = new_id("msg");
        tx.execute(
            "INSERT INTO messages (id, account_id, provider_id, thread_id, from_name, from_email, to_json, cc_json, date, subject,
               body_text, body_html, labels_json, is_from_me, unread, message_id_header, references_json, auth, bcc_json, list_id, reply_to_json,
               auto_kind, dsn_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![
                mid,
                account_id,
                msg.provider_id,
                thread_id,
                from.name,
                from.email,
                serde_json::to_string(&p.to)?,
                serde_json::to_string(&p.cc)?,
                date,
                p.subject,
                p.body_text,
                None::<String>,
                labels_json,
                is_from_me as i64,
                unread as i64,
                p.message_id,
                serde_json::to_string(&p.references)?,
                if is_from_me { "self" } else { crate::auth::verdict(&p.auth_results, &from.email) },
                serde_json::to_string(&p.bcc)?,
                p.list_id,
                serde_json::to_string(&p.reply_to)?,
                p.auto_kind,
                p.dsn.as_ref().map(serde_json::to_string).transpose()?,
            ],
        )?;
        let msg_rowid = tx.last_insert_rowid();
        let fts_rowid = fts_rowid(date, msg_rowid);
        if let Some(html) = &p.body_html {
            tx.prepare_cached("INSERT INTO message_html (message_id, html) VALUES (?1, ?2)")?
                .execute(params![mid, html])?;
        }
        readings::save(tx, msg_rowid, &mid, reading)?;
        tx.execute(
            "UPDATE messages SET fts_rowid = ?2 WHERE id = ?1",
            params![mid, fts_rowid],
        )?;
        let recipients: Vec<String> = p.to.iter().chain(p.cc.iter()).map(fmt_addr).collect();
        fts.push(FtsRow {
            rowid: fts_rowid,
            subject: &p.subject,
            sender: fmt_addr(&from),
            recipients: recipients.join(", "),
            body: &p.body_text,
        });
        if is_from_me {
            conversations::note_correspondents(tx, p.to.iter().chain(&p.cc).chain(&p.bcc))?;
        }
        addresses::note(tx, std::iter::once(&from).chain(&p.to).chain(&p.cc))?;
        let decorative = images::classify_incoming(
            tx,
            &from.email,
            &mid,
            &p.attachments,
            p.body_html.as_deref(),
        )?;
        for (a, decorative) in p.attachments.iter().zip(decorative) {
            let aid = new_id("att");
            let dir = self.blobs.join(&aid);
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(safe_filename(&a.filename));
            std::fs::write(&path, &a.data)?;
            tx.execute(
                "INSERT INTO attachments (id, message_id, filename, mime, size, path, content_id, inline, decorative) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![aid, mid, a.filename, a.mime, a.data.len() as i64, path.to_string_lossy(), a.content_id, a.inline as i64, decorative as i64],
            )?;
        }
        // A second delivery of an email already stored (its parts now saved) points at the copy kept.
        duplicates::apply(tx, &duplicates::regroup(tx, &mid)?)?;
        Self::refresh_thread(tx, &thread_id)?;
        if new_thread {
            let (in_inbox, names) = Self::thread_remote_labels(tx, account_id, &thread_id)?;
            let status_label = Self::status_from_labels(tx, &names)?;
            let (last_date, unread): (i64, bool) = tx.query_row(
                "SELECT last_date, unread FROM threads WHERE id = ?1",
                [&thread_id],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
            )?;
            let status = initial_status(
                in_inbox,
                names.contains("TRASH"),
                unread,
                status_label.as_deref(),
                last_date,
                now_ms(),
                mode,
            );
            tx.execute(
                "UPDATE threads SET status = ?2 WHERE id = ?1",
                params![thread_id, status],
            )?;
            Self::adopt_meta_labels(tx, &thread_id, &names)?;
        }
        Ok(thread_id)
    }

    /// Recomputes denormalized thread fields from its messages.
    fn refresh_thread(tx: &Connection, thread_id: &str) -> Result<()> {
        Self::index_thread_labels(tx, thread_id)?;
        let account_id: String = tx.query_row(
            "SELECT account_id FROM threads WHERE id = ?1",
            [thread_id],
            |r| r.get(0),
        )?;
        Self::sync_projects_tx(tx, &account_id, thread_id)?;
        // Only the last message's body makes the snippet: the others' are not read.
        let mut st = tx.prepare_cached(
            "SELECT from_name, from_email, is_from_me, unread, date, subject, m.id,
               (SELECT COUNT(*) FROM attachments a WHERE a.message_id = m.id)
             FROM messages m WHERE thread_id = ?1 ORDER BY date",
        )?;
        let rows: Vec<MsgAgg> = st
            .query_map([thread_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)? != 0,
                    r.get::<_, i64>(3)? != 0,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let Some(last) = rows.last() else {
            return Ok(());
        };
        let mut participants: Vec<String> = vec![];
        for (name, email, me, ..) in &rows {
            let n = if *me {
                "You".to_string()
            } else {
                display_name(name, email)
            };
            if !participants.contains(&n) {
                participants.push(n);
            }
        }
        participants.truncate(4);
        let subject = rows
            .iter()
            .map(|r| r.5.clone())
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        let body: String = tx
            .prepare_cached("SELECT body_text FROM messages WHERE id = ?1")?
            .query_row([&last.6], |r| r.get(0))?;
        let snippet = crate::quote::snippet(&body);
        let last_from = if last.2 {
            "You".to_string()
        } else {
            display_name(&last.0, &last.1)
        };
        tx.execute(
            "UPDATE threads SET subject = ?2, snippet = ?3, participants = ?4, last_from = ?5, last_date = ?6,
               unread = ?7, message_count = ?8, has_attachments = ?9, first_date = ?10 WHERE id = ?1",
            params![
                thread_id,
                subject,
                snippet,
                serde_json::to_string(&participants)?,
                last_from,
                last.4,
                rows.iter().any(|r| r.3) as i64,
                // Second deliveries of an email are not counted.
                tx.prepare_cached(
                    "SELECT COUNT(*) FROM messages WHERE thread_id = ?1 AND duplicate_of IS NULL"
                )?
                .query_row([thread_id], |r| r.get::<_, i64>(0))?,
                rows.iter().any(|r| r.7 > 0) as i64,
                rows[0].4,
            ],
        )?;
        Ok(())
    }

    fn index_thread_labels(tx: &Connection, thread_id: &str) -> Result<()> {
        tx.execute(
            "DELETE FROM thread_labels WHERE thread_id = ?1",
            [thread_id],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO thread_labels (thread_id, account_id, label_id)
             SELECT DISTINCT m.thread_id, m.account_id, j.value FROM messages m, json_each(m.labels_json) j WHERE m.thread_id = ?1",
            [thread_id],
        )?;
        Ok(())
    }

    /// The user's own labels per account, with thread counts; nested names use "/".
    pub fn user_labels(&self) -> Result<Vec<Label>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            &format!("SELECT l.account_id, l.label_id, l.name, (SELECT COUNT(*) FROM thread_labels tl WHERE tl.account_id = l.account_id AND tl.label_id = l.label_id)
             FROM labels l WHERE 1{} ORDER BY l.name COLLATE NOCASE",
            self.visible_on("l")
        ))?;
        let rows = st
            .query_map([], |r| {
                Ok(Label {
                    account_id: r.get(0)?,
                    id: r.get(1)?,
                    name: r.get(2)?,
                    count: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter(|l| {
                !SYSTEM_LABELS.contains(&l.id.as_str())
                    && !l.id.starts_with("CATEGORY_")
                    && !l.name.starts_with(LABEL_PREFIX)
                    && l.name != "Mermoz"
            })
            .collect())
    }

    /// Union of label names across the thread's messages, as last seen on the server.
    fn thread_remote_labels(
        tx: &Connection,
        account_id: &str,
        thread_id: &str,
    ) -> Result<(bool, BTreeSet<String>)> {
        let mut st = tx.prepare_cached("SELECT labels_json FROM messages WHERE thread_id = ?1")?;
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for row in st.query_map([thread_id], |r| r.get::<_, String>(0))? {
            let v: Vec<String> = serde_json::from_str(&row?).unwrap_or_default();
            ids.extend(v);
        }
        let ids: Vec<String> = ids.into_iter().collect();
        let names: BTreeSet<String> = Self::label_names(tx, account_id, &ids)?
            .into_iter()
            .collect();
        Ok((names.contains("INBOX"), names))
    }

    fn status_from_labels(tx: &Connection, names: &BTreeSet<String>) -> Result<Option<String>> {
        let mut st = tx.prepare_cached("SELECT key, name FROM statuses")?;
        let statuses: Vec<(String, String)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (key, name) in statuses {
            if names.contains(&format!("{LABEL_PREFIX}{name}")) {
                return Ok(Some(key));
            }
        }
        Ok(None)
    }

    /// A priority label set on another device becomes the local field.
    fn adopt_meta_labels(tx: &Connection, thread_id: &str, names: &BTreeSet<String>) -> Result<()> {
        for n in names {
            if let Some(p) = n.strip_prefix(&format!("{LABEL_PREFIX}Priority/")) {
                let v = (1..=4).find(|i| priority_name(*i) == Some(p)).unwrap_or(0);
                tx.execute(
                    "UPDATE threads SET priority = ?2 WHERE id = ?1",
                    params![thread_id, v],
                )?;
            }
        }
        Ok(())
    }

    /// Label and read-state changes made elsewhere (phone, webmail) update the status,
    /// unless a local change is still waiting to be pushed — local intent wins.
    fn adopt_remote_labels(
        tx: &Connection,
        account_id: &str,
        thread_id: &str,
        was_unread: bool,
    ) -> Result<()> {
        let pending: bool = tx
            .query_row(
                "SELECT 1 FROM ops WHERE kind IN ('labels', 'read') AND key = ?1",
                [thread_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if pending {
            return Ok(());
        }
        let (in_inbox, names) = Self::thread_remote_labels(tx, account_id, thread_id)?;
        let (current, unread): (String, bool) = tx.query_row(
            "SELECT status, unread FROM threads WHERE id = ?1",
            [thread_id],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
        )?;
        let archived: bool = tx
            .query_row(
                "SELECT archived FROM statuses WHERE key = ?1",
                [&current],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            != 0;
        let back_in_inbox = if unread { "unread" } else { "todo" };
        let next = if names.contains("TRASH") {
            Some("deleted".to_string())
        } else {
            match Self::status_from_labels(tx, &names)? {
                Some(s) => Some(s),
                None if current == "deleted" => {
                    Some(if in_inbox { back_in_inbox } else { "archive" }.to_string())
                }
                None if !in_inbox && !archived => Some("archive".to_string()),
                None if in_inbox && archived => Some(back_in_inbox.to_string()),
                None => None,
            }
        };
        match next.filter(|s| *s != current) {
            Some(s) => {
                tx.execute(
                    "UPDATE threads SET status = ?2, prev_status = NULL WHERE id = ?1",
                    params![thread_id, s],
                )?;
            }
            // Only a change of read state counts: an unread thread can be filed without reading.
            None if was_unread && !unread && current == "unread" => {
                Self::leave_unread_tx(tx, account_id, thread_id)?
            }
            None if !was_unread
                && unread
                && in_inbox
                && !archived
                && current != "unread"
                && current != "review" =>
            {
                Self::enter_unread_tx(tx, account_id, thread_id, &current)?
            }
            None => {}
        }
        Self::adopt_meta_labels(tx, thread_id, &names)
    }

    // ---- threads ---------------------------------------------------------

    pub fn resolve_thread_id(&self, id_or_number: &str) -> Result<String> {
        let r = self.r.lock().unwrap();
        let num = id_or_number
            .strip_prefix("M-")
            .or_else(|| id_or_number.strip_prefix("m-"))
            .and_then(|n| n.parse::<i64>().ok());
        let found: Option<String> = match num {
            Some(n) => r
                .query_row("SELECT id FROM threads WHERE number = ?1", [n], |r| {
                    r.get(0)
                })
                .optional()?,
            None => r
                .query_row(
                    "SELECT id FROM threads WHERE id = ?1",
                    [id_or_number],
                    |r| r.get(0),
                )
                .optional()?,
        };
        found.ok_or_else(|| Error::NotFound(format!("thread {id_or_number}")))
    }

    pub fn threads(&self, f: &ThreadFilter) -> Result<(Vec<ThreadSummary>, i64)> {
        let mut clauses: Vec<String> = vec![];
        let mut args: Vec<rusqlite::types::Value> = vec![];
        if let Some(s) = &f.status {
            clauses.push(format!("t.status IN ({})", vec!["?"; s.len()].join(",")));
            args.extend(s.iter().map(|x| x.clone().into()));
        }
        if let Some(p) = &f.project {
            clauses.push(
                "t.id IN (SELECT tp.thread_id FROM thread_projects tp WHERE tp.name = ?)".into(),
            );
            args.push(p.clone().into());
        }
        if let Some(a) = &f.account_id {
            clauses.push("t.account_id = ?".into());
            args.push(a.clone().into());
        }
        if let Some(l) = &f.label_id {
            clauses.push("t.id IN (SELECT tl.thread_id FROM thread_labels tl WHERE tl.label_id = ? AND (? IS NULL OR tl.account_id = ?))".into());
            args.push(l.clone().into());
            args.push(f.account_id.clone().into());
            args.push(f.account_id.clone().into());
        }
        if let Some(es) = f.participants.as_ref().filter(|e| !e.is_empty()) {
            // Threads they wrote in, or where the user wrote to them.
            let list = vec!["lower(?)"; es.len()].join(",");
            clauses.push(format!(
                "(t.id IN (SELECT m.thread_id FROM messages m WHERE lower(m.from_email) IN ({list}))
                  OR t.id IN (SELECT m.thread_id FROM messages m, json_each(m.to_json) j
                              WHERE m.is_from_me = 1 AND lower(json_extract(j.value, '$.email')) IN ({list})))"
            ));
            args.extend(es.iter().map(|e| e.clone().into()));
            args.extend(es.iter().map(|e| e.clone().into()));
        }
        if let Some(p) = &f.priority {
            clauses.push(format!("t.priority IN ({})", vec!["?"; p.len()].join(",")));
            args.extend(p.iter().map(|x| (*x).into()));
        }
        if let Some(sf) = &f.suggested {
            let mut c = format!(
                "EXISTS (SELECT 1 FROM suggestions s WHERE s.thread_id = t.id AND {PENDING_SUGGESTION}
                   AND (s.status IS NOT NULL OR s.projects_json != '[]')"
            );
            if let SuggestedFilter::To(g) = sf {
                c.push_str(" AND s.status IS ? AND s.projects_json = ?");
                args.push(g.status.clone().into());
                args.push(serde_json::to_string(&g.projects)?.into());
            }
            c.push(')');
            clauses.push(c);
        }
        if f.conversations {
            clauses.push("t.conv_lead = 1".into());
        }
        if let Some(k) = &f.kinds {
            clauses.push(format!("t.conv_kind IN ({})", vec!["?"; k.len()].join(",")));
            args.extend(k.iter().map(|x| x.clone().into()));
        }
        clauses.extend(self.visible_clause());
        // Mail put off with Later leaves its column until it comes back (or new mail arrives).
        if f.status.is_some() {
            clauses.push(
                "NOT EXISTS (SELECT 1 FROM feed_snoozes z WHERE z.thread_id = t.id AND z.woke = 0 AND z.for_date >= t.last_date)"
                    .into(),
            );
        }
        let wh = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let r = self.r.lock().unwrap();
        let total: i64 = r.query_row(
            &format!("SELECT COUNT(*) FROM threads t {wh}"),
            params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        let limit = if f.limit <= 0 { 200 } else { f.limit };
        let sql = format!(
            "SELECT {THREAD_COLS} FROM threads t {wh} ORDER BY {} LIMIT {limit} OFFSET {}",
            order_clause(f.order.as_deref(), f.reverse),
            f.offset.max(0)
        );
        let mut st = r.prepare_cached(&sql)?;
        let mut rows = st
            .query_map(params_from_iter(args.iter()), thread_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        attach(&r, &mut rows)?;
        Ok((rows, total))
    }

    pub fn thread(&self, id: &str) -> Result<ThreadSummary> {
        let r = self.r.lock().unwrap();
        let t = r
            .query_row(
                &format!("SELECT {THREAD_COLS} FROM threads t WHERE t.id = ?1"),
                [id],
                thread_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("thread {id}")))?;
        let mut rows = [t];
        attach(&r, &mut rows)?;
        let [t] = rows;
        Ok(t)
    }

    pub fn messages(&self, thread_id: &str) -> Result<Vec<Message>> {
        let r = self.r.lock().unwrap();
        messages_where(&r, "m.thread_id = ?1", thread_id, None)
    }

    /// One message, by its Mermoz id.
    pub fn message(&self, id: &str) -> Result<Message> {
        let r = self.r.lock().unwrap();
        messages_where(&r, "m.id = ?1", id, None)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("message {id}")))
    }

    /// A message's own parts its HTML can show inline (`cid:` links), read from disk.
    pub fn inline_parts(&self, message_id: &str) -> Result<Vec<crate::inline::InlinePart>> {
        let rows: Vec<(Option<String>, String, String, String)> = {
            let r = self.r.lock().unwrap();
            let mut st = r.prepare_cached(
                "SELECT content_id, filename, mime, path FROM attachments WHERE message_id = ?1 AND mime LIKE 'image/%'",
            )?;
            st.query_map([message_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        Ok(rows
            .into_iter()
            .filter_map(|(content_id, filename, mime, path)| {
                Some(crate::inline::InlinePart {
                    content_id,
                    filename,
                    mime,
                    data: std::fs::read(path).ok()?,
                })
            })
            .collect())
    }

    /// (account id, Gmail message id) of a message whose image parts were stored
    /// before Content-IDs were kept, so they can be learnt from Gmail once.
    pub fn message_missing_cids(&self, message_id: &str) -> Result<Option<(String, String)>> {
        let r = self.r.lock().unwrap();
        Ok(r.query_row(
            "SELECT m.account_id, m.provider_id FROM messages m WHERE m.id = ?1 AND EXISTS
               (SELECT 1 FROM attachments a WHERE a.message_id = m.id AND a.content_id IS NULL AND a.mime LIKE 'image/%')",
            [message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
    }

    /// Records the Content-ID and placement of a message's parts, matched by file name
    /// in order. Parts without a Content-ID get an empty one, so they are asked once.
    pub fn set_attachment_cids(
        &self,
        message_id: &str,
        parts: &[crate::mime::ParsedAttachment],
    ) -> Result<()> {
        let w = self.w.lock().unwrap();
        let ids: Vec<(String, String)> = w
            .prepare_cached(
                "SELECT id, filename FROM attachments WHERE message_id = ?1 ORDER BY rowid",
            )?
            .query_map([message_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut used = vec![false; parts.len()];
        for (id, name) in ids {
            let found = parts
                .iter()
                .enumerate()
                .find(|(i, p)| !used[*i] && p.filename == name);
            let (cid, inline) = match found {
                Some((i, p)) => {
                    used[i] = true;
                    (p.content_id.clone().unwrap_or_default(), p.inline)
                }
                None => (String::new(), false),
            };
            w.execute(
                "UPDATE attachments SET content_id = ?2, inline = ?3 WHERE id = ?1",
                params![id, cid, inline as i64],
            )?;
        }
        Ok(())
    }

    /// (account id, Gmail message id) of a stored message.
    pub fn message_origin(&self, message_id: &str) -> Result<(String, String)> {
        let r = self.r.lock().unwrap();
        r.query_row(
            "SELECT account_id, provider_id FROM messages WHERE id = ?1",
            [message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("message {message_id}")))
    }

    /// (HTML body, subject) of a message.
    pub fn message_html(&self, id: &str) -> Result<(Option<String>, String)> {
        let r = self.r.lock().unwrap();
        r.query_row(
            &format!(
                "SELECT {}, subject FROM messages m WHERE id = ?1",
                bodies::HTML_OF_M
            ),
            [id],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("message {id}")))
    }

    pub fn attachment(&self, id: &str) -> Result<(String, String, String, i64)> {
        let r = self.r.lock().unwrap();
        r.query_row(
            "SELECT path, filename, mime, size FROM attachments WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("attachment {id}")))
    }

    /// (account id, thread id) an attachment arrived in.
    pub fn attachment_thread(&self, id: &str) -> Result<(String, String)> {
        let r = self.r.lock().unwrap();
        r.query_row(
            "SELECT m.account_id, m.thread_id FROM attachments a JOIN messages m ON m.id = a.message_id WHERE a.id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("attachment {id}")))
    }

    /// Applies an issue edit locally right away and queues the Gmail label push. The
    /// other open threads of each conversation follow (one status per conversation).
    pub fn update_threads(&self, ids: &[String], patch: &ThreadPatch) -> Result<usize> {
        self.patch_threads(ids, patch, true)
    }

    /// [`Self::update_threads`] on exactly these threads: their conversations' other
    /// threads stay as they are.
    pub fn update_threads_exactly(&self, ids: &[String], patch: &ThreadPatch) -> Result<usize> {
        self.patch_threads(ids, patch, false)
    }

    /// The threads touched by [`Self::update_threads`] on `ids`, as they are now: what
    /// `threads.restore` takes to undo it exactly.
    pub fn states_before_update(&self, ids: &[String]) -> Result<Vec<ThreadState>> {
        self.with_open_siblings(ids)?
            .iter()
            .map(|t| Ok(ThreadState::from(&self.thread(t)?)))
            .collect()
    }

    /// `follow`: the open siblings of each thread take the same patch.
    fn patch_threads(&self, ids: &[String], patch: &ThreadPatch, follow: bool) -> Result<usize> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        if let Some(s) = &patch.status {
            let ok = tx
                .query_row("SELECT 1 FROM statuses WHERE key = ?1", [s], |_| Ok(()))
                .optional()?
                .is_some();
            if !ok {
                return Err(Error::InvalidParams(format!("unknown status '{s}'")));
            }
        }
        for p in patch.projects.iter().flatten().chain(&patch.add_projects) {
            validate_project_name(p)?;
        }
        if let Some(Some(d)) = &patch.due
            && chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err()
        {
            return Err(Error::InvalidParams(format!(
                "due must be YYYY-MM-DD, got '{d}'"
            )));
        }
        if let Some(p) = patch.priority
            && !(0..=4).contains(&p)
        {
            return Err(Error::InvalidParams(format!(
                "priority must be 0..4, got {p}"
            )));
        }
        // One status per conversation: the other open threads of each conversation follow.
        let mut all: Vec<String> = vec![];
        for id in ids {
            if !all.contains(id) {
                all.push(id.clone());
            }
            let siblings = if follow {
                conversations::open_siblings(&tx, id).unwrap_or_default()
            } else {
                vec![]
            };
            for sib in siblings {
                if !all.contains(&sib) {
                    all.push(sib);
                }
            }
        }
        let mut n = 0;
        for id in &all {
            let acc: Option<String> = tx
                .query_row("SELECT account_id FROM threads WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()?;
            let Some(acc) = acc else {
                return Err(Error::NotFound(format!("thread {id}")));
            };
            if let Some(s) = &patch.status {
                tx.execute(
                    "UPDATE threads SET status = ?2, prev_status = NULL WHERE id = ?1",
                    params![id, s],
                )?;
                Self::decide_suggestion_tx(&tx, id, "user")?;
            }
            if let Some(p) = patch.priority {
                tx.execute(
                    "UPDATE threads SET priority = ?2 WHERE id = ?1",
                    params![id, p],
                )?;
            }
            if patch.touches_projects() {
                let current = Self::thread_projects_tx(&tx, id)?;
                let mut next: BTreeSet<String> = match &patch.projects {
                    Some(v) => v.iter().map(|p| p.trim().to_string()).collect(),
                    None => current.clone(),
                };
                next.extend(patch.add_projects.iter().map(|p| p.trim().to_string()));
                for p in &patch.remove_projects {
                    next.remove(p.trim());
                }
                let add: Vec<String> = next.difference(&current).cloned().collect();
                let remove: Vec<String> = current.difference(&next).cloned().collect();
                Self::set_thread_projects_tx(&tx, &acc, id, &add, &remove)?;
            }
            if let Some(d) = &patch.due {
                tx.execute("UPDATE threads SET due = ?2 WHERE id = ?1", params![id, d])?;
            }
            if patch.status.is_some() || patch.priority.is_some() {
                Self::enqueue_label_sync_tx(&tx, &acc, id)?;
            }
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Reading leaves Unread for the thread's next status; marking unread brings it back.
    pub fn mark_read(&self, thread_id: &str, read: bool) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let (acc, status): (String, String) = tx
            .query_row(
                "SELECT account_id, status FROM threads WHERE id = ?1",
                [thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("thread {thread_id}")))?;
        tx.execute(
            "UPDATE messages SET unread = ?2 WHERE thread_id = ?1",
            params![thread_id, (!read) as i64],
        )?;
        tx.execute(
            "UPDATE threads SET unread = ?2 WHERE id = ?1",
            params![thread_id, (!read) as i64],
        )?;
        if read && status == "unread" {
            Self::leave_unread_tx(&tx, &acc, thread_id)?;
        } else if !read && status != "unread" && status != "review" {
            Self::enter_unread_tx(&tx, &acc, thread_id, &status)?;
        }
        Self::enqueue_tx(
            &tx,
            &acc,
            "read",
            thread_id,
            &serde_json::json!({ "read": read }),
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Moves a thread to Unread, remembering an open status to resume after reading.
    fn enter_unread_tx(
        tx: &Connection,
        account_id: &str,
        thread_id: &str,
        current: &str,
    ) -> Result<()> {
        let prev = RESUMABLE.contains(&current).then_some(current);
        tx.execute(
            "UPDATE threads SET status = 'unread', prev_status = ?2 WHERE id = ?1",
            params![thread_id, prev],
        )?;
        Self::enqueue_label_sync_tx(tx, account_id, thread_id)
    }

    /// Applies the read transition: status from [`status_after_read`], plus the
    /// suggested projects, which reading confirms.
    fn leave_unread_tx(tx: &Connection, account_id: &str, thread_id: &str) -> Result<()> {
        let prev: Option<String> = tx.query_row(
            "SELECT prev_status FROM threads WHERE id = ?1",
            [thread_id],
            |r| r.get(0),
        )?;
        let sugg: Option<(Option<String>, String)> = tx
            .query_row(
                "SELECT status, projects_json FROM suggestions WHERE thread_id = ?1 AND decision IS NULL",
                [thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let habit = Self::sender_habit_tx(tx, thread_id)?;
        let suggested = sugg.as_ref().and_then(|(s, _)| s.clone());
        let next = status_after_read(prev.as_deref(), suggested.as_deref(), habit.as_deref());
        tx.execute(
            "UPDATE threads SET status = ?2, prev_status = NULL WHERE id = ?1",
            params![thread_id, next],
        )?;
        if let Some((_, pj)) = &sugg {
            let wanted: Vec<String> = serde_json::from_str(pj).unwrap_or_default();
            let current = Self::thread_projects_tx(tx, thread_id)?;
            let add: Vec<String> = wanted
                .into_iter()
                .filter(|p| !current.contains(p))
                .collect();
            Self::set_thread_projects_tx(tx, account_id, thread_id, &add, &[])?;
        }
        Self::decide_suggestion_tx(tx, thread_id, "read")?;
        Self::enqueue_label_sync_tx(tx, account_id, thread_id)
    }

    /// The most frequent routed status of the last sender's other threads, for automated
    /// senders only: archived history says nothing about the next mail from a person.
    fn sender_habit_tx(tx: &Connection, thread_id: &str) -> Result<Option<String>> {
        let email: Option<String> = tx
            .query_row(
                "SELECT lower(from_email) FROM messages WHERE thread_id = ?1 AND is_from_me = 0 ORDER BY date DESC LIMIT 1",
                [thread_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(email) = email.filter(|e| is_automated(e)) else {
            return Ok(None);
        };
        Ok(tx
            .query_row(
                "SELECT t.status FROM threads t WHERE t.id != ?1
                   AND t.status IN ('todo','waiting','in_progress','backlog','done','archive')
                   AND t.id IN (SELECT thread_id FROM messages WHERE lower(from_email) = ?2)
                 GROUP BY t.status ORDER BY COUNT(*) DESC LIMIT 1",
                params![thread_id, email],
                |r| r.get(0),
            )
            .optional()?)
    }

    fn decide_suggestion_tx(tx: &Connection, thread_id: &str, decision: &str) -> Result<()> {
        tx.execute(
            "UPDATE suggestions SET decision = ?2, decided = ?3 WHERE thread_id = ?1 AND decision IS NULL",
            params![thread_id, decision, now_ms()],
        )?;
        Ok(())
    }

    fn thread_projects_tx(tx: &Connection, thread_id: &str) -> Result<BTreeSet<String>> {
        let mut st = tx.prepare_cached("SELECT name FROM thread_projects WHERE thread_id = ?1")?;
        Ok(st
            .query_map([thread_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Adds/removes projects locally and queues the matching Gmail label change. The op
    /// carries explicit adds/removes so it never strips a label added on another device.
    fn set_thread_projects_tx(
        tx: &Connection,
        account_id: &str,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        if add.is_empty() && remove.is_empty() {
            return Ok(());
        }
        for p in add {
            tx.execute(
                "INSERT OR IGNORE INTO thread_projects (thread_id, name) VALUES (?1, ?2)",
                params![thread_id, p],
            )?;
        }
        for p in remove {
            tx.execute(
                "DELETE FROM thread_projects WHERE thread_id = ?1 AND name = ?2",
                params![thread_id, p],
            )?;
        }
        let (mut adds, mut removes) = Self::pending_project_delta_tx(tx, thread_id)?;
        for p in add {
            removes.remove(p);
            adds.insert(p.clone());
        }
        for p in remove {
            adds.remove(p);
            removes.insert(p.clone());
        }
        Self::enqueue_tx(
            tx,
            account_id,
            "projects",
            thread_id,
            &serde_json::json!({ "add": adds, "remove": removes }),
        )
    }

    /// Project adds/removes still waiting to be pushed for a thread.
    fn pending_project_delta_tx(
        tx: &Connection,
        thread_id: &str,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
        let payload: Option<String> = tx
            .query_row(
                "SELECT payload FROM ops WHERE kind = 'projects' AND key = ?1",
                [thread_id],
                |r| r.get(0),
            )
            .optional()?;
        let v: serde_json::Value = payload
            .and_then(|p| serde_json::from_str(&p).ok())
            .unwrap_or_default();
        let set = |k: &str| -> BTreeSet<String> {
            v[k].as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok((set("add"), set("remove")))
    }

    /// Local projects follow the thread's Gmail labels, with unpushed edits on top.
    fn sync_projects_tx(tx: &Connection, account_id: &str, thread_id: &str) -> Result<()> {
        let (_, _, names) = Self::thread_remote_user_labels(tx, account_id, thread_id)?;
        let (adds, removes) = Self::pending_project_delta_tx(tx, thread_id)?;
        let wanted: BTreeSet<String> = names
            .into_iter()
            .chain(adds)
            .filter(|n| !removes.contains(n))
            .collect();
        let current = Self::thread_projects_tx(tx, thread_id)?;
        for p in current.difference(&wanted) {
            tx.execute(
                "DELETE FROM thread_projects WHERE thread_id = ?1 AND name = ?2",
                params![thread_id, p],
            )?;
        }
        for p in wanted.difference(&current) {
            tx.execute(
                "INSERT OR IGNORE INTO thread_projects (thread_id, name) VALUES (?1, ?2)",
                params![thread_id, p],
            )?;
        }
        Ok(())
    }

    /// (in inbox, in trash, user label names) as last seen on the server.
    fn thread_remote_user_labels(
        tx: &Connection,
        account_id: &str,
        thread_id: &str,
    ) -> Result<(bool, bool, BTreeSet<String>)> {
        // System labels may be missing from the label cache; their id is their name.
        // `+account_id` keeps SQLite on the (thread_id, label_id) key: through the
        // (account_id, label_id) index it read every label row of the mailbox per message.
        let mut st = tx.prepare_cached(
            "SELECT tl.label_id, l.name FROM thread_labels tl LEFT JOIN labels l ON l.account_id = tl.account_id AND l.label_id = tl.label_id
             WHERE tl.thread_id = ?1 AND +tl.account_id = ?2",
        )?;
        let (mut inbox, mut trash, mut names) = (false, false, BTreeSet::new());
        for row in st.query_map(params![thread_id, account_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })? {
            match row? {
                (id, _) if id == "INBOX" => inbox = true,
                (id, _) if id == "TRASH" => trash = true,
                (id, Some(name)) if is_user_label(&id, &name) => {
                    names.insert(name);
                }
                _ => {}
            }
        }
        Ok((inbox, trash, names))
    }

    pub fn thread_status(&self, thread_id: &str) -> Result<String> {
        let r = self.r.lock().unwrap();
        r.query_row(
            "SELECT status FROM threads WHERE id = ?1",
            [thread_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("thread {thread_id}")))
    }

    /// Whether the server has the thread in Trash.
    pub fn thread_trashed(&self, account_id: &str, thread_id: &str) -> Result<bool> {
        let r = self.r.lock().unwrap();
        Ok(Self::thread_remote_user_labels(&r, account_id, thread_id)?.1)
    }

    /// Desired Gmail labels for a thread, from its current issue fields.
    pub fn thread_desired_labels(&self, thread_id: &str) -> Result<BTreeSet<String>> {
        let r = self.r.lock().unwrap();
        let (status, name, archived, priority): (String, String, i64, i64) = r.query_row(
            "SELECT t.status, s.name, s.archived, t.priority FROM threads t JOIN statuses s ON s.key = t.status WHERE t.id = ?1",
            [thread_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        Ok(desired_labels(&status, archived != 0, &name, priority))
    }

    /// Managed labels (INBOX + Mermoz/*) currently on the thread, by name.
    pub fn thread_current_labels(
        &self,
        account_id: &str,
        thread_id: &str,
    ) -> Result<BTreeSet<String>> {
        let r = self.r.lock().unwrap();
        let (_, names) = Self::thread_remote_labels(&r, account_id, thread_id)?;
        Ok(names
            .into_iter()
            .filter(|n| n == "INBOX" || n.starts_with(LABEL_PREFIX))
            .collect())
    }

    /// Mirrors a successful remote label change locally so later diffs stay right.
    pub fn apply_label_delta(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let rows: Vec<(String, String)> = {
            let mut st = tx.prepare("SELECT id, labels_json FROM messages WHERE thread_id = ?1")?;
            st.query_map([thread_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (mid, lj) in rows {
            let mut v: BTreeSet<String> = serde_json::from_str::<Vec<String>>(&lj)
                .unwrap_or_default()
                .into_iter()
                .collect();
            for a in add {
                v.insert(a.clone());
            }
            for r in remove {
                v.remove(r);
            }
            tx.execute(
                "UPDATE messages SET labels_json = ?2 WHERE id = ?1",
                params![mid, serde_json::to_string(&v)?],
            )?;
        }
        Self::index_thread_labels(&tx, thread_id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn provider_thread_id(&self, thread_id: &str) -> Result<(String, String)> {
        let r = self.r.lock().unwrap();
        r.query_row(
            "SELECT account_id, provider_thread_id FROM threads WHERE id = ?1",
            [thread_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("thread {thread_id}")))
    }

    /// Full-text search, most recent matching threads first. FTS rowids are
    /// date-ordered, so even a word in every message returns in milliseconds.
    /// Threads matching every word of `q`, best first: the words as a phrase, then all
    /// of them in the subject or sender, then anywhere; the most recent first within
    /// each. Ranked among the newest 1,000 matches, so a word in half the mailbox stays
    /// fast. (FTS5's bm25 counts every match of every word first: 20 ms more for a
    /// common word at 500k messages.)
    pub fn search(&self, q: &str, limit: i64, typing: bool) -> Result<Vec<ThreadSummary>> {
        let Some(fq) = fts_query(q, typing) else {
            return Ok(vec![]);
        };
        let mut args: Vec<rusqlite::types::Value> =
            vec![fq.all.into(), limit.into(), 1000i64.into()];
        let mut ctes = String::new();
        let mut rank = String::from("CASE");
        for (n, q) in [fq.phrase, fq.headline].into_iter().enumerate() {
            let Some(q) = q else { continue };
            args.push(q.into());
            // Each tier only looks back to the oldest candidate, so its scan stays short.
            ctes.push_str(&format!(
                ", t{n} AS (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?{}
                   AND rowid >= (SELECT MIN(rowid) FROM hits))",
                args.len()
            ));
            rank.push_str(&format!(" WHEN hits.rowid IN t{n} THEN {n}"));
        }
        rank = if ctes.is_empty() {
            "2".into()
        } else {
            rank + " ELSE 2 END"
        };
        let r = self.r.lock().unwrap();
        let sql = format!(
            "WITH hits AS (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1
                           ORDER BY rowid DESC LIMIT ?3){ctes}
             SELECT {THREAD_COLS} FROM hits JOIN messages m ON m.fts_rowid = hits.rowid
               JOIN threads t ON t.id = m.thread_id{}
             GROUP BY t.id ORDER BY MIN({rank}), t.last_date DESC LIMIT ?2",
            self.visible()
        );
        let mut st = r.prepare_cached(&sql)?;
        let mut rows = st
            .query_map(params_from_iter(args.iter()), thread_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        attach(&r, &mut rows)?;
        Ok(rows)
    }

    // ---- statuses / projects ---------------------------------------------

    pub fn statuses(&self) -> Result<Vec<Status>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached("SELECT key, name, ord, color FROM statuses ORDER BY ord")?;
        Ok(st
            .query_map([], |r| {
                Ok(Status {
                    key: r.get(0)?,
                    name: r.get(1)?,
                    order: r.get(2)?,
                    color: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Saves the user's status order; `keys` must list every status exactly once.
    pub fn reorder_statuses(&self, keys: &[String]) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let known: BTreeSet<String> = {
            let mut st = tx.prepare("SELECT key FROM statuses")?;
            st.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let given: BTreeSet<String> = keys.iter().cloned().collect();
        if given != known || keys.len() != known.len() {
            return Err(Error::InvalidParams(format!(
                "list every status exactly once: {}",
                known.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
        for (i, k) in keys.iter().enumerate() {
            tx.execute(
                "UPDATE statuses SET ord = ?2 WHERE key = ?1",
                params![k, i as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn status_archived(&self, key: &str) -> Result<bool> {
        let r = self.r.lock().unwrap();
        Ok(
            r.query_row("SELECT archived FROM statuses WHERE key = ?1", [key], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
            .unwrap_or(0)
                != 0,
        )
    }

    /// Every project: the user's Gmail labels on any account, plus projects created here
    /// that no thread carries yet.
    pub fn projects(&self) -> Result<Vec<Project>> {
        let r = self.r.lock().unwrap();
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut st = r.prepare_cached(&format!(
            "SELECT label_id, name FROM labels l WHERE 1{}",
            self.visible_on("l")
        ))?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, name) = row?;
            if is_user_label(&id, &name) {
                names.insert(name);
            }
        }
        // A hidden account's folders stay out too, unless the user colored them.
        let filed = if self.visible().is_empty() {
            "SELECT DISTINCT name FROM thread_projects".to_string()
        } else {
            format!(
                "SELECT DISTINCT tp.name FROM thread_projects tp JOIN threads t ON t.id = tp.thread_id WHERE 1{}",
                self.visible()
            )
        };
        let mut st = r.prepare_cached(&format!("SELECT name FROM project_colors UNION {filed}"))?;
        for n in st.query_map([], |r| r.get::<_, String>(0))? {
            names.insert(n?);
        }
        let mut colors: std::collections::HashMap<String, String> = Default::default();
        let mut st = r.prepare_cached("SELECT name, color FROM project_colors")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (n, c) = row?;
            colors.insert(n, c);
        }
        let mut out: Vec<Project> = names
            .into_iter()
            .map(|n| Project {
                id: n.clone(),
                color: colors
                    .get(&n)
                    .cloned()
                    .unwrap_or_else(|| default_color(&n).to_string()),
                name: n,
            })
            .collect();
        out.sort_by_key(|p| p.name.to_lowercase());
        Ok(out)
    }

    /// Renames and/or recolors a project. A rename carries the sub-projects
    /// ("Old/x" → "New/x") and queues the Gmail label renames on every account.
    pub fn update_project(
        &self,
        id: &str,
        name: Option<&str>,
        color: Option<&str>,
    ) -> Result<Project> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let mut current = id.to_string();
        if let Some(n) = name.map(str::trim).filter(|n| *n != id) {
            validate_project_name(n)?;
            let taken = tx
                .query_row(
                    "SELECT 1 FROM thread_projects WHERE name = ?1 UNION SELECT 1 FROM project_colors WHERE name = ?1
                     UNION SELECT 1 FROM labels WHERE name = ?1",
                    [n],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if taken {
                return Err(Error::InvalidParams(format!(
                    "a project named '{n}' already exists"
                )));
            }
            let accounts: Vec<String> = {
                let mut st = tx.prepare("SELECT id FROM accounts")?;
                st.query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for (old, new) in Self::project_family_tx(&tx, id)?.into_iter().map(|o| {
                let n2 = format!("{n}{}", &o[id.len()..]);
                (o, n2)
            }) {
                tx.execute(
                    "UPDATE thread_projects SET name = ?2 WHERE name = ?1",
                    params![old, new],
                )?;
                tx.execute(
                    "UPDATE project_colors SET name = ?2 WHERE name = ?1",
                    params![old, new],
                )?;
                tx.execute(
                    "UPDATE labels SET name = ?2 WHERE name = ?1",
                    params![old, new],
                )?;
                for acc in &accounts {
                    Self::enqueue_tx(
                        &tx,
                        acc,
                        "label_rename",
                        &format!("{acc}:{new}"),
                        &serde_json::json!({ "from": old, "to": new }),
                    )?;
                }
            }
            current = n.to_string();
        }
        if let Some(c) = color {
            tx.execute(
                "INSERT INTO project_colors (name, color) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET color = excluded.color",
                params![current, c],
            )?;
        }
        let color: String = tx
            .query_row(
                "SELECT color FROM project_colors WHERE name = ?1",
                [&current],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| default_color(&current).to_string());
        tx.commit()?;
        Ok(Project {
            id: current.clone(),
            name: current,
            color,
        })
    }

    /// A project and its sub-projects, by name.
    fn project_family_tx(tx: &Connection, name: &str) -> Result<Vec<String>> {
        let child = format!("{name}/%");
        let mut st = tx.prepare(
            "SELECT name FROM labels WHERE name = ?1 OR name LIKE ?2
             UNION SELECT name FROM thread_projects WHERE name = ?1 OR name LIKE ?2
             UNION SELECT name FROM project_colors WHERE name = ?1 OR name LIKE ?2",
        )?;
        let v = st
            .query_map(params![name, child], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(v)
    }

    /// Deletes a project: its Gmail label is deleted on every account (the mail stays,
    /// only the label goes). Returns how many threads carried it.
    pub fn delete_project(&self, id: &str) -> Result<usize> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let n = tx.execute("DELETE FROM thread_projects WHERE name = ?1", [id])?;
        let known = tx.execute("DELETE FROM project_colors WHERE name = ?1", [id])?;
        let accounts: Vec<String> = {
            let mut st = tx.prepare("SELECT DISTINCT account_id FROM labels WHERE name = ?1")?;
            st.query_map([id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        if n == 0 && known == 0 && accounts.is_empty() {
            return Err(Error::NotFound(format!("project {id}")));
        }
        for acc in &accounts {
            Self::enqueue_tx(
                &tx,
                acc,
                "label_delete",
                &format!("{acc}:{id}"),
                &serde_json::json!({ "name": id }),
            )?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Creates a project; its Gmail label is created when a thread first gets it.
    pub fn create_project(&self, name: &str, color: Option<&str>) -> Result<Project> {
        let name = name.trim();
        validate_project_name(name)?;
        let color = color.unwrap_or_else(|| default_color(name)).to_string();
        let w = self.w.lock().unwrap();
        w.execute(
            "INSERT INTO project_colors (name, color) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET color = excluded.color",
            params![name, color],
        )?;
        Ok(Project {
            id: name.into(),
            name: name.into(),
            color,
        })
    }

    // ---- routing suggestions -------------------------------------------

    /// Stores the router's prediction for a thread, replacing an older one.
    pub fn save_suggestion(
        &self,
        thread_id: &str,
        sg: &Suggestion,
        model: &str,
        for_date: i64,
    ) -> Result<()> {
        let w = self.w.lock().unwrap();
        w.execute(
            UPSERT_SUGGESTION,
            params![
                thread_id,
                sg.status,
                sg.status_confidence,
                serde_json::to_string(&sg.projects)?,
                sg.project_confidence,
                sg.priority,
                sg.automated,
                model,
                for_date,
                now_ms(),
                None::<String>,
            ],
        )?;
        Ok(())
    }

    /// Unread threads without a prediction for their latest message, newest first.
    pub fn threads_to_route(&self, limit: i64) -> Result<Vec<(String, i64)>> {
        let r = self.bg.lock().unwrap();
        let mut st = r.prepare_cached(&format!(
            "SELECT t.id, t.last_date FROM threads t LEFT JOIN suggestions s ON s.thread_id = t.id
             WHERE t.status = 'unread' AND (s.thread_id IS NULL OR s.for_date < t.last_date)
               AND {}
             ORDER BY t.last_date DESC LIMIT ?1",
            visibility::NOT_DEMO_T
        ))?;
        Ok(st
            .query_map([limit], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// What the router tells Jev about a thread: the last three messages without quoted
    /// history, and where earlier threads with the same people were filed.
    pub fn route_input(&self, thread_id: &str) -> Result<crate::jev::RouteInput> {
        Ok(self.route_input_over(thread_id, false)?.0)
    }

    /// The Feed's Jev input for a conversation's newest thread: its messages are the
    /// latest of the whole conversation, so a date written in an earlier thread counts.
    /// Comes with the date each of those messages was written (Unix ms).
    pub fn feed_input(&self, thread_id: &str) -> Result<(crate::jev::RouteInput, Vec<i64>)> {
        self.route_input_over(thread_id, true)
    }

    fn route_input_over(
        &self,
        thread_id: &str,
        conversation: bool,
    ) -> Result<(crate::jev::RouteInput, Vec<i64>)> {
        let r = self.bg.lock().unwrap();
        let scope = if conversation {
            "thread_id IN (SELECT id FROM threads WHERE conversation_id = \
               (SELECT COALESCE(conversation_id, id) FROM threads WHERE id = ?1))"
        } else {
            "thread_id = ?1"
        };
        // (from name, from email, mine, date, subject, body), oldest first.
        type Row = (String, String, bool, i64, String, String);
        let rows: Vec<Row> = {
            let mut st = r.prepare_cached(&format!(
                "SELECT from_name, from_email, is_from_me, date, subject, body_text \
                 FROM messages WHERE {scope} ORDER BY date"
            ))?;
            st.query_map([thread_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)? != 0,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        // The subject is the thread's own, never an earlier thread's.
        let subject: String = r
            .prepare_cached(
                "SELECT subject FROM messages WHERE thread_id = ?1 AND subject != '' ORDER BY date LIMIT 1",
            )?
            .query_row([thread_id], |r| r.get(0))
            .optional()?
            .unwrap_or_default();
        let now = now_ms();
        let recent = rows[rows.len().saturating_sub(3)..]
            .iter()
            .map(|(name, email, mine, date, _, body)| {
                let (new, _) = crate::quote::readable(body);
                crate::jev::RouteMessage {
                    from: if *mine {
                        "me".into()
                    } else if name.is_empty() {
                        email.clone()
                    } else {
                        format!("{name} <{email}>")
                    },
                    days_ago: crate::jev::days_ago(*date, now),
                    text: new
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(700)
                        .collect(),
                }
            })
            .collect();
        let mut st = r.prepare_cached(
            // Only the indexed sender column is searched for peers: matching recipients of
            // sent mail too costs 500× more for the same accuracy on a real mailbox.
            "WITH people AS (
               SELECT lower(from_email) e FROM messages WHERE thread_id = ?1 AND is_from_me = 0
               UNION SELECT lower(json_extract(j.value, '$.email')) FROM messages m, json_each(m.to_json) j
                 WHERE m.thread_id = ?1 AND m.is_from_me = 1),
             peers AS (
               SELECT DISTINCT m.thread_id FROM people p JOIN messages m ON lower(m.from_email) = p.e WHERE m.thread_id != ?1)
             SELECT tp.name, COUNT(*) n FROM thread_projects tp JOIN peers p ON p.thread_id = tp.thread_id
             GROUP BY tp.name ORDER BY n DESC LIMIT 5",
        )?;
        let history = st
            .query_map([thread_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        // The same people's threads, filed or not: a folder only means something
        // against how much of their mail the user never filed.
        let (peer_threads, filed_peers): (i64, i64) = r.query_row(
            "WITH people AS (
               SELECT lower(from_email) e FROM messages WHERE thread_id = ?1 AND is_from_me = 0
               UNION SELECT lower(json_extract(j.value, '$.email')) FROM messages m, json_each(m.to_json) j
                 WHERE m.thread_id = ?1 AND m.is_from_me = 1),
             peers AS (
               SELECT DISTINCT m.thread_id FROM people p JOIN messages m ON lower(m.from_email) = p.e WHERE m.thread_id != ?1)
             SELECT COUNT(*), COALESCE(SUM(EXISTS (SELECT 1 FROM thread_projects tp WHERE tp.thread_id = p.thread_id)), 0) FROM peers p",
            [thread_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let sender = thread_sender_tx(&r, thread_id)?;
        let mut st = r.prepare_cached(
            "SELECT verdict, status, projects_json, COUNT(*) FROM sort_feedback WHERE sender = ?1 GROUP BY 1, 2, 3",
        )?;
        let earlier_answers = st
            .query_map([&sender], |r| {
                let (verdict, status, pj): (String, Option<String>, String) =
                    (r.get(0)?, r.get(1)?, r.get(2)?);
                let projects: Vec<String> = serde_json::from_str(&pj).unwrap_or_default();
                let mut to: Vec<String> = status.into_iter().collect();
                to.extend(projects);
                Ok((format!("{verdict}: {}", to.join(" · ")), r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let age = |r: Option<&Row>| r.map_or(0, |r| crate::jev::days_ago(r.3, now));
        let written = rows[rows.len().saturating_sub(3)..]
            .iter()
            .map(|r| r.3)
            .collect();
        let input = crate::jev::RouteInput {
            today: chrono::Local::now().format("%Y-%m-%d").to_string(),
            last_activity_days_ago: age(rows.last()),
            thread_started_days_ago: age(rows.first()),
            subject,
            messages_in_thread: rows.len() as i64,
            last_message_from_me: rows.last().map(|r| r.2).unwrap_or(false),
            recent_messages: recent,
            sender_history: history,
            peer_threads,
            unfiled_peers: peer_threads - filed_peers,
            earlier_answers,
        };
        Ok((input, written))
    }

    /// Every project with a description built from the mail filed in it: its four most
    /// frequent correspondents.
    pub fn project_descriptions(&self) -> Result<Vec<(String, String)>> {
        let names: Vec<String> = self.projects()?.into_iter().map(|p| p.name).collect();
        let r = self.r.lock().unwrap();
        let mut people: std::collections::HashMap<String, Vec<String>> = Default::default();
        let mut st = r.prepare_cached(
            "SELECT tp.name, MAX(m.from_name), lower(m.from_email), COUNT(DISTINCT m.thread_id) n
             FROM thread_projects tp JOIN messages m ON m.thread_id = tp.thread_id
             WHERE m.is_from_me = 0 GROUP BY tp.name, lower(m.from_email) ORDER BY tp.name, n DESC",
        )?;
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (n, name, email) = row?;
            let v = people.entry(n).or_default();
            if v.len() < 4 {
                v.push(if name.is_empty() { email } else { name });
            }
        }
        Ok(names
            .into_iter()
            .map(|n| {
                let d = crate::jev::describe(&n, people.get(&n).map(Vec::as_slice).unwrap_or(&[]));
                (n, d)
            })
            .collect())
    }

    /// Turns down pending suggestions: the threads stay where they are and are not
    /// suggested again until new mail arrives. Returns how many were pending.
    pub fn reject_suggestions(&self, ids: &[String]) -> Result<usize> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let mut n = 0;
        for id in ids {
            let target: Option<(Option<String>, String)> = tx
                .query_row(
                    "SELECT status, projects_json FROM suggestions WHERE thread_id = ?1 AND decision IS NULL",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((status, pj)) = target else {
                continue;
            };
            n += tx.execute(
                "UPDATE suggestions SET decision = 'rejected', decided = ?2 WHERE thread_id = ?1 AND decision IS NULL",
                params![id, now_ms()],
            )?;
            let target = SuggestionGroup {
                status,
                projects: serde_json::from_str(&pj).unwrap_or_default(),
                count: 0,
            };
            let sender = thread_sender_tx(&tx, id)?;
            record_feedback_tx(&tx, id, &sender, &target, "no")?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Undoes [`Self::reject_suggestions`]: the suggestions are pending again.
    pub fn reopen_suggestions(&self, ids: &[String]) -> Result<usize> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let mut n = 0;
        for id in ids {
            n += tx.execute(
                "UPDATE suggestions SET decision = NULL, decided = NULL WHERE thread_id = ?1 AND decision = 'rejected'",
                [id],
            )?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Accepts pending suggestions as they stand: status, projects and priority apply,
    /// and the threads count as read.
    pub fn accept_suggestions(&self, ids: &[String]) -> Result<usize> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let mut n = 0;
        for id in ids {
            let row: Option<(String, Option<String>, String, Option<i64>)> = tx
                .query_row(
                    &format!("SELECT t.account_id, s.status, s.projects_json, s.priority FROM suggestions s JOIN threads t ON t.id = s.thread_id
                     WHERE s.thread_id = ?1 AND {PENDING_SUGGESTION}"),
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let Some((acc, status, pj, priority)) = row else {
                continue;
            };
            let wanted: Vec<String> = serde_json::from_str(&pj).unwrap_or_default();
            let current = Self::thread_projects_tx(&tx, id)?;
            let add: Vec<String> = wanted
                .into_iter()
                .filter(|p| !current.contains(p))
                .collect();
            Self::set_thread_projects_tx(&tx, &acc, id, &add, &[])?;
            if let Some(st) = &status {
                tx.execute(
                    "UPDATE threads SET status = ?2, prev_status = NULL WHERE id = ?1",
                    params![id, st],
                )?;
            }
            if let Some(p) = priority {
                tx.execute(
                    "UPDATE threads SET priority = ?2 WHERE id = ?1",
                    params![id, p],
                )?;
            }
            let unread: bool =
                tx.query_row("SELECT unread FROM threads WHERE id = ?1", [id], |r| {
                    Ok(r.get::<_, i64>(0)? != 0)
                })?;
            if unread {
                tx.execute("UPDATE messages SET unread = 0 WHERE thread_id = ?1", [id])?;
                tx.execute("UPDATE threads SET unread = 0 WHERE id = ?1", [id])?;
                Self::enqueue_tx(&tx, &acc, "read", id, &serde_json::json!({ "read": true }))?;
            }
            Self::decide_suggestion_tx(&tx, id, "accepted")?;
            let target = SuggestionGroup {
                status: status.clone(),
                projects: serde_json::from_str(&pj).unwrap_or_default(),
                count: 0,
            };
            let sender = thread_sender_tx(&tx, id)?;
            record_feedback_tx(&tx, id, &sender, &target, "yes")?;
            Self::enqueue_label_sync_tx(&tx, &acc, id)?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Which of `ids` may get a suggestion now: read threads in open columns with no
    /// decided verdict since their last message, newest first. Pending suggestions made
    /// for the thread as it is now are `ready`; the rest need the AI.
    pub fn suggest_candidates(&self, ids: &[String]) -> Result<Vec<SortCandidate>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let (cols, mut args) = open_columns();
        args.extend(ids.iter().map(|e| e.clone().into()));
        let marks = vec!["?"; ids.len()].join(",");
        let r = self.bg.lock().unwrap();
        let mut st = r.prepare(&format!(
            "SELECT t.id, t.last_date, s.thread_id IS NOT NULL AND s.decision IS NULL AND s.for_date >= t.last_date
                    AND s.for_status IS t.status
             FROM threads t LEFT JOIN suggestions s ON s.thread_id = t.id
             WHERE t.status IN ({cols}) AND {SORT_DUE} AND t.id IN ({marks}) AND {}
             ORDER BY t.last_date DESC",
            visibility::NOT_DEMO_T
        ))?;
        Ok(st
            .query_map(params_from_iter(args.iter()), |r| {
                Ok(SortCandidate {
                    id: r.get(0)?,
                    last_date: r.get(1)?,
                    ready: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Who a thread is with, for learning: the last sender who is not the user, else
    /// the first person the user wrote to.
    pub fn thread_sender(&self, thread_id: &str) -> Result<String> {
        let r = self.r.lock().unwrap();
        thread_sender_tx(&r, thread_id)
    }

    /// The destination this sender's mail goes to once the user said yes to it
    /// [`TRUST_AFTER`] times, as most of their yes answers, and never no.
    pub fn sender_rule(&self, sender: &str) -> Result<Option<SuggestionGroup>> {
        let r = self.r.lock().unwrap();
        let total: i64 = r.query_row(
            "SELECT COUNT(*) FROM sort_feedback WHERE sender = ?1 AND verdict = 'yes'",
            [sender],
            |r| r.get(0),
        )?;
        let top: Option<(Option<String>, String, i64)> = r
            .query_row(
                "SELECT f.status, f.projects_json, COUNT(*) n FROM sort_feedback f
                 WHERE f.sender = ?1 AND f.verdict = 'yes' AND NOT EXISTS (
                   SELECT 1 FROM sort_feedback g WHERE g.sender = f.sender AND g.verdict = 'no'
                     AND g.status IS f.status AND g.projects_json = f.projects_json)
                 GROUP BY 1, 2 ORDER BY n DESC LIMIT 1",
                [sender],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(top
            .filter(|(_, _, n)| *n >= TRUST_AFTER && n * 2 > total)
            .map(|(status, pj, count)| SuggestionGroup {
                status,
                projects: serde_json::from_str(&pj).unwrap_or_default(),
                count,
            }))
    }

    /// Whether the user already said no to sending this sender's mail there.
    pub fn refused_before(&self, sender: &str, target: &SuggestionGroup) -> Result<bool> {
        let r = self.r.lock().unwrap();
        refused_tx(&r, sender, target)
    }

    /// What the answers taught, newest first: senders with a rule, and refused moves.
    pub fn learned(&self, limit: i64) -> Result<Vec<Learned>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare(
            "SELECT f.sender, f.status, f.projects_json, f.verdict, COUNT(*) n, MAX(f.created) last,
                    (SELECT MAX(m.from_name) FROM messages m WHERE lower(m.from_email) = f.sender)
             FROM sort_feedback f GROUP BY 1, 2, 3, 4 ORDER BY last DESC",
        )?;
        let rows: Vec<FeedbackRow> = st
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(6)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(st);
        drop(r);
        let mut out = vec![];
        for (sender, status, pj, verdict, n, name) in rows {
            let target = SuggestionGroup {
                status: status.clone(),
                projects: serde_json::from_str(&pj).unwrap_or_default(),
                count: n,
            };
            let kind = match verdict.as_str() {
                "no" => "avoid",
                _ if self
                    .sender_rule(&sender)?
                    .is_some_and(|g| g.status == status && g.projects == target.projects) =>
                {
                    "rule"
                }
                _ => continue,
            };
            out.push(Learned {
                name: name
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| sender.clone()),
                sender,
                status,
                projects: target.projects,
                kind: kind.into(),
                count: n,
            });
            if out.len() as i64 >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Forgets a learned item: a rule's yes answers, or an avoided move's no answers,
    /// for that sender and destination. Returns how many answers were dropped.
    pub fn forget_learned(&self, item: &Learned) -> Result<usize> {
        let verdict = if item.kind == "avoid" { "no" } else { "yes" };
        let w = self.w.lock().unwrap();
        Ok(w.execute(
            "DELETE FROM sort_feedback WHERE sender = ?1 AND status IS ?2 AND projects_json = ?3 AND verdict = ?4",
            params![
                item.sender,
                item.status,
                serde_json::to_string(&item.projects)?,
                verdict
            ],
        )?)
    }

    /// Records a yes/no answer to a card and applies it; returns how to undo it.
    pub fn answer_sort(&self, thread_id: &str, answer: &SortAnswer) -> Result<SortUndo> {
        let siblings = self.open_siblings(thread_id)?;
        let undo = SortUndo {
            thread: ThreadState::from(&self.thread(thread_id)?),
            since: now_ms(),
            siblings: siblings
                .iter()
                .map(|t| Ok(ThreadState::from(&self.thread(t)?)))
                .collect::<Result<_>>()?,
        };
        match answer {
            SortAnswer::Yes => {
                self.accept_suggestions(&[thread_id.to_string()])?;
                // One status per conversation: its other open threads go where it went.
                let status = self.thread(thread_id)?.status;
                if status != undo.thread.status && !siblings.is_empty() {
                    let patch = ThreadPatch {
                        status: Some(status),
                        ..Default::default()
                    };
                    self.patch_threads(&siblings, &patch, false)?;
                }
            }
            SortAnswer::No => {
                self.reject_suggestions(&[thread_id.to_string()])?;
            }
            SortAnswer::Change { status, projects } => {
                self.reject_suggestions(&[thread_id.to_string()])?;
                self.update_threads(
                    &[thread_id.to_string()],
                    &ThreadPatch {
                        status: status.clone(),
                        add_projects: projects.clone(),
                        ..Default::default()
                    },
                )?;
                let sender = self.thread_sender(thread_id)?;
                let target = SuggestionGroup {
                    status: status.clone(),
                    projects: projects.clone(),
                    count: 0,
                };
                let w = self.w.lock().unwrap();
                record_feedback_tx(&w, thread_id, &sender, &target, "yes")?;
            }
        }
        Ok(undo)
    }

    /// Takes an answer back: the thread returns as it was, the answer is forgotten and
    /// its card is pending again.
    pub fn undo_sort(&self, undo: &SortUndo) -> Result<()> {
        let mut before = vec![undo.thread.clone()];
        before.extend(undo.siblings.iter().cloned());
        self.restore_threads(&before)?;
        let w = self.w.lock().unwrap();
        w.execute(
            "DELETE FROM sort_feedback WHERE thread_id = ?1 AND created >= ?2",
            params![undo.thread.id, undo.since],
        )?;
        w.execute(
            "UPDATE suggestions SET decision = NULL, decided = NULL WHERE thread_id = ?1",
            [&undo.thread.id],
        )?;
        Ok(())
    }

    /// Closes a prepared card whose destination the user refused for this sender since
    /// it was computed; returns whether it was closed.
    pub fn close_if_refused(&self, thread_id: &str) -> Result<bool> {
        let w = self.w.lock().unwrap();
        let pending: Option<(Option<String>, String)> = w
            .query_row(
                "SELECT status, projects_json FROM suggestions WHERE thread_id = ?1 AND decision IS NULL",
                [thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((status, pj)) = pending else {
            return Ok(false);
        };
        let target = SuggestionGroup {
            status,
            projects: serde_json::from_str(&pj).unwrap_or_default(),
            count: 0,
        };
        let sender = thread_sender_tx(&w, thread_id)?;
        if !refused_tx(&w, &sender, &target)? {
            return Ok(false);
        }
        w.execute(
            "UPDATE suggestions SET decision = 'refused_before', decided = ?2 WHERE thread_id = ?1 AND decision IS NULL",
            params![thread_id, now_ms()],
        )?;
        Ok(true)
    }

    /// What made a thread's stored suggestion: "learned" (a sender rule) or the AI model.
    pub fn suggestion_model(&self, thread_id: &str) -> Result<Option<String>> {
        let r = self.r.lock().unwrap();
        Ok(r.query_row(
            "SELECT model FROM suggestions WHERE thread_id = ?1",
            [thread_id],
            |r| r.get(0),
        )
        .optional()?)
    }

    /// Clears machine suggestions nobody reviewed on read threads (see
    /// [`MACHINE_VERDICTS`]) and any whole-mailbox pass still recorded.
    pub fn clear_machine_suggestions(&self) -> Result<usize> {
        let w = self.w.lock().unwrap();
        let n = w.execute(MACHINE_VERDICTS, [])?;
        w.execute("DELETE FROM settings WHERE key = 'resort'", [])?;
        Ok(n)
    }

    /// Applies [`crate::jev::guard_stale`] to pending suggestions as threads age: on a
    /// thread silent past the threshold, a status other than Done/Archive and the
    /// priority are cleared; a suggestion left with nothing to show is closed as
    /// "unchanged" so the thread is not asked again. Decided suggestions stay as they
    /// are. Returns how many suggestions changed.
    pub fn drop_stale_suggestions(&self, now: i64) -> Result<usize> {
        let allowed = crate::jev::STALE_STATUSES
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(",");
        let keep = format!("status IN ({allowed})");
        let w = self.w.lock().unwrap();
        Ok(w.execute(
            &format!(
                "UPDATE suggestions SET
                   status = CASE WHEN {keep} THEN status END,
                   status_confidence = CASE WHEN {keep} THEN status_confidence END,
                   priority = NULL,
                   decision = CASE WHEN {keep} OR projects_json != '[]' THEN NULL ELSE 'unchanged' END,
                   decided = CASE WHEN {keep} OR projects_json != '[]' THEN NULL ELSE ?1 END
                 WHERE decision IS NULL
                   AND ((status IS NOT NULL AND NOT {keep}) OR priority IS NOT NULL)
                   AND EXISTS (SELECT 1 FROM threads t WHERE t.id = suggestions.thread_id
                                 AND (?1 - t.last_date) / {} > ?2)",
                crate::jev::DAY_MS
            ),
            params![now, crate::jev::STALE_AFTER_DAYS],
        )?)
    }

    /// Stores a prediction for an already-read thread, reduced to what would
    /// change (see [`crate::jev::column_suggestion`]) against the thread as it is now.
    /// "No change", a destination the user refused for this sender before, or a thread
    /// that left the open columns meanwhile, is recorded as decided so it is not asked
    /// again. Returns whether a suggestion is now shown.
    pub fn save_column_suggestion(
        &self,
        thread_id: &str,
        sg: &Suggestion,
        model: &str,
        for_date: i64,
    ) -> Result<bool> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM threads WHERE id = ?1",
                [thread_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(status) = status else {
            return Ok(false);
        };
        let current: Vec<String> = Self::thread_projects_tx(&tx, thread_id)?
            .into_iter()
            .collect();
        let sender = thread_sender_tx(&tx, thread_id)?;
        let (kept, decision) = match crate::jev::column_suggestion(sg, &status, &current) {
            Some(k) if refused_tx(&tx, &sender, &SuggestionGroup::of(&k))? => {
                (k, Some("refused_before"))
            }
            Some(k) if RESUMABLE.contains(&status.as_str()) => (k, None),
            Some(k) => (k, Some("moved")),
            None => (Suggestion::default(), Some("unchanged")),
        };
        tx.execute(
            UPSERT_SUGGESTION,
            params![
                thread_id,
                kept.status,
                kept.status_confidence,
                serde_json::to_string(&kept.projects)?,
                kept.project_confidence,
                kept.priority,
                kept.automated,
                model,
                for_date,
                now_ms(),
                decision,
            ],
        )?;
        tx.commit()?;
        Ok(decision.is_none())
    }

    /// Pending suggestions of one column, grouped by where they would send threads
    /// (largest group first).
    pub fn column_suggestions(
        &self,
        status: &str,
        scope: &ColumnScope,
    ) -> Result<ColumnSuggestions> {
        let (scope_sql, scope_args) = scope.clause();
        let r = self.r.lock().unwrap();
        let mut args: Vec<rusqlite::types::Value> = vec![status.to_string().into()];
        args.extend(scope_args);
        let mut st = r.prepare_cached(&format!(
            "SELECT s.status, s.projects_json, COUNT(*) FROM threads t JOIN suggestions s ON s.thread_id = t.id
             WHERE t.status = ?1 AND {PENDING_SUGGESTION} AND (s.status IS NOT NULL OR s.projects_json != '[]'){scope_sql}{}
             GROUP BY s.status, s.projects_json ORDER BY COUNT(*) DESC, s.status",
            self.visible()
        ))?;
        let groups: Vec<SuggestionGroup> = st
            .query_map(params_from_iter(args.iter()), |r| {
                let pj: String = r.get(1)?;
                Ok(SuggestionGroup {
                    status: r.get(0)?,
                    projects: serde_json::from_str(&pj).unwrap_or_default(),
                    count: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ColumnSuggestions {
            status: status.to_string(),
            pending: groups.iter().map(|g| g.count).sum(),
            groups,
        })
    }

    /// Accepts every pending suggestion of a column, or only those sending threads to
    /// `target`. Returns the threads as they were before, for undo.
    pub fn accept_column(
        &self,
        status: &str,
        scope: &ColumnScope,
        target: Option<&SuggestionGroup>,
    ) -> Result<Vec<ThreadSummary>> {
        let (scope_sql, scope_args) = scope.clause();
        let ids: Vec<String> = {
            let r = self.r.lock().unwrap();
            let mut args: Vec<rusqlite::types::Value> = vec![status.to_string().into()];
            args.extend(scope_args);
            let target_sql = match target {
                Some(g) => {
                    args.push(g.status.clone().into());
                    args.push(serde_json::to_string(&g.projects)?.into());
                    " AND s.status IS ? AND s.projects_json = ?"
                }
                None => " AND (s.status IS NOT NULL OR s.projects_json != '[]')",
            };
            let mut st = r.prepare(&format!(
                "SELECT t.id FROM threads t JOIN suggestions s ON s.thread_id = t.id
                 WHERE t.status = ?1 AND {PENDING_SUGGESTION}{scope_sql}{target_sql}{}",
                self.visible()
            ))?;
            st.query_map(params_from_iter(args.iter()), |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let before = ids
            .iter()
            .map(|id| self.thread(id))
            .collect::<Result<Vec<_>>>()?;
        self.accept_suggestions(&ids)?;
        Ok(before)
    }

    /// Puts threads back exactly as in the snapshots: read state, status, priority,
    /// projects and follow-up date (the undo of a group accept).
    pub fn restore_threads(&self, before: &[ThreadState]) -> Result<usize> {
        type Shape = (String, i64, Vec<String>, Option<String>);
        let mut same: std::collections::BTreeMap<Shape, Vec<String>> = Default::default();
        for t in before {
            let now = self.thread(&t.id)?;
            if now.unread != t.unread {
                self.mark_read(&t.id, !t.unread)?;
            }
            let mut projects = t.projects.clone();
            projects.sort();
            same.entry((t.status.clone(), t.priority, projects, t.due.clone()))
                .or_default()
                .push(t.id.clone());
        }
        let mut n = 0;
        // Each thread gets its own state back: siblings never follow a restore.
        for ((status, priority, projects, due), ids) in same {
            n += self.patch_threads(
                &ids,
                &ThreadPatch {
                    status: Some(status),
                    priority: Some(priority),
                    projects: Some(projects),
                    due: Some(due),
                    ..Default::default()
                },
                false,
            )?;
        }
        Ok(n)
    }

    /// Background router calls made today (UTC), for the daily cap.
    pub fn background_calls_today(&self) -> Result<i64> {
        Ok(self
            .setting(&background_calls_key())?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// Adds to today's background call count; returns the new total.
    pub fn add_background_calls(&self, n: i64) -> Result<i64> {
        let w = self.w.lock().unwrap();
        Ok(w.query_row(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = CAST(value AS INTEGER) + ?2 RETURNING CAST(value AS INTEGER)",
            params![background_calls_key(), n],
            |r| r.get(0),
        )?)
    }

    /// The System One provider the user picked.
    pub fn jev_provider(&self) -> Result<crate::jev::Provider> {
        Ok(crate::jev::provider_from_setting(
            self.setting(PROVIDER_SETTING)?.as_deref(),
        ))
    }

    pub fn set_jev_provider(&self, p: crate::jev::Provider) -> Result<()> {
        self.set_setting(PROVIDER_SETTING, p.id())
    }

    /// Calls made to `p` today (UTC), for Settings.
    pub fn provider_calls_today(&self, p: crate::jev::Provider) -> Result<i64> {
        Ok(self
            .setting(&provider_calls_key(p))?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    pub fn add_provider_calls(&self, p: crate::jev::Provider, n: i64) -> Result<()> {
        self.w.lock().unwrap().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = CAST(value AS INTEGER) + ?2",
            params![provider_calls_key(p), n],
        )?;
        Ok(())
    }

    /// The daily cap on background router calls.
    pub fn background_daily_cap(&self) -> Result<i64> {
        Ok(self
            .setting("jev_daily_cap")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(crate::jev::DEFAULT_DAILY_CAP))
    }

    /// (message id, provider id, sender) of a thread's messages whose sender is not
    /// verified yet (mail imported before verification existed).
    pub fn unverified_senders(&self, thread_id: &str) -> Result<Vec<(String, String, String)>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT id, provider_id, from_email FROM messages WHERE thread_id = ?1 AND auth IS NULL AND is_from_me = 0",
        )?;
        Ok(st
            .query_map([thread_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// A thread's received messages stored before their `Reply-To` and kind (bounce,
    /// auto-reply) were kept, newest first: the ones a reply reads.
    pub fn missing_headers(&self, thread_id: &str) -> Result<Vec<MissingHeaders>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT id, provider_id, from_email, subject FROM messages
             WHERE thread_id = ?1 AND is_from_me = 0 AND (reply_to_json IS NULL OR auto_kind IS NULL)
             ORDER BY date DESC LIMIT 20",
        )?;
        Ok(st
            .query_map([thread_id], |r| {
                Ok(MissingHeaders {
                    id: r.get(0)?,
                    provider_id: r.get(1)?,
                    from: r.get(2)?,
                    subject: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Records what [`Self::missing_headers`] lacked.
    pub fn set_headers(
        &self,
        message_id: &str,
        reply_to: &[Address],
        kind: crate::dsn::AutoKind,
        dsn: Option<&crate::dsn::Dsn>,
    ) -> Result<()> {
        let w = self.w.lock().unwrap();
        w.execute(
            "UPDATE messages SET reply_to_json = ?2, auto_kind = ?3, dsn_json = ?4 WHERE id = ?1",
            params![
                message_id,
                serde_json::to_string(reply_to)?,
                kind.as_str(),
                dsn.map(serde_json::to_string).transpose()?
            ],
        )?;
        Ok(())
    }

    pub fn set_sender_auth(&self, message_id: &str, verdict: &str) -> Result<()> {
        let w = self.w.lock().unwrap();
        w.execute(
            "UPDATE messages SET auth = ?2 WHERE id = ?1",
            params![message_id, verdict],
        )?;
        Ok(())
    }

    /// Forgets a label deleted on the server.
    pub fn remove_label(&self, account_id: &str, label_id: &str) -> Result<()> {
        let w = self.w.lock().unwrap();
        w.execute(
            "DELETE FROM thread_labels WHERE account_id = ?1 AND label_id = ?2",
            params![account_id, label_id],
        )?;
        w.execute(
            "DELETE FROM labels WHERE account_id = ?1 AND label_id = ?2",
            params![account_id, label_id],
        )?;
        Ok(())
    }

    // ---- drafts --------------------------------------------------------

    pub fn create_draft(&self, d: &Draft) -> Result<Draft> {
        let w = self.w.lock().unwrap();
        w.execute(
            &format!("INSERT INTO drafts ({DRAFT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
            params![
                d.id,
                d.account_id,
                d.thread_id,
                serde_json::to_string(&d.to)?,
                serde_json::to_string(&d.cc)?,
                d.subject,
                d.body,
                d.state,
                d.origin,
                d.created,
                d.updated,
                d.reply_to_message_id
            ],
        )?;
        drop(w);
        self.draft(&d.id)
    }

    pub fn draft(&self, id: &str) -> Result<Draft> {
        let r = self.r.lock().unwrap();
        let mut d = r
            .query_row(
                &format!("SELECT {DRAFT_COLS} FROM drafts WHERE id = ?1"),
                [id],
                draft_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("draft {id}")))?;
        d.attachments = Self::draft_attachments_on(&r, id)?;
        Ok(d)
    }

    fn draft_attachments_on(conn: &Connection, draft_id: &str) -> Result<Vec<DraftAttachment>> {
        let mut st = conn.prepare_cached(
            "SELECT id, kind, filename, mime, size, path, url, inline FROM draft_attachments WHERE draft_id = ?1 ORDER BY ord",
        )?;
        Ok(st
            .query_map([draft_id], draft_attachment_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    fn draft_dir(&self, draft_id: &str) -> PathBuf {
        self.blobs.join("drafts").join(safe_filename(draft_id))
    }

    /// Copies a file (or records a link) into the draft. Refuses folders, sent
    /// drafts and anything that would push the files past what Gmail can send.
    pub fn add_draft_attachment(
        &self,
        draft_id: &str,
        a: &NewAttachment,
    ) -> Result<DraftAttachment> {
        let d = self.draft(draft_id)?;
        if d.state == "sent" {
            return Err(Error::InvalidParams(format!(
                "draft {draft_id} was already sent"
            )));
        }
        let id = new_id("datt");
        let (kind, filename, mime, size, path, url) = match &a.source {
            AttachmentSource::Link(url) => {
                let title = a.filename.clone().unwrap_or_else(|| url.clone());
                (
                    AttachmentKind::Link,
                    title,
                    "text/uri-list".to_string(),
                    0,
                    None,
                    Some(url.clone()),
                )
            }
            src => {
                let (from, name, known_mime) = match src {
                    AttachmentSource::Path(p) => {
                        let meta = std::fs::metadata(p).map_err(|_| {
                            Error::InvalidParams(format!("{} could not be found.", p.display()))
                        })?;
                        if meta.is_dir() {
                            return Err(Error::InvalidParams(format!(
                                "{} is a folder. Compress it first, then attach the .zip file.",
                                p.display()
                            )));
                        }
                        let name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "attachment".into());
                        (p.clone(), name, None)
                    }
                    AttachmentSource::Message(aid) => {
                        let (path, name, mime, _) = self.attachment(aid)?;
                        (PathBuf::from(path), name, Some(mime))
                    }
                    AttachmentSource::Link(_) => unreachable!("handled above"),
                };
                let filename = a
                    .filename
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or(name);
                let size = std::fs::metadata(&from)?.len();
                let used: u64 = d.attachments.iter().map(|x| x.size.max(0) as u64).sum();
                crate::compose::check_total(used + size)?;
                let dir = self.draft_dir(draft_id).join(&id);
                std::fs::create_dir_all(&dir)?;
                let dest = dir.join(safe_filename(&filename));
                std::fs::copy(&from, &dest)?;
                let mut head = [0u8; 16];
                let n = std::io::Read::read(&mut std::fs::File::open(&dest)?, &mut head)?;
                let mime = match known_mime {
                    Some(m) if !m.is_empty() && m != "application/octet-stream" => m,
                    _ => crate::compose::mime_for(&filename, &head[..n]),
                };
                let dest = dest.to_string_lossy().into_owned();
                (
                    AttachmentKind::File,
                    filename,
                    mime,
                    size as i64,
                    Some(dest),
                    None,
                )
            }
        };
        let w = self.w.lock().unwrap();
        w.execute(
            "INSERT INTO draft_attachments (id, draft_id, kind, filename, mime, size, path, url, inline, ord)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
               (SELECT COALESCE(MAX(ord), 0) + 1 FROM draft_attachments WHERE draft_id = ?2))",
            params![
                id,
                draft_id,
                if kind == AttachmentKind::Link { "link" } else { "file" },
                filename,
                mime,
                size,
                path,
                url,
                a.inline as i64
            ],
        )?;
        w.execute(
            "UPDATE drafts SET updated = ?2 WHERE id = ?1",
            params![draft_id, now_ms()],
        )?;
        drop(w);
        Ok(DraftAttachment {
            id,
            kind,
            filename,
            mime,
            size,
            path,
            url,
            inline: a.inline,
        })
    }

    pub fn remove_draft_attachment(&self, draft_id: &str, attachment_id: &str) -> Result<()> {
        let n = self.w.lock().unwrap().execute(
            "DELETE FROM draft_attachments WHERE id = ?1 AND draft_id = ?2",
            params![attachment_id, draft_id],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!(
                "attachment {attachment_id} of draft {draft_id}"
            )));
        }
        let _ =
            std::fs::remove_dir_all(self.draft_dir(draft_id).join(safe_filename(attachment_id)));
        Ok(())
    }

    /// Drops a draft's file copies once they are no longer needed (sent or deleted).
    pub fn purge_draft_files(&self, draft_id: &str) -> Result<()> {
        self.w.lock().unwrap().execute(
            "DELETE FROM draft_attachments WHERE draft_id = ?1",
            [draft_id],
        )?;
        match std::fs::remove_dir_all(self.draft_dir(draft_id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub fn drafts(&self, state: Option<&str>) -> Result<Vec<Draft>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(&format!(
            "SELECT {DRAFT_COLS} FROM drafts t WHERE (?1 IS NULL OR state = ?1) AND state != 'sent'{} ORDER BY updated DESC",
            self.visible()
        ))?;
        let mut out: Vec<Draft> = st
            .query_map([state], draft_row)?
            .collect::<rusqlite::Result<_>>()?;
        for d in &mut out {
            d.attachments = Self::draft_attachments_on(&r, &d.id)?;
        }
        Ok(out)
    }

    pub fn update_draft(
        &self,
        id: &str,
        to: Option<Vec<String>>,
        cc: Option<Vec<String>>,
        subject: Option<String>,
        body: Option<String>,
    ) -> Result<Draft> {
        let mut d = self.draft(id)?;
        if d.state == "sent" {
            return Err(Error::InvalidParams(format!("draft {id} was already sent")));
        }
        if let Some(v) = to {
            d.to = v;
        }
        if let Some(v) = cc {
            d.cc = v;
        }
        if let Some(v) = subject {
            d.subject = v;
        }
        if let Some(v) = body {
            d.body = v;
        }
        let w = self.w.lock().unwrap();
        w.execute(
            "UPDATE drafts SET to_json = ?2, cc_json = ?3, subject = ?4, body = ?5, updated = ?6 WHERE id = ?1",
            params![id, serde_json::to_string(&d.to)?, serde_json::to_string(&d.cc)?, d.subject, d.body, now_ms()],
        )?;
        drop(w);
        self.draft(id)
    }

    pub fn delete_draft(&self, id: &str) -> Result<()> {
        let n = self
            .w
            .lock()
            .unwrap()
            .execute("DELETE FROM drafts WHERE id = ?1", [id])?;
        if n == 0 {
            return Err(Error::NotFound(format!("draft {id}")));
        }
        self.purge_draft_files(id)
    }

    /// Moves a draft to Review and its thread to the Review status, remembering
    /// the previous status so a rejection can put it back.
    pub fn move_to_review(&self, draft_id: &str) -> Result<Draft> {
        let d = self.draft(draft_id)?;
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let prev: Option<String> = match &d.thread_id {
            Some(t) => tx
                .query_row("SELECT status FROM threads WHERE id = ?1", [t], |r| {
                    r.get(0)
                })
                .optional()?,
            None => None,
        };
        tx.execute(
            "UPDATE drafts SET state = 'review', origin = 'agent', updated = ?2, prev_status = COALESCE(prev_status, ?3) WHERE id = ?1",
            params![draft_id, now_ms(), prev],
        )?;
        if let Some(t) = &d.thread_id {
            tx.execute("UPDATE threads SET status = 'review' WHERE id = ?1", [t])?;
            Self::enqueue_label_sync_tx(&tx, &d.account_id, t)?;
        }
        tx.commit()?;
        drop(w);
        self.draft(draft_id)
    }

    pub fn set_draft_state(&self, draft_id: &str, state: &str) -> Result<()> {
        self.w.lock().unwrap().execute(
            "UPDATE drafts SET state = ?2, updated = ?3 WHERE id = ?1",
            params![draft_id, state, now_ms()],
        )?;
        Ok(())
    }

    /// After a rejection the thread goes back to where it was before the agent asked.
    pub fn reject_draft(&self, draft_id: &str) -> Result<()> {
        let d = self.draft(draft_id)?;
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let prev: Option<String> = tx.query_row(
            "SELECT prev_status FROM drafts WHERE id = ?1",
            [draft_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE drafts SET state = 'rejected', updated = ?2 WHERE id = ?1",
            params![draft_id, now_ms()],
        )?;
        if let Some(t) = &d.thread_id {
            tx.execute(
                "UPDATE threads SET status = ?2 WHERE id = ?1 AND status = 'review'",
                params![t, prev.unwrap_or_else(|| "todo".into())],
            )?;
            Self::enqueue_label_sync_tx(&tx, &d.account_id, t)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Reply threading headers: In-Reply-To and References from the last message.
    /// In-Reply-To and References for a reply to `message_id`.
    pub fn reply_headers(&self, message_id: &str) -> Result<(Option<String>, Vec<String>)> {
        let r = self.r.lock().unwrap();
        let row: Option<(Option<String>, String)> = r
            .query_row(
                "SELECT message_id_header, references_json FROM messages WHERE id = ?1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((mid, refs)) = row else {
            return Ok((None, vec![]));
        };
        let mut refs: Vec<String> = serde_json::from_str(&refs).unwrap_or_default();
        if let Some(m) = &mid {
            refs.push(m.clone());
        }
        Ok((mid, refs))
    }

    /// Defaults for a reply-all to the thread's reply target ([`crate::reply`] rules).
    pub fn reply_defaults(
        &self,
        thread_id: &str,
        me: &str,
    ) -> Result<(Vec<String>, Vec<String>, String)> {
        let msgs = self.messages(thread_id)?;
        let t = self.thread(thread_id)?;
        let subject = if t.subject.to_lowercase().starts_with("re:") {
            t.subject.clone()
        } else {
            format!("Re: {}", t.subject)
        };
        let Some(last) = crate::reply::target(&msgs) else {
            return Ok((vec![], vec![], subject));
        };
        let (to, cc) = crate::reply::reply_all(last, &[me.to_string()], &msgs);
        let emails = |v: Vec<Address>| v.into_iter().map(|a| a.email).collect::<Vec<_>>();
        let (to, cc) = (emails(to), emails(cc));
        Ok((to, cc, subject))
    }

    // ---- built-in agent ---------------------------------------------------

    pub fn providers(&self) -> Result<Vec<crate::agent::Provider>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT id, name, kind, base_url, model, auth, profile FROM providers ORDER BY created",
        )?;
        Ok(st
            .query_map([], |r| {
                Ok(crate::agent::Provider {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    base_url: r.get(3)?,
                    model: r.get(4)?,
                    auth: r.get(5)?,
                    profile: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_provider(&self, p: &crate::agent::Provider) -> Result<()> {
        if !matches!(p.kind.as_str(), "anthropic" | "openai" | "chatgpt") {
            return Err(Error::InvalidParams(format!(
                "kind must be anthropic or openai, got '{}'",
                p.kind
            )));
        }
        if p.name.trim().is_empty() || p.model.trim().is_empty() {
            return Err(Error::InvalidParams(
                "provider needs a name and a model".into(),
            ));
        }
        self.w.lock().unwrap().execute(
            "INSERT INTO providers (id, name, kind, base_url, model, created, auth, profile) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, kind = excluded.kind, base_url = excluded.base_url,
               model = excluded.model, auth = excluded.auth, profile = excluded.profile",
            params![p.id, p.name, p.kind, p.base_url, p.model, now_ms(), p.auth, p.profile],
        )?;
        Ok(())
    }

    pub fn delete_provider(&self, id: &str) -> Result<()> {
        let w = self.w.lock().unwrap();
        w.execute("DELETE FROM account_providers WHERE provider_id = ?1", [id])?;
        if w.execute("DELETE FROM providers WHERE id = ?1", [id])? == 0 {
            return Err(Error::NotFound(format!("provider {id}")));
        }
        Ok(())
    }

    /// Provider ids an account may send mail to; "*" means any configured provider.
    pub fn account_providers(&self, account_id: &str) -> Result<Vec<String>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT provider_id FROM account_providers WHERE account_id = ?1 ORDER BY provider_id",
        )?;
        Ok(st
            .query_map([account_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_account_providers(&self, account_id: &str, ids: &[String]) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        tx.execute(
            "DELETE FROM account_providers WHERE account_id = ?1",
            [account_id],
        )?;
        for id in ids {
            tx.execute(
                "INSERT OR IGNORE INTO account_providers (account_id, provider_id) VALUES (?1, ?2)",
                params![account_id, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The provider that will read this account's mail: the first allowed one, or none.
    pub fn provider_for_account(&self, account_id: &str) -> Result<Option<crate::agent::Provider>> {
        // The demo account's mail never goes to an AI: its AI words come with the demo.
        if self
            .account(account_id)
            .is_ok_and(|a| crate::demo_account::is_demo(&a))
        {
            return Ok(None);
        }
        let allowed = self.account_providers(account_id)?;
        let all = self.providers()?;
        Ok(all
            .into_iter()
            .find(|p| allowed.iter().any(|a| a == "*" || *a == p.id)))
    }

    /// Received messages not yet digested, newest first, within the backfill window.
    pub fn pending_digests(
        &self,
        account_id: &str,
        since_ms: i64,
        limit: i64,
    ) -> Result<Vec<PendingDigest>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT m.id, m.from_name, m.from_email, m.subject, m.body_text FROM messages m
             WHERE m.account_id = ?1 AND m.is_from_me = 0 AND m.date >= ?2 AND length(m.body_text) > 0
               AND NOT EXISTS (SELECT 1 FROM digests d WHERE d.message_id = m.id)
             ORDER BY m.date DESC LIMIT ?3",
        )?;
        Ok(st
            .query_map(params![account_id, since_ms, limit], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Records a digest outcome; a card is merged into the sender's directory entry.
    /// Returns the message's thread id.
    pub fn save_digest(
        &self,
        message_id: &str,
        provider_id: &str,
        outcome: &Result<crate::agent::Digest>,
    ) -> Result<String> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let (thread_id, from_name, from_email, date): (String, String, String, i64) = tx
            .query_row(
                "SELECT thread_id, from_name, from_email, date FROM messages WHERE id = ?1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
        match outcome {
            Ok(d) => {
                tx.execute(
                    "INSERT OR REPLACE INTO digests (message_id, status, clean_body, card_json, provider_id, created) VALUES (?1, 'ok', ?2, ?3, ?4, ?5)",
                    params![message_id, d.clean_body, d.card.as_ref().map(|c| serde_json::to_string(c).unwrap_or_default()), provider_id, now_ms()],
                )?;
                if let Some(card) = &d.card {
                    Self::merge_contact(&tx, &from_email, &from_name, card, date)?;
                }
            }
            Err(e) => {
                tx.execute(
                    "INSERT OR REPLACE INTO digests (message_id, status, provider_id, error, created) VALUES (?1, 'error', ?2, ?3, ?4)",
                    params![message_id, provider_id, e.to_string(), now_ms()],
                )?;
            }
        }
        tx.commit()?;
        Ok(thread_id)
    }

    fn merge_contact(
        tx: &Connection,
        email: &str,
        from_name: &str,
        card: &crate::agent::Card,
        seen: i64,
    ) -> Result<()> {
        let email = email.trim().to_lowercase();
        if email.is_empty() {
            return Ok(());
        }
        let old = Self::contact_tx(tx, &email)?
            .map(|c| c.card)
            .unwrap_or_default();
        let mut merged = crate::agent::merge_card(&old, card);
        if merged.name.is_empty() {
            merged.name = from_name.to_string();
        }
        tx.execute(
            "INSERT INTO contacts (email, name, title, company, phones_json, address, website, socials_json, last_seen, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(email) DO UPDATE SET name = excluded.name, title = excluded.title, company = excluded.company,
               phones_json = excluded.phones_json, address = excluded.address, website = excluded.website,
               socials_json = excluded.socials_json, last_seen = MAX(contacts.last_seen, excluded.last_seen), updated = excluded.updated",
            params![
                email,
                merged.name,
                merged.title,
                merged.company,
                serde_json::to_string(&merged.phones)?,
                merged.address,
                merged.website,
                serde_json::to_string(&merged.socials)?,
                seen,
                now_ms()
            ],
        )?;
        Ok(())
    }

    fn contact_tx(conn: &Connection, email: &str) -> Result<Option<Contact>> {
        Ok(conn
            .query_row(
                "SELECT email, name, title, company, phones_json, address, website, socials_json, last_seen FROM contacts WHERE email = ?1",
                [email],
                contact_row,
            )
            .optional()?)
    }

    pub fn contact(&self, email: &str) -> Result<Contact> {
        let r = self.r.lock().unwrap();
        Self::contact_tx(&r, &email.trim().to_lowercase())?
            .ok_or_else(|| Error::NotFound(format!("contact {email}")))
    }

    /// Directory, most recently heard from first; `query` matches name, email or company.
    pub fn contacts(&self, query: Option<&str>, limit: i64) -> Result<Vec<Contact>> {
        let r = self.r.lock().unwrap();
        let like = format!("%{}%", query.unwrap_or("").trim().to_lowercase());
        let mut st = r.prepare_cached(
            "SELECT email, name, title, company, phones_json, address, website, socials_json, last_seen FROM contacts
             WHERE lower(name) LIKE ?1 OR email LIKE ?1 OR lower(company) LIKE ?1 ORDER BY last_seen DESC LIMIT ?2",
        )?;
        Ok(st
            .query_map(params![like, limit], contact_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// The directory: people the user has written to or whose signature was read, one entry
    /// per person even when they use several addresses; most recent first.
    pub fn people(&self, query: Option<&str>, limit: i64) -> Result<Vec<Person>> {
        let r = self.r.lock().unwrap();
        // (email, name, messages, last_seen, written_to)
        let mut by_email: std::collections::HashMap<String, (String, i64, i64, bool)> =
            Default::default();
        let mut st = r.prepare_cached(
            &format!("SELECT lower(from_email), MAX(from_name), COUNT(*), MAX(date) FROM messages m WHERE is_from_me = 0 AND from_email != ''{} GROUP BY lower(from_email)", self.visible_m()),
        )?;
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })? {
            let (e, n, c, d) = row?;
            by_email.insert(e, (n, c, d, false));
        }
        let mut st = r.prepare_cached(&format!(
            "SELECT lower(json_extract(j.value, '$.email')), MAX(json_extract(j.value, '$.name')), COUNT(*), MAX(m.date)
             FROM messages m, json_each(m.to_json) j WHERE m.is_from_me = 1{} GROUP BY 1",
            self.visible_m()
        ))?;
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })? {
            let (Some(e), n, c, d) = row? else { continue };
            let entry = by_email.entry(e).or_insert((String::new(), 0, 0, false));
            if entry.0.is_empty() {
                entry.0 = n.unwrap_or_default();
            }
            entry.1 += c;
            entry.2 = entry.2.max(d);
            entry.3 = true;
        }
        // Threads per address, both directions, to count conversations per person.
        let mut threads_of: std::collections::HashMap<String, std::collections::HashSet<String>> =
            Default::default();
        let mut st = r.prepare_cached(&format!(
            "SELECT lower(from_email), thread_id FROM messages m WHERE is_from_me = 0{v} GROUP BY 1, 2
             UNION SELECT lower(json_extract(j.value, '$.email')), m.thread_id FROM messages m, json_each(m.to_json) j
             WHERE m.is_from_me = 1{v} GROUP BY 1, 2",
            v = self.visible_m()
        ))?;
        for row in st.query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
        })? {
            if let (Some(e), t) = row? {
                threads_of.entry(e).or_default().insert(t);
            }
        }
        let mut overrides: std::collections::HashMap<String, (String, String, String)> =
            Default::default();
        let mut st =
            r.prepare_cached("SELECT email, person_id, name, kind FROM person_overrides")?;
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (e, id, n, k) = row?;
            overrides.insert(e, (id, n, k));
        }
        let mut cards: std::collections::HashMap<String, Contact> = Default::default();
        let mut st = r.prepare_cached("SELECT email, name, title, company, phones_json, address, website, socials_json, last_seen FROM contacts")?;
        for c in st.query_map([], contact_row)? {
            let c = c?;
            cards.insert(c.email.clone(), c);
        }
        // People only: someone the user wrote to, or whose signature carried details.
        let mut people: Vec<Person> = vec![];
        let mut index: std::collections::HashMap<String, usize> = Default::default();
        let mut convs: Vec<std::collections::HashSet<String>> = vec![];
        let mut emails: Vec<_> = by_email.into_iter().collect();
        emails.sort_by_key(|e| std::cmp::Reverse(e.1.2));
        for (email, (name, _count, last, written)) in emails {
            let card = cards.get(&email);
            if is_automated(&email) || (!written && card.is_none()) {
                continue;
            }
            let display = card
                .map(|c| c.card.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| display_person(&name));
            // Nameless correspondents show (and sort) as their address.
            let display = if display.trim().is_empty() {
                email.clone()
            } else {
                display
            };
            let key = person_key(&display);
            let card_data = card.map(|c| c.card.clone()).unwrap_or_default();
            let mut merge_key = if key.split(' ').count() >= 2 {
                key
            } else {
                email.clone()
            };
            // The AI's grouping and clean name win over the heuristics.
            let mut kind = "person".to_string();
            let display = match overrides.get(&email) {
                Some((id, name, k)) => {
                    merge_key = format!("ai:{id}");
                    kind = k.clone();
                    name.clone()
                }
                None => display,
            };
            match index.get(&merge_key) {
                Some(&i) => {
                    let p = &mut people[i];
                    p.emails.push(email.clone());
                    convs[i].extend(threads_of.remove(&email).unwrap_or_default());
                    p.last_seen = p.last_seen.max(last);
                    p.card = crate::agent::merge_card(&card_data, &p.card);
                }
                None => {
                    let mut c = card_data;
                    c.name = display;
                    index.insert(merge_key, people.len());
                    convs.push(threads_of.remove(&email).unwrap_or_default());
                    people.push(Person {
                        email: email.clone(),
                        emails: vec![email],
                        card: c,
                        conversations: 0,
                        last_seen: last,
                        kind,
                    });
                }
            }
        }
        for (p, c) in people.iter_mut().zip(convs) {
            p.conversations = c.len() as i64;
        }
        // Alphabetical by family name, then first name, like Contacts (accents and
        // "Last, First" folded); entries
        // without a real name (just an address) go last.
        people.sort_by_cached_key(|p| sort_key(&p.card.name));
        let q = query.unwrap_or("").trim().to_lowercase();
        Ok(people
            .into_iter()
            .filter(|p| {
                q.is_empty()
                    || p.card.name.to_lowercase().contains(&q)
                    || p.card.company.to_lowercase().contains(&q)
                    || p.emails.iter().any(|e| e.contains(&q))
            })
            .take(limit.max(0) as usize)
            .collect())
    }

    /// The person behind an address (with all their addresses).
    pub fn person(&self, email: &str) -> Result<Person> {
        let e = email.trim().to_lowercase();
        self.people(None, i64::MAX)?
            .into_iter()
            .find(|p| p.emails.contains(&e))
            .ok_or_else(|| Error::NotFound(format!("person {email}")))
    }

    /// Records the AI's grouping: every address of every member maps to one person.
    pub fn apply_person_groups(
        &self,
        people: &[Person],
        groups: &[crate::agent::PersonGroup],
    ) -> Result<usize> {
        let by_id: std::collections::HashMap<&str, &Person> =
            people.iter().map(|p| (p.email.as_str(), p)).collect();
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let mut n = 0;
        for g in groups {
            let members: Vec<&Person> = g
                .ids
                .iter()
                .filter_map(|i| by_id.get(i.as_str()).copied())
                .collect();
            let Some(first) = members.first() else {
                continue;
            };
            let name = g.name.trim();
            if name.is_empty() {
                continue;
            }
            let person_id = first.email.clone();
            for m in &members {
                for e in &m.emails {
                    tx.execute(
                        "INSERT INTO person_overrides (email, person_id, name, kind, updated) VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(email) DO UPDATE SET person_id = excluded.person_id, name = excluded.name, kind = excluded.kind, updated = excluded.updated",
                        params![e, person_id, name, g.kind, now_ms()],
                    )?;
                }
            }
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    /// A thread's messages to clean, mine included: every one for a forced re-clean, or
    /// only those without a stored clean version (`missing_only`), which are never redone.
    pub fn thread_digest_inputs(
        &self,
        thread_id: &str,
        missing_only: bool,
    ) -> Result<Vec<PendingDigest>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT id, from_name, from_email, subject, body_text FROM messages m WHERE thread_id = ?1 AND length(body_text) > 0
               AND (?2 = 0 OR NOT EXISTS (SELECT 1 FROM digests d WHERE d.message_id = m.id AND d.status = 'ok'))
             ORDER BY date",
        )?;
        Ok(st
            .query_map(params![thread_id, missing_only as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// (done, pending, failed) digests for an account inside the backfill window.
    pub fn digest_stats(&self, account_id: &str, since_ms: i64) -> Result<(i64, i64, i64)> {
        let r = self.r.lock().unwrap();
        Ok(r.query_row(
            "SELECT
               (SELECT COUNT(*) FROM digests d JOIN messages m ON m.id = d.message_id WHERE m.account_id = ?1 AND d.status = 'ok'),
               (SELECT COUNT(*) FROM messages m WHERE m.account_id = ?1 AND m.is_from_me = 0 AND m.date >= ?2 AND length(m.body_text) > 0
                  AND NOT EXISTS (SELECT 1 FROM digests d WHERE d.message_id = m.id)),
               (SELECT COUNT(*) FROM digests d JOIN messages m ON m.id = d.message_id WHERE m.account_id = ?1 AND d.status = 'error')",
            params![account_id, since_ms],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)
    }

    // ---- chat about a thread -----------------------------------------------

    /// A thread's conversation with the AI, oldest first.
    pub fn chat_messages(&self, thread_id: &str) -> Result<Vec<crate::chat::ChatMessage>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT id, thread_id, role, text, actions_json, searched, research_unavailable, created
             FROM chat_messages WHERE thread_id = ?1 ORDER BY created, rowid",
        )?;
        Ok(st
            .query_map([thread_id], |r| {
                Ok(crate::chat::ChatMessage {
                    id: r.get(0)?,
                    thread_id: r.get(1)?,
                    role: r.get(2)?,
                    text: r.get(3)?,
                    actions: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                    searched: r.get::<_, i64>(5)? != 0,
                    research_unavailable: r.get::<_, i64>(6)? != 0,
                    created: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_chat_message(&self, m: &crate::chat::ChatMessage) -> Result<()> {
        self.w.lock().unwrap().execute(
            "INSERT INTO chat_messages (id, thread_id, role, text, actions_json, searched, research_unavailable, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                m.id,
                m.thread_id,
                m.role,
                m.text,
                serde_json::to_string(&m.actions)?,
                m.searched as i64,
                m.research_unavailable as i64,
                m.created
            ],
        )?;
        Ok(())
    }

    /// Forgets a thread's conversation (the panel's "Start over").
    pub fn clear_chat(&self, thread_id: &str) -> Result<usize> {
        Ok(self.w.lock().unwrap().execute(
            "DELETE FROM chat_messages WHERE thread_id = ?1",
            [thread_id],
        )?)
    }

    /// The account's own latest messages with these correspondents, newest first.
    /// Keeping history within the account preserves per-mailbox AI consent.
    pub fn my_past_messages(
        &self,
        thread_id: &str,
        limit: i64,
    ) -> Result<Vec<crate::chat::PastMessage>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "WITH people AS (
               SELECT DISTINCT lower(from_email) e FROM messages WHERE thread_id = ?1 AND is_from_me = 0),
             peers AS (
               SELECT DISTINCT m.thread_id FROM people p JOIN messages m ON lower(m.from_email) = p.e
               WHERE m.thread_id != ?1 AND m.account_id = (SELECT account_id FROM threads WHERE id = ?1))
             SELECT m.date, m.subject, m.body_text FROM peers JOIN messages m ON m.thread_id = peers.thread_id
             WHERE m.is_from_me = 1 AND m.account_id = (SELECT account_id FROM threads WHERE id = ?1)
             ORDER BY m.date DESC LIMIT ?2",
        )?;
        Ok(st
            .query_map(params![thread_id, limit], |r| {
                let body: String = r.get(2)?;
                Ok(crate::chat::PastMessage {
                    date: r.get(0)?,
                    subject: r.get(1)?,
                    body: crate::quote::readable(&body).0.chars().take(600).collect(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    // ---- settings / feedback ---------------------------------------------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        let r = self.r.lock().unwrap();
        Ok(
            r.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?,
        )
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.w.lock().unwrap().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn add_feedback(
        &self,
        text: &str,
        command: Option<&str>,
        agent: Option<&str>,
    ) -> Result<String> {
        let id = new_id("fbk");
        self.w.lock().unwrap().execute(
            "INSERT INTO feedback (id, text, command, agent, created) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, text, command, agent, now_ms()],
        )?;
        Ok(id)
    }

    pub fn feedback(&self) -> Result<Vec<Feedback>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT id, text, command, agent, created FROM feedback ORDER BY created DESC",
        )?;
        Ok(st
            .query_map([], |r| {
                Ok(Feedback {
                    id: r.get(0)?,
                    text: r.get(1)?,
                    command: r.get(2)?,
                    agent: r.get(3)?,
                    created: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    // ---- write-back queue ------------------------------------------------

    fn enqueue_tx(
        tx: &Connection,
        account_id: &str,
        kind: &str,
        key: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        // One pending op per (kind, key): a newer edit supersedes an unsent one and,
        // being the user's own, no longer waits behind a bulk run.
        tx.execute(
            "INSERT INTO ops (account_id, kind, key, payload, created) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(kind, key) DO UPDATE SET payload = excluded.payload, seq = seq + 1, attempts = 0, last_error = NULL, bulk = 0",
            params![account_id, kind, key, payload.to_string(), now_ms()],
        )?;
        Ok(())
    }

    fn enqueue_label_sync_tx(tx: &Connection, account_id: &str, thread_id: &str) -> Result<()> {
        Self::enqueue_tx(tx, account_id, "labels", thread_id, &serde_json::json!({}))
    }

    pub fn enqueue(
        &self,
        account_id: &str,
        kind: &str,
        key: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let w = self.w.lock().unwrap();
        Self::enqueue_tx(&w, account_id, kind, key, payload)
    }

    pub fn pending_ops(&self, limit: i64) -> Result<Vec<Op>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached("SELECT id, account_id, kind, key, payload, attempts, seq, bulk FROM ops ORDER BY id LIMIT ?1")?;
        Ok(st
            .query_map([limit], |r| {
                let p: String = r.get(4)?;
                Ok(Op {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    kind: r.get(2)?,
                    key: r.get(3)?,
                    payload: serde_json::from_str(&p).unwrap_or_default(),
                    attempts: r.get(5)?,
                    seq: r.get(6)?,
                    bulk: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Removes an op only if it wasn't superseded while it was in flight.
    pub fn op_done(&self, op: &Op) -> Result<()> {
        self.w.lock().unwrap().execute(
            "DELETE FROM ops WHERE id = ?1 AND seq = ?2",
            params![op.id, op.seq],
        )?;
        Ok(())
    }

    pub fn op_failed(&self, op: &Op, err: &str) -> Result<()> {
        self.w.lock().unwrap().execute(
            "UPDATE ops SET attempts = attempts + 1, last_error = ?2 WHERE id = ?1",
            params![op.id, err],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::ParsedMessage;
    use rstest::*;

    const DAY: i64 = 24 * 3600 * 1000;

    struct Env {
        _dir: tempfile::TempDir,
        store: Store,
        acc: String,
    }

    #[fixture]
    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        let acc = store.add_account("me@example.com", "gmail").unwrap().id;
        Env {
            _dir: dir,
            store,
            acc,
        }
    }

    fn incoming(
        pid: &str,
        tid: &str,
        from: &str,
        labels: &[&str],
        date: i64,
        body: &str,
    ) -> IncomingMessage {
        IncomingMessage {
            provider_id: pid.into(),
            provider_thread_id: tid.into(),
            label_ids: labels.iter().map(|s| s.to_string()).collect(),
            internal_date: date,
            parsed: ParsedMessage {
                from: Some(Address {
                    name: "Marc Petit".into(),
                    email: from.into(),
                }),
                to: vec![Address {
                    name: "".into(),
                    email: "me@example.com".into(),
                }],
                subject: "Audit report v2".into(),
                body_text: body.into(),
                message_id: Some(format!("<{pid}@x>")),
                ..Default::default()
            },
        }
    }

    #[rstest]
    #[case::status_label_wins(true, false, false, Some("waiting"), 0, SyncMode::Initial, "waiting")]
    #[case::trash_wins(true, true, true, Some("waiting"), 0, SyncMode::Incremental, "deleted")]
    #[case::archived_is_archive(false, false, false, None, 0, SyncMode::Initial, "archive")]
    #[case::recent_unread_inbox_is_unread(true, false, true, None, 0, SyncMode::Initial, "unread")]
    #[case::recent_read_inbox_is_todo(true, false, false, None, 0, SyncMode::Initial, "todo")]
    #[case::old_inbox_initial_is_backlog(true, false, true, None, 40, SyncMode::Initial, "backlog")]
    #[case::old_inbox_incremental_is_unread(
        true,
        false,
        true,
        None,
        40,
        SyncMode::Incremental,
        "unread"
    )]
    fn initial_status_rules(
        #[case] inbox: bool,
        #[case] trashed: bool,
        #[case] unread: bool,
        #[case] label: Option<&str>,
        #[case] age_days: i64,
        #[case] mode: SyncMode,
        #[case] want: &str,
    ) {
        let now = 1_000 * DAY;
        assert_eq!(
            initial_status(
                inbox,
                trashed,
                unread,
                label,
                now - age_days * DAY,
                now,
                mode
            ),
            want
        );
    }

    #[rstest]
    #[case::done_reopens("done", false, true, Some("unread"))]
    #[case::archive_reopens("archive", false, true, Some("unread"))]
    #[case::in_progress_is_interrupted("in_progress", false, true, Some("unread"))]
    #[case::my_reply_does_not_reopen("done", true, true, None)]
    #[case::filtered_out_of_inbox_does_not_reopen("archive", false, false, None)]
    #[case::review_keeps_its_draft("review", false, true, None)]
    fn reopen_rule(
        #[case] current: &str,
        #[case] from_me: bool,
        #[case] in_inbox: bool,
        #[case] want: Option<&str>,
    ) {
        assert_eq!(status_after_new_message(current, from_me, in_inbox), want);
    }

    #[rstest]
    #[case::waiting_gets_its_answer(Some("waiting"), Some("archive"), None, "todo")]
    #[case::in_progress_resumes(Some("in_progress"), Some("archive"), None, "in_progress")]
    #[case::suggestion_applies(None, Some("archive"), Some("done"), "archive")]
    #[case::habit_without_suggestion(None, None, Some("archive"), "archive")]
    #[case::todo_by_default(None, None, None, "todo")]
    #[case::done_does_not_resume(Some("done"), None, None, "todo")]
    fn read_rule(
        #[case] prev: Option<&str>,
        #[case] suggested: Option<&str>,
        #[case] habit: Option<&str>,
        #[case] want: &str,
    ) {
        assert_eq!(status_after_read(prev, suggested, habit), want);
    }

    #[rstest]
    #[case::unread_inbox_only("unread", false, "Unread", 0, vec!["INBOX"])]
    #[case::archive_has_nothing("archive", true, "Archive", 0, vec![])]
    #[case::done_archived_with_label("done", true, "Done", 0, vec!["Mermoz/Done"])]
    #[case::todo_label("todo", false, "Todo", 0, vec!["INBOX", "Mermoz/Todo"])]
    #[case::deleted_has_nothing("deleted", true, "Deleted", 2, vec![])]
    #[case::priority_label("todo", false, "Todo", 2, vec!["INBOX", "Mermoz/Priority/High", "Mermoz/Todo"])]
    #[case::normal_priority_no_label("unread", false, "Unread", 3, vec!["INBOX"])]
    fn desired_label_sets(
        #[case] status: &str,
        #[case] archived: bool,
        #[case] name: &str,
        #[case] prio: i64,
        #[case] want: Vec<&str>,
    ) {
        let got: Vec<String> = desired_labels(status, archived, name, prio)
            .into_iter()
            .collect();
        assert_eq!(got, want);
    }

    #[rstest]
    #[case::user_label("Label_7", "Immo/Houlgate", true)]
    #[case::inbox("INBOX", "INBOX", false)]
    #[case::star("YELLOW_STAR", "YELLOW_STAR", false)]
    #[case::category("CATEGORY_UPDATES", "CATEGORY_UPDATES", false)]
    #[case::mermoz_status("Label_1", "Mermoz/Todo", false)]
    fn user_label_detection(#[case] id: &str, #[case] name: &str, #[case] want: bool) {
        assert_eq!(is_user_label(id, name), want);
    }

    #[rstest]
    #[case::finished_words_are_whole("audit report", false, Some("\"audit\" \"report\""))]
    #[case::typed_word_is_a_prefix("audit rep", true, Some("\"audit\" \"rep\"*"))]
    #[case::a_space_finishes_the_word("audit rep ", true, Some("\"audit\" \"rep\""))]
    #[case::short_typed_word_is_whole("audit re", true, Some("\"audit\" \"re\""))]
    #[case::one_letter_is_whole("m", true, Some("\"m\""))]
    #[case::from_maps_to_sender("from:marc", false, Some("sender:\"marc\""))]
    #[case::typed_sender_is_a_prefix("from:mar", true, Some("sender:\"mar\"*"))]
    #[case::punctuation_is_dropped("\"hello\" (world)", false, Some("\"hello\" \"world\""))]
    #[case::empty_is_none("  ", false, None)]
    fn fts_query_matches_every_word(
        #[case] q: &str,
        #[case] typing: bool,
        #[case] want: Option<&str>,
    ) {
        assert_eq!(fts_query(q, typing).map(|f| f.all).as_deref(), want);
    }

    #[rstest]
    #[case::words_in_a_row("french bee", false, Some("\"french bee\""))]
    #[case::typed_phrase_ends_in_a_prefix("french bee", true, Some("\"french bee\"*"))]
    #[case::columns_stay_out("from:ada french bee", false, Some("\"french bee\""))]
    #[case::one_word_has_none("french", false, None)]
    #[case::a_long_typed_word_waits("french beekeep", true, None)]
    fn fts_query_phrase(#[case] q: &str, #[case] typing: bool, #[case] want: Option<&str>) {
        assert_eq!(fts_query(q, typing).unwrap().phrase.as_deref(), want);
    }

    #[rstest]
    #[case::finished_words("french bee", false, Some("{subject sender} : (\"french\" \"bee\")"))]
    #[case::a_long_typed_word_waits("beekeep", true, None)]
    fn fts_query_headline(#[case] q: &str, #[case] typing: bool, #[case] want: Option<&str>) {
        assert_eq!(fts_query(q, typing).unwrap().headline.as_deref(), want);
    }

    #[rstest]
    fn new_unread_inbox_thread_is_unread(env: Env) {
        let tid = env
            .store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming(
                    "m1",
                    "t1",
                    "marc@x.com",
                    &["INBOX", "UNREAD"],
                    now_ms(),
                    "hi",
                ),
                SyncMode::Initial,
            )
            .unwrap();
        assert_eq!(env.store.thread(&tid).unwrap().status, "unread");
    }

    #[rstest]
    fn thread_number_is_assigned(env: Env) {
        let tid = env
            .store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        assert_eq!(env.store.thread(&tid).unwrap().number, "M-1");
    }

    #[rstest]
    fn reply_reopens_done_thread_incrementally(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &[], now_ms() - DAY, "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m2", "t1", "marc@x.com", &["INBOX"], now_ms(), "again"),
            SyncMode::Incremental,
        )
        .unwrap();
        assert_eq!(s.thread(&tid).unwrap().status, "unread");
    }

    #[rstest]
    fn unread_aggregates_to_thread(env: Env) {
        let tid = env
            .store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming(
                    "m1",
                    "t1",
                    "marc@x.com",
                    &["INBOX", "UNREAD"],
                    now_ms(),
                    "hi",
                ),
                SyncMode::Initial,
            )
            .unwrap();
        assert!(env.store.thread(&tid).unwrap().unread);
    }

    #[rstest]
    fn search_finds_body_words(env: Env) {
        env.store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming(
                    "m1",
                    "t1",
                    "marc@x.com",
                    &["INBOX"],
                    now_ms(),
                    "remediation timeline",
                ),
                SyncMode::Initial,
            )
            .unwrap();
        assert_eq!(env.store.search("remed", 10, true).unwrap().len(), 1);
    }

    #[rstest]
    fn reopening_keeps_schema_version(env: Env) {
        let path = env._dir.path().join("db.sqlite");
        drop(env.store);
        let again = Store::open(&path, &env._dir.path().join("blobs")).unwrap();
        let v: i64 = again
            .r
            .lock()
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
    }

    #[rstest]
    #[case::separators("a/b:c.pdf", "a_b_c.pdf")]
    #[case::empty("  ", "attachment")]
    #[case::hidden(".env", "env")]
    #[case::newline("re\nport.pdf", "re_port.pdf")]
    fn safe_filename_cleans(#[case] input: &str, #[case] want: &str) {
        assert_eq!(safe_filename(input), want);
    }

    #[rstest]
    fn safe_filename_caps_length_keeping_extension() {
        let long = format!("{}.pdf", "é".repeat(200));
        let out = safe_filename(&long);
        assert!(out.len() <= 120 && out.ends_with(".pdf"));
    }

    #[rstest]
    fn long_attachment_name_does_not_block_ingest(env: Env) {
        let mut m = incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi");
        m.parsed.attachments.push(crate::mime::ParsedAttachment {
            filename: "x".repeat(400),
            mime: "text/plain".into(),
            data: b"d".to_vec(),
            ..Default::default()
        });
        assert!(
            env.store
                .ingest(&env.acc, "me@example.com", &m, SyncMode::Initial)
                .is_ok()
        );
    }

    #[rstest]
    fn failing_message_does_not_block_batch(env: Env) {
        let blobs = env._dir.path().join("blobs");
        let good = incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "ok");
        let mut bad = incoming("m2", "t2", "marc@x.com", &["INBOX"], now_ms(), "bad");
        bad.parsed.attachments.push(crate::mime::ParsedAttachment {
            filename: "a.txt".into(),
            mime: "text/plain".into(),
            data: b"d".to_vec(),
            ..Default::default()
        });
        // Blobs dir replaced by a file: attachment writes fail for this batch.
        std::fs::remove_dir_all(&blobs).unwrap();
        std::fs::write(&blobs, b"").unwrap();
        let ids = env
            .store
            .ingest_many(&env.acc, "me@example.com", &[bad, good], SyncMode::Initial)
            .unwrap();
        assert_eq!(ids.len(), 1);
    }

    /// A batch where one message fails: the failed one leaves no search entry.
    fn batch_with_a_failure(env: &Env) {
        let blobs = env._dir.path().join("blobs");
        let good = incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "okidoki");
        let mut bad = incoming("m2", "t2", "marc@x.com", &["INBOX"], now_ms(), "zebracorn");
        bad.parsed.attachments.push(crate::mime::ParsedAttachment {
            filename: "a.txt".into(),
            mime: "text/plain".into(),
            data: b"d".to_vec(),
            ..Default::default()
        });
        std::fs::remove_dir_all(&blobs).unwrap();
        std::fs::write(&blobs, b"").unwrap();
        env.store
            .ingest_many(&env.acc, "me@example.com", &[good, bad], SyncMode::Initial)
            .unwrap();
    }

    #[rstest]
    fn failed_message_is_not_searchable(env: Env) {
        batch_with_a_failure(&env);
        assert!(env.store.search("zebracorn", 10, false).unwrap().is_empty());
    }

    #[rstest]
    fn rest_of_a_failed_batch_is_searchable(env: Env) {
        batch_with_a_failure(&env);
        assert_eq!(env.store.search("okidoki", 10, false).unwrap().len(), 1);
    }

    #[rstest]
    fn checkpoint_copies_a_write(env: Env) {
        env.store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        assert!(env.store.checkpoint().unwrap() > 0);
    }

    #[rstest]
    fn checkpoint_waits_out_a_write_in_flight(env: Env) {
        env.store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        let _writing = env.store.w.lock().unwrap();
        assert_eq!(env.store.checkpoint().unwrap(), 0);
    }

    #[rstest]
    fn checkpoint_after_a_write_in_flight_copies_it(env: Env) {
        env.store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        let writing = env.store.w.lock().unwrap();
        env.store.checkpoint().unwrap();
        drop(writing);
        assert!(env.store.checkpoint().unwrap() > 0);
    }

    #[rstest]
    fn sync_file_flushes_a_written_file() {
        let mut f = tempfile::tempfile().unwrap();
        std::io::Write::write_all(&mut f, b"page").unwrap();
        assert!(super::sync_file(&f).is_ok());
    }

    #[rstest]
    fn checkpoint_without_writes_does_nothing(env: Env) {
        env.store.checkpoint().unwrap();
        assert_eq!(env.store.checkpoint().unwrap(), 0);
    }

    #[rstest]
    fn single_ingest_is_searchable(env: Env) {
        env.store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "okidoki"),
                SyncMode::Initial,
            )
            .unwrap();
        assert_eq!(env.store.search("okidoki", 10, false).unwrap().len(), 1);
    }

    #[rstest]
    fn digest_card_lands_in_directory(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        let mid = s.messages(&tid).unwrap()[0].id.clone();
        let card = crate::agent::Card {
            title: "CFO".into(),
            ..Default::default()
        };
        s.save_digest(
            &mid,
            "prv",
            &Ok(crate::agent::Digest {
                clean_body: "hi".into(),
                card: Some(card),
            }),
        )
        .unwrap();
        assert_eq!(s.contact("Marc@X.com").unwrap().card.title, "CFO");
    }

    #[rstest]
    fn digest_clean_body_is_served(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming(
                    "m1",
                    "t1",
                    "marc@x.com",
                    &["INBOX"],
                    now_ms(),
                    "hi\n--\nsig",
                ),
                SyncMode::Initial,
            )
            .unwrap();
        let mid = s.messages(&tid).unwrap()[0].id.clone();
        s.save_digest(
            &mid,
            "prv",
            &Ok(crate::agent::Digest {
                clean_body: "hi".into(),
                card: None,
            }),
        )
        .unwrap();
        assert_eq!(
            s.messages(&tid).unwrap()[0].body_clean.as_deref(),
            Some("hi")
        );
    }

    #[rstest]
    fn digested_message_is_no_longer_pending(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        let mid = s.messages(&tid).unwrap()[0].id.clone();
        s.save_digest(&mid, "prv", &Err(Error::other("boom")))
            .unwrap();
        assert!(s.pending_digests(&env.acc, 0, 10).unwrap().is_empty());
    }

    #[rstest]
    fn wildcard_routing_picks_first_provider(env: Env) {
        let s = &env.store;
        s.save_provider(&crate::agent::Provider {
            id: "p1".into(),
            name: "Work".into(),
            kind: "anthropic".into(),
            base_url: None,
            model: "claude-opus-5-5".into(),
            auth: "key".into(),
            profile: None,
        })
        .unwrap();
        s.set_account_providers(&env.acc, &["*".into()]).unwrap();
        assert_eq!(s.provider_for_account(&env.acc).unwrap().unwrap().id, "p1");
    }

    #[rstest]
    fn no_routing_means_no_provider(env: Env) {
        let s = &env.store;
        s.save_provider(&crate::agent::Provider {
            id: "p1".into(),
            name: "Work".into(),
            kind: "anthropic".into(),
            base_url: None,
            model: "m".into(),
            auth: "key".into(),
            profile: None,
        })
        .unwrap();
        assert!(s.provider_for_account(&env.acc).unwrap().is_none());
    }

    #[rstest]
    fn participant_filter_finds_sender_threads(env: Env) {
        let s = &env.store;
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
            SyncMode::Initial,
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m2", "t2", "lea@y.com", &["INBOX"], now_ms(), "hi"),
            SyncMode::Initial,
        )
        .unwrap();
        let f = ThreadFilter {
            participants: Some(vec!["MARC@x.com".into()]),
            ..Default::default()
        };
        assert_eq!(s.threads(&f).unwrap().1, 1);
    }

    #[rstest]
    #[case::comma_order("Delobelle, Anne-Charlotte", "anne charlotte delobelle")]
    #[case::accents("Léa Dubois", "lea dubois")]
    #[case::spaces("  Marc   Petit ", "marc petit")]
    fn person_keys(#[case] input: &str, #[case] want: &str) {
        assert_eq!(person_key(input), want);
    }

    #[rstest]
    #[case::noreply("no-reply@bank.example", true)]
    #[case::notifications("notifications@github.com", true)]
    #[case::person("marc@acme.example", false)]
    fn automated_senders(#[case] email: &str, #[case] want: bool) {
        assert_eq!(is_automated(email), want);
    }

    #[rstest]
    fn same_person_two_addresses_is_one_entry(env: Env) {
        let s = &env.store;
        let mut a = incoming("m1", "t1", "marc@work.example", &["INBOX"], now_ms(), "hi");
        a.parsed.from = Some(Address {
            name: "Marc Petit".into(),
            email: "marc@work.example".into(),
        });
        let mut b = incoming("m2", "t2", "marc@home.example", &["INBOX"], now_ms(), "hi");
        b.parsed.from = Some(Address {
            name: "Petit, Marc".into(),
            email: "marc@home.example".into(),
        });
        // Replies make them correspondents (not just senders).
        let mut r1 = incoming("m3", "t1", "me@example.com", &["SENT"], now_ms(), "re");
        r1.parsed.to = vec![Address {
            name: "Marc Petit".into(),
            email: "marc@work.example".into(),
        }];
        let mut r2 = incoming("m4", "t2", "me@example.com", &["SENT"], now_ms(), "re");
        r2.parsed.to = vec![Address {
            name: "Marc".into(),
            email: "marc@home.example".into(),
        }];
        for m in [a, b, r1, r2] {
            s.ingest(&env.acc, "me@example.com", &m, SyncMode::Initial)
                .unwrap();
        }
        assert_eq!(s.people(None, 50).unwrap().len(), 1);
    }

    #[rstest]
    #[case::western("Bob Durand", "durand")]
    #[case::caps_family_first("NAGEOTTE Pierre-Etienne (75)", "nageotte")]
    #[case::suffix_ignored("Amanda (Airbnb)", "amanda")]
    #[case::comma("Delobelle, Anne-Charlotte", "delobelle")]
    #[case::digits_kept("ICASP 12", "12")]
    #[case::company_suffix("Kourouniotis Bros S.A.", "bros")]
    fn family_name_for_sorting(#[case] name: &str, #[case] want: &str) {
        assert_eq!(sort_key(name).2, want);
    }

    #[rstest]
    fn digit_names_sort_after_letters() {
        assert!(sort_key("09filali") > sort_key("Zoé Martin"));
    }

    #[rstest]
    fn people_are_sorted_by_family_name(env: Env) {
        let s = &env.store;
        for (i, (name, email)) in [
            ("Zoé Martin", "z@x.com"),
            ("Émile Abel", "e@x.com"),
            ("Bob Durand", "b@x.com"),
        ]
        .iter()
        .enumerate()
        {
            let mut m = incoming(
                &format!("s{i}"),
                &format!("t{i}"),
                "me@example.com",
                &["SENT"],
                now_ms(),
                "hi",
            );
            m.parsed.to = vec![Address {
                name: name.to_string(),
                email: email.to_string(),
            }];
            s.ingest(&env.acc, "me@example.com", &m, SyncMode::Initial)
                .unwrap();
        }
        let names: Vec<String> = s
            .people(None, 10)
            .unwrap()
            .into_iter()
            .map(|p| p.card.name)
            .collect();
        assert_eq!(names, vec!["Émile Abel", "Bob Durand", "Zoé Martin"]);
    }

    #[rstest]
    fn conversation_count_matches_their_threads(env: Env) {
        let s = &env.store;
        // Marc writes in t1 and I reply; in t2 I write first and get no answer.
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "a"),
            SyncMode::Initial,
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m2", "t1", "marc@x.com", &["INBOX"], now_ms(), "b"),
            SyncMode::Initial,
        )
        .unwrap();
        for (id, t) in [("s1", "t1"), ("s2", "t2")] {
            let mut m = incoming(id, t, "me@example.com", &["SENT"], now_ms(), "re");
            m.parsed.to = vec![Address {
                name: "Marc Petit".into(),
                email: "marc@x.com".into(),
            }];
            s.ingest(&env.acc, "me@example.com", &m, SyncMode::Initial)
                .unwrap();
        }
        let person = s.person("marc@x.com").unwrap();
        let f = ThreadFilter {
            participants: Some(person.emails.clone()),
            ..Default::default()
        };
        assert_eq!((person.conversations, s.threads(&f).unwrap().1), (2, 2));
    }

    #[rstest]
    fn ai_groups_merge_and_rename_people(env: Env) {
        let s = &env.store;
        for (i, (name, email)) in [
            ("lieber adele", "adele@x.com"),
            ("Adèle L.", "a.lieber@y.com"),
        ]
        .iter()
        .enumerate()
        {
            let mut m = incoming(
                &format!("s{i}"),
                &format!("t{i}"),
                "me@example.com",
                &["SENT"],
                now_ms(),
                "hi",
            );
            m.parsed.to = vec![Address {
                name: name.to_string(),
                email: email.to_string(),
            }];
            s.ingest(&env.acc, "me@example.com", &m, SyncMode::Initial)
                .unwrap();
        }
        let people = s.people(None, 10).unwrap();
        let ids = people.iter().map(|p| p.email.clone()).collect();
        s.apply_person_groups(
            &people,
            &[crate::agent::PersonGroup {
                ids,
                name: "Adèle Lieber".into(),
                kind: "person".into(),
            }],
        )
        .unwrap();
        let names: Vec<String> = s
            .people(None, 10)
            .unwrap()
            .into_iter()
            .map(|p| p.card.name)
            .collect();
        assert_eq!(names, vec!["Adèle Lieber"]);
    }

    #[rstest]
    fn deleting_a_project_frees_its_threads(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        let p = s.create_project("test", None).unwrap();
        s.update_threads(
            std::slice::from_ref(&tid),
            &ThreadPatch {
                add_projects: vec![p.id.clone()],
                ..Default::default()
            },
        )
        .unwrap();
        s.delete_project(&p.id).unwrap();
        assert!(s.thread(&tid).unwrap().projects.is_empty());
    }

    #[rstest]
    fn renaming_to_an_existing_name_is_refused(env: Env) {
        let s = &env.store;
        s.create_project("Hiring", None).unwrap();
        let p = s.create_project("test", None).unwrap();
        assert!(s.update_project(&p.id, Some("Hiring"), None).is_err());
    }

    #[rstest]
    fn renaming_updates_the_name(env: Env) {
        let s = &env.store;
        let p = s.create_project("test", None).unwrap();
        assert_eq!(
            s.update_project(&p.id, Some("Q4 audit"), None)
                .unwrap()
                .name,
            "Q4 audit"
        );
    }

    #[rstest]
    fn senders_never_answered_are_not_people(env: Env) {
        let s = &env.store;
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming(
                "m1",
                "t1",
                "promo@shop.example",
                &["INBOX"],
                now_ms(),
                "sale",
            ),
            SyncMode::Initial,
        )
        .unwrap();
        assert!(s.people(None, 50).unwrap().is_empty());
    }

    /// Three threads: "old" started first but had the latest reply.
    fn ordered_threads(env: &Env) {
        let s = &env.store;
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "old", "a@x.com", &["INBOX"], 1_000 * DAY, "a"),
            SyncMode::Incremental,
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m2", "mid", "b@x.com", &["INBOX"], 1_001 * DAY, "b"),
            SyncMode::Incremental,
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m3", "new", "c@x.com", &["INBOX"], 1_002 * DAY, "c"),
            SyncMode::Incremental,
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m4", "old", "a@x.com", &["INBOX"], 1_003 * DAY, "reply"),
            SyncMode::Incremental,
        )
        .unwrap();
    }

    fn order_of(env: &Env, order: &str, reverse: bool) -> Vec<String> {
        let f = ThreadFilter {
            order: Some(order.into()),
            reverse,
            ..Default::default()
        };
        env.store
            .threads(&f)
            .unwrap()
            .0
            .into_iter()
            .map(|t| t.snippet)
            .collect()
    }

    #[rstest]
    #[case::last_activity_newest_first("last_date", false, vec!["reply", "c", "b"])]
    #[case::last_activity_oldest_first("last_date", true, vec!["b", "c", "reply"])]
    #[case::created_newest_first("created", false, vec!["c", "b", "reply"])]
    #[case::created_oldest_first("created", true, vec!["reply", "b", "c"])]
    fn orderings(env: Env, #[case] order: &str, #[case] reverse: bool, #[case] want: Vec<&str>) {
        ordered_threads(&env);
        assert_eq!(order_of(&env, order, reverse), want);
    }

    #[rstest]
    #[case::urgent_first(false, vec!["urgent", "low", "none"])]
    #[case::low_first_none_still_last(true, vec!["low", "urgent", "none"])]
    fn priority_ordering(env: Env, #[case] reverse: bool, #[case] want: Vec<&str>) {
        let s = &env.store;
        for (pid, tid, prio) in [("m1", "none", 0), ("m2", "urgent", 1), ("m3", "low", 4)] {
            let id = s
                .ingest(
                    &env.acc,
                    "me@example.com",
                    &incoming(pid, tid, "a@x.com", &["INBOX"], now_ms(), tid),
                    SyncMode::Incremental,
                )
                .unwrap();
            s.update_threads(
                &[id],
                &ThreadPatch {
                    priority: Some(prio),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert_eq!(order_of(&env, "priority", reverse), want);
    }

    #[rstest]
    fn fts_rowid_orders_by_date() {
        assert!(fts_rowid(2_000, 1) > fts_rowid(1_000, 0xF_FFFF));
    }

    #[rstest]
    fn search_returns_newest_first(env: Env) {
        let s = &env.store;
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming(
                "m1",
                "t1",
                "marc@x.com",
                &["INBOX"],
                now_ms() - DAY,
                "budget old",
            ),
            SyncMode::Initial,
        )
        .unwrap();
        let newer = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m2", "t2", "marc@x.com", &["INBOX"], now_ms(), "budget new"),
                SyncMode::Initial,
            )
            .unwrap();
        assert_eq!(s.search("budget", 10, false).unwrap()[0].id, newer);
    }

    /// One email of its own thread; returns the thread id.
    fn mail(
        env: &Env,
        n: u32,
        from: (&str, &str),
        subject: &str,
        body: &str,
        age_days: i64,
    ) -> String {
        let mut m = incoming(
            &format!("rel{n}"),
            &format!("relt{n}"),
            from.1,
            &["INBOX"],
            now_ms() - age_days * DAY,
            body,
        );
        m.parsed.subject = subject.into();
        m.parsed.from = Some(Address {
            name: from.0.into(),
            email: from.1.into(),
        });
        env.store
            .ingest(&env.acc, "me@example.com", &m, SyncMode::Initial)
            .unwrap()
    }

    /// A small mailbox where "french bee" means the airline, and other mail has the
    /// words apart, as parts of other words, or in a less telling place.
    struct Relevance {
        env: Env,
        flight: String,
        apart: String,
        in_subject: String,
        in_body: String,
    }

    #[fixture]
    fn relevance(env: Env) -> Relevance {
        let flight = mail(
            &env,
            1,
            ("French Bee", "noreply@frenchbee.example"),
            "Your booking",
            "Your French Bee flight to Papeete is confirmed.",
            30,
        );
        let apart = mail(
            &env,
            2,
            ("Ada Lind", "ada@corp.example"),
            "Garden",
            "The bee hotel is ready; the french doors are painted.",
            2,
        );
        mail(
            &env,
            3,
            ("Georg Kargel", "georg@bank.example"),
            "Bank transfer",
            "We have been to the French desk about your transfer.",
            1,
        );
        let in_subject = mail(
            &env,
            4,
            ("Nora", "nora@agency.example"),
            "Invoice for October",
            "Please find it attached.",
            20,
        );
        let in_body = mail(
            &env,
            5,
            ("Marc", "marc@agency.example"),
            "Hello",
            "The invoice will follow next week, after the meeting.",
            1,
        );
        Relevance {
            env,
            flight,
            apart,
            in_subject,
            in_body,
        }
    }

    fn ids(r: &Relevance, q: &str, typing: bool) -> Vec<String> {
        r.env
            .store
            .search(q, 10, typing)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect()
    }

    #[rstest]
    fn every_word_must_match(relevance: Relevance) {
        assert_eq!(ids(&relevance, "french bee", false).len(), 2);
    }

    #[rstest]
    fn a_word_inside_another_does_not_match(relevance: Relevance) {
        // "been" is not "bee": the bank mail stays out.
        assert!(
            !ids(&relevance, "bee french", false)
                .iter()
                .any(|i| i != &relevance.flight && i != &relevance.apart)
        );
    }

    #[rstest]
    fn the_phrase_ranks_first(relevance: Relevance) {
        // Older, but the words are in a row.
        assert_eq!(ids(&relevance, "french bee", false)[0], relevance.flight);
    }

    #[rstest]
    fn words_apart_still_match(relevance: Relevance) {
        assert_eq!(ids(&relevance, "french bee", false)[1], relevance.apart);
    }

    #[rstest]
    fn the_subject_ranks_above_the_body(relevance: Relevance) {
        assert_eq!(
            ids(&relevance, "invoice", false),
            vec![relevance.in_subject.clone(), relevance.in_body.clone()]
        );
    }

    #[rstest]
    #[case::domain("frenchbee")]
    #[case::address("noreply@frenchbee.example")]
    #[case::sender_name("bee")]
    fn the_sender_is_searchable_without_from(relevance: Relevance, #[case] q: &str) {
        assert!(ids(&relevance, q, false).contains(&relevance.flight));
    }

    #[rstest]
    fn case_and_accents_do_not_matter(relevance: Relevance) {
        assert_eq!(
            ids(&relevance, "PAPÉETE", false),
            vec![relevance.flight.clone()]
        );
    }

    #[rstest]
    fn an_accented_query_finds_plain_text(relevance: Relevance) {
        assert_eq!(
            ids(&relevance, "papéete", false),
            vec![relevance.flight.clone()]
        );
    }

    #[rstest]
    fn the_word_being_typed_is_a_prefix(relevance: Relevance) {
        assert_eq!(
            ids(&relevance, "papee", true),
            vec![relevance.flight.clone()]
        );
    }

    #[rstest]
    fn a_finished_word_is_not_a_prefix(relevance: Relevance) {
        assert!(ids(&relevance, "papee", false).is_empty());
    }

    #[rstest]
    fn search_ignores_accents(env: Env) {
        env.store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming(
                    "m1",
                    "t1",
                    "marc@x.com",
                    &["INBOX"],
                    now_ms(),
                    "réunion demain",
                ),
                SyncMode::Initial,
            )
            .unwrap();
        assert_eq!(env.store.search("reunion", 10, false).unwrap().len(), 1);
    }

    #[rstest]
    fn update_status_queues_label_push(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        s.update_threads(
            &[tid],
            &ThreadPatch {
                status: Some("todo".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(s.pending_ops(10).unwrap()[0].kind, "labels");
    }

    #[rstest]
    fn unknown_status_is_rejected(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        assert!(matches!(
            s.update_threads(
                &[tid],
                &ThreadPatch {
                    status: Some("nope".into()),
                    ..Default::default()
                }
            ),
            Err(Error::InvalidParams(_))
        ));
    }

    #[rstest]
    fn resolve_by_number(env: Env) {
        let tid = env
            .store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        assert_eq!(env.store.resolve_thread_id("M-1").unwrap(), tid);
    }

    #[rstest]
    fn reject_restores_previous_status(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        s.update_threads(
            std::slice::from_ref(&tid),
            &ThreadPatch {
                status: Some("todo".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let d = Draft {
            id: new_id("dft"),
            account_id: env.acc.clone(),
            thread_id: Some(tid.clone()),
            to: vec![],
            cc: vec![],
            subject: "".into(),
            body: "b".into(),
            state: "draft".into(),
            origin: "agent".into(),
            created: 0,
            updated: 0,
            attachments: vec![],
            reply_to_message_id: None,
        };
        s.create_draft(&d).unwrap();
        s.move_to_review(&d.id).unwrap();
        s.reject_draft(&d.id).unwrap();
        assert_eq!(s.thread(&tid).unwrap().status, "todo");
    }

    #[rstest]
    fn reply_defaults_target_sender(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        assert_eq!(
            s.reply_defaults(&tid, "me@example.com").unwrap().0,
            vec!["marc@x.com".to_string()]
        );
    }

    #[rstest]
    fn remote_archive_marks_archive(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &[], now_ms(), "hi"),
            SyncMode::Incremental,
        )
        .unwrap();
        assert_eq!(s.thread(&tid).unwrap().status, "archive");
    }

    #[rstest]
    fn superseded_op_survives_done(env: Env) {
        let s = &env.store;
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        s.update_threads(
            std::slice::from_ref(&tid),
            &ThreadPatch {
                status: Some("todo".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let in_flight = s.pending_ops(10).unwrap().remove(0);
        s.update_threads(
            &[tid],
            &ThreadPatch {
                status: Some("waiting".into()),
                ..Default::default()
            },
        )
        .unwrap();
        s.op_done(&in_flight).unwrap();
        assert_eq!(s.pending_ops(10).unwrap().len(), 1);
    }

    #[rstest]
    fn label_filter_finds_filed_thread(env: Env) {
        let s = &env.store;
        s.set_labels(&env.acc, &[("Label_7".into(), "Clients/Acme".into())])
            .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &["Label_7"], now_ms(), "hi"),
            SyncMode::Initial,
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m2", "t2", "marc@x.com", &["INBOX"], now_ms(), "hi"),
            SyncMode::Initial,
        )
        .unwrap();
        let f = ThreadFilter {
            label_id: Some("Label_7".into()),
            ..Default::default()
        };
        assert_eq!(s.threads(&f).unwrap().1, 1);
    }

    #[rstest]
    fn user_labels_hide_system_and_mermoz(env: Env) {
        let s = &env.store;
        s.set_labels(
            &env.acc,
            &[
                ("INBOX".into(), "INBOX".into()),
                ("Label_1".into(), "Mermoz/Todo".into()),
                ("Label_2".into(), "Receipts".into()),
            ],
        )
        .unwrap();
        let names: Vec<String> = s
            .user_labels()
            .unwrap()
            .into_iter()
            .map(|l| l.name)
            .collect();
        assert_eq!(names, vec!["Receipts".to_string()]);
    }

    #[rstest]
    fn counts_exclude_archive(env: Env) {
        let s = &env.store;
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &[], now_ms(), "hi"),
            SyncMode::Initial,
        )
        .unwrap();
        assert_eq!(s.counts(false).unwrap()["status"].get("archive"), None);
    }

    fn unread_thread(env: &Env) -> String {
        env.store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming(
                    "m1",
                    "t1",
                    "marc@x.com",
                    &["INBOX", "UNREAD"],
                    now_ms(),
                    "hi",
                ),
                SyncMode::Incremental,
            )
            .unwrap()
    }

    fn archive_suggestion() -> Suggestion {
        Suggestion {
            status: Some("archive".into()),
            projects: vec!["Receipts".into()],
            ..Default::default()
        }
    }

    #[rstest]
    fn reading_applies_suggested_status(env: Env) {
        let tid = unread_thread(&env);
        env.store
            .save_suggestion(&tid, &archive_suggestion(), "jev", now_ms())
            .unwrap();
        env.store.mark_read(&tid, true).unwrap();
        assert_eq!(env.store.thread(&tid).unwrap().status, "archive");
    }

    #[rstest]
    fn reading_applies_suggested_projects(env: Env) {
        let tid = unread_thread(&env);
        env.store
            .save_suggestion(&tid, &archive_suggestion(), "jev", now_ms())
            .unwrap();
        env.store.mark_read(&tid, true).unwrap();
        assert_eq!(
            env.store.thread(&tid).unwrap().projects,
            vec!["Receipts".to_string()]
        );
    }

    #[rstest]
    fn read_suggestion_is_no_longer_pending(env: Env) {
        let tid = unread_thread(&env);
        env.store
            .save_suggestion(&tid, &archive_suggestion(), "jev", now_ms())
            .unwrap();
        env.store.mark_read(&tid, true).unwrap();
        assert_eq!(env.store.thread(&tid).unwrap().suggestion, None);
    }

    #[rstest]
    fn reading_without_suggestion_is_todo(env: Env) {
        let tid = unread_thread(&env);
        env.store.mark_read(&tid, true).unwrap();
        assert_eq!(env.store.thread(&tid).unwrap().status, "todo");
    }

    #[rstest]
    fn reading_after_new_mail_resumes_in_progress(env: Env) {
        let s = &env.store;
        let tid = unread_thread(&env);
        s.update_threads(
            std::slice::from_ref(&tid),
            &ThreadPatch {
                status: Some("in_progress".into()),
                ..Default::default()
            },
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming(
                "m2",
                "t1",
                "marc@x.com",
                &["INBOX", "UNREAD"],
                now_ms(),
                "more",
            ),
            SyncMode::Incremental,
        )
        .unwrap();
        s.mark_read(&tid, true).unwrap();
        assert_eq!(s.thread(&tid).unwrap().status, "in_progress");
    }

    #[rstest]
    fn marking_unread_moves_back_to_unread(env: Env) {
        let tid = unread_thread(&env);
        env.store.mark_read(&tid, true).unwrap();
        env.store.mark_read(&tid, false).unwrap();
        assert_eq!(env.store.thread(&tid).unwrap().status, "unread");
    }

    #[rstest]
    fn accepting_a_suggestion_marks_read(env: Env) {
        let tid = unread_thread(&env);
        env.store
            .save_suggestion(&tid, &archive_suggestion(), "jev", now_ms())
            .unwrap();
        env.store
            .accept_suggestions(std::slice::from_ref(&tid))
            .unwrap();
        assert!(!env.store.thread(&tid).unwrap().unread);
    }

    #[rstest]
    fn routed_thread_waits_for_its_next_message(env: Env) {
        let tid = unread_thread(&env);
        let last = env.store.thread(&tid).unwrap().last_date;
        env.store
            .save_suggestion(&tid, &archive_suggestion(), "jev", last)
            .unwrap();
        assert!(env.store.threads_to_route(10).unwrap().is_empty());
    }

    #[rstest]
    fn gmail_label_is_a_project(env: Env) {
        let s = &env.store;
        s.set_labels(&env.acc, &[("Label_7".into(), "Immo/Houlgate".into())])
            .unwrap();
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["Label_7"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        assert_eq!(
            s.thread(&tid).unwrap().projects,
            vec!["Immo/Houlgate".to_string()]
        );
    }

    #[rstest]
    fn project_filter_finds_labelled_thread(env: Env) {
        let s = &env.store;
        s.set_labels(&env.acc, &[("Label_7".into(), "Immo".into())])
            .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &["Label_7"], now_ms(), "hi"),
            SyncMode::Initial,
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m2", "t2", "marc@x.com", &["INBOX"], now_ms(), "hi"),
            SyncMode::Initial,
        )
        .unwrap();
        let f = ThreadFilter {
            project: Some("Immo".into()),
            ..Default::default()
        };
        assert_eq!(s.threads(&f).unwrap().1, 1);
    }

    #[rstest]
    fn removing_a_project_added_offline_queues_only_the_removal(env: Env) {
        let s = &env.store;
        let tid = unread_thread(&env);
        s.update_threads(
            std::slice::from_ref(&tid),
            &ThreadPatch {
                add_projects: vec!["P".into()],
                ..Default::default()
            },
        )
        .unwrap();
        s.update_threads(
            std::slice::from_ref(&tid),
            &ThreadPatch {
                remove_projects: vec!["P".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let op = s
            .pending_ops(10)
            .unwrap()
            .into_iter()
            .find(|o| o.kind == "projects")
            .unwrap();
        assert_eq!(
            op.payload,
            serde_json::json!({ "add": [], "remove": ["P"] })
        );
    }

    #[rstest]
    fn server_update_keeps_unpushed_project(env: Env) {
        let s = &env.store;
        let tid = unread_thread(&env);
        s.update_threads(
            std::slice::from_ref(&tid),
            &ThreadPatch {
                add_projects: vec!["P".into()],
                ..Default::default()
            },
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
            SyncMode::Incremental,
        )
        .unwrap();
        assert_eq!(s.thread(&tid).unwrap().projects, vec!["P".to_string()]);
    }

    #[rstest]
    fn remote_trash_marks_deleted(env: Env) {
        let s = &env.store;
        let tid = unread_thread(&env);
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &["TRASH"], now_ms(), "hi"),
            SyncMode::Incremental,
        )
        .unwrap();
        assert_eq!(s.thread(&tid).unwrap().status, "deleted");
    }

    #[rstest]
    fn renaming_a_project_renames_its_sub_projects(env: Env) {
        let s = &env.store;
        s.set_labels(
            &env.acc,
            &[
                ("Label_1".into(), "Immo".into()),
                ("Label_2".into(), "Immo/Houlgate".into()),
            ],
        )
        .unwrap();
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["Label_2"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap();
        s.update_project("Immo", Some("Logement"), None).unwrap();
        assert_eq!(
            s.thread(&tid).unwrap().projects,
            vec!["Logement/Houlgate".to_string()]
        );
    }

    #[rstest]
    fn stored_sender_failures_are_checked_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let c = Connection::open(&path).unwrap();
        let before = MIGRATIONS
            .iter()
            .position(|m| *m == RECHECK_SENDER_FAILS)
            .unwrap();
        for (i, step) in MIGRATIONS.iter().take(before).enumerate() {
            c.execute_batch(&format!(
                "BEGIN; {step}; PRAGMA user_version = {}; COMMIT;",
                i + 1
            ))
            .unwrap();
        }
        c.execute_batch(
            "INSERT INTO accounts (id, email, provider, created) VALUES ('acc', 'me@example.com', 'gmail', 0);
             INSERT INTO threads (id, account_id, provider_thread_id, number, status, unread) VALUES ('t1', 'acc', 'p1', 1, 'todo', 0);
             INSERT INTO messages (id, account_id, provider_id, thread_id, from_name, from_email, to_json, cc_json, date, subject, body_text, labels_json, is_from_me, unread, auth)
               VALUES ('m1', 'acc', 'g1', 't1', 'Ann', 'ann@mail.example', '[]', '[]', 0, 'Hi', '', '[]', 0, 0, 'fail');",
        )
        .unwrap();
        drop(c);
        let store = Store::open(&path, &dir.path().join("blobs")).unwrap();
        assert_eq!(store.unverified_senders("t1").unwrap().len(), 1);
    }

    /// A store last opened before Workflow v2, with one thread per old status.
    fn v1_store(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("db.sqlite");
        let c = Connection::open(&path).unwrap();
        for (i, step) in MIGRATIONS.iter().take(6).enumerate() {
            c.execute_batch(&format!(
                "BEGIN; {step}; PRAGMA user_version = {}; COMMIT;",
                i + 1
            ))
            .unwrap();
        }
        c.execute_batch(
            "INSERT INTO accounts (id, email, provider, created) VALUES ('acc', 'me@example.com', 'gmail', 0);
             INSERT INTO threads (id, account_id, provider_thread_id, number, status, unread) VALUES
               ('t_done', 'acc', 'p1', 1, 'done', 0), ('t_cancel', 'acc', 'p2', 2, 'canceled', 0),
               ('t_triage_unread', 'acc', 'p3', 3, 'triage', 1), ('t_triage_read', 'acc', 'p4', 4, 'triage', 0),
               ('t_backlog', 'acc', 'p5', 5, 'backlog', 1);",
        )
        .unwrap();
        path
    }

    #[rstest]
    #[case::done_becomes_archive("t_done", "archive")]
    #[case::canceled_becomes_archive("t_cancel", "archive")]
    #[case::unread_triage_becomes_unread("t_triage_unread", "unread")]
    #[case::read_triage_becomes_todo("t_triage_read", "todo")]
    #[case::backlog_stays("t_backlog", "backlog")]
    fn v2_migration_maps_old_statuses(#[case] thread: &str, #[case] want: &str) {
        let dir = tempfile::tempdir().unwrap();
        let path = v1_store(&dir);
        let store = Store::open(&path, &dir.path().join("blobs")).unwrap();
        assert_eq!(store.thread(thread).unwrap().status, want);
    }

    #[rstest]
    fn filing_an_unread_thread_survives_the_server_echo(env: Env) {
        let s = &env.store;
        let tid = unread_thread(&env);
        s.update_threads(
            std::slice::from_ref(&tid),
            &ThreadPatch {
                status: Some("todo".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let op = s.pending_ops(10).unwrap().remove(0);
        s.op_done(&op).unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming(
                "m1",
                "t1",
                "marc@x.com",
                &["INBOX", "UNREAD"],
                now_ms(),
                "hi",
            ),
            SyncMode::Incremental,
        )
        .unwrap();
        assert_eq!(s.thread(&tid).unwrap().status, "todo");
    }

    #[rstest]
    fn reading_on_another_device_leaves_unread(env: Env) {
        let s = &env.store;
        let tid = unread_thread(&env);
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
            SyncMode::Incremental,
        )
        .unwrap();
        assert_eq!(s.thread(&tid).unwrap().status, "todo");
    }

    #[rstest]
    fn route_input_counts_where_the_sender_was_filed(env: Env) {
        let s = &env.store;
        s.set_labels(&env.acc, &[("Label_7".into(), "Immo".into())])
            .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming(
                "m1",
                "t1",
                "marc@x.com",
                &["Label_7"],
                now_ms() - DAY,
                "old",
            ),
            SyncMode::Initial,
        )
        .unwrap();
        let tid = s
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming(
                    "m2",
                    "t2",
                    "marc@x.com",
                    &["INBOX", "UNREAD"],
                    now_ms(),
                    "new",
                ),
                SyncMode::Initial,
            )
            .unwrap();
        assert_eq!(
            s.route_input(&tid).unwrap().sender_history,
            vec![("Immo".to_string(), 1)]
        );
    }

    #[rstest]
    fn project_description_names_its_correspondents(env: Env) {
        let s = &env.store;
        s.set_labels(&env.acc, &[("Label_7".into(), "Immo".into())])
            .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &["Label_7"], now_ms(), "hi"),
            SyncMode::Initial,
        )
        .unwrap();
        assert_eq!(
            s.project_descriptions().unwrap(),
            vec![(
                "Immo".to_string(),
                "Folder 'Immo'. Frequent correspondents: Marc Petit.".to_string()
            )]
        );
    }

    fn status_keys(s: &Store) -> Vec<String> {
        s.statuses().unwrap().into_iter().map(|x| x.key).collect()
    }

    #[rstest]
    fn statuses_keep_the_users_order(env: Env) {
        let mut keys = status_keys(&env.store);
        keys.swap(1, 4);
        env.store.reorder_statuses(&keys).unwrap();
        assert_eq!(status_keys(&env.store), keys);
    }

    #[rstest]
    fn reorder_must_list_every_status(env: Env) {
        assert!(env.store.reorder_statuses(&["todo".to_string()]).is_err());
    }

    #[rstest]
    fn a_forward_shows_the_forwarded_email(env: Env) {
        let mut m = incoming(
            "m1",
            "t1",
            "aude@x.com",
            &["INBOX"],
            now_ms(),
            "Cela t'intéresse ?\n\nAude\n\nDe : Hub Finance <hub@x.com>\nEnvoyé : mardi 6 octobre 2026\nÀ : Aude\nObjet : Event\n\nWe warmly invite you",
        );
        m.parsed.subject = "TR: Event".into();
        let tid = env
            .store
            .ingest(&env.acc, "me@example.com", &m, SyncMode::Initial)
            .unwrap();
        assert!(
            env.store.messages(&tid).unwrap()[0]
                .body_new
                .contains("We warmly invite you")
        );
    }

    #[rstest]
    fn my_sent_copy_keeps_its_blind_copies(env: Env) {
        let mut m = incoming("m1", "t1", "me@example.com", &["SENT"], now_ms(), "hi");
        m.parsed.bcc = vec![Address {
            name: "Boss".into(),
            email: "boss@x.com".into(),
        }];
        let tid = env
            .store
            .ingest(&env.acc, "me@example.com", &m, SyncMode::Incremental)
            .unwrap();
        assert_eq!(
            env.store.messages(&tid).unwrap()[0].bcc[0].email,
            "boss@x.com"
        );
    }

    // ---- background column classification ------------------------------

    /// A read thread filed in `status`, last message `age_ms` ago.
    fn open_thread(env: &Env, n: &str, status: &str, age_ms: i64) -> String {
        let tid = env
            .store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming(
                    &format!("m{n}"),
                    &format!("t{n}"),
                    "marc@x.com",
                    &["INBOX"],
                    now_ms() - age_ms,
                    "hi",
                ),
                SyncMode::Initial,
            )
            .unwrap();
        env.store
            .update_threads(
                std::slice::from_ref(&tid),
                &ThreadPatch {
                    status: Some(status.into()),
                    ..Default::default()
                },
            )
            .unwrap();
        tid
    }

    fn jev_archive() -> Suggestion {
        Suggestion {
            status: Some("archive".into()),
            status_confidence: Some(0.9),
            projects: vec!["Receipts".into()],
            project_confidence: Some(0.9),
            ..Default::default()
        }
    }

    fn classify(env: &Env, tid: &str, sg: &Suggestion) -> bool {
        let last = env.store.thread(tid).unwrap().last_date;
        env.store
            .save_column_suggestion(tid, sg, "jev", last)
            .unwrap()
    }

    /// Every thread that may get a suggestion now.
    fn to_classify(env: &Env) -> Vec<String> {
        let all: Vec<String> = {
            let r = env.store.r.lock().unwrap();
            let mut st = r.prepare("SELECT id FROM threads").unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        env.store
            .suggest_candidates(&all)
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect()
    }

    // ---- stale threads never keep a live status suggestion -------------

    const TEN_YEARS_MS: i64 = 3650 * crate::jev::DAY_MS;

    fn jev_says(status: &str, project: Option<&str>) -> Suggestion {
        Suggestion {
            status: Some(status.into()),
            status_confidence: Some(0.9),
            projects: project.into_iter().map(String::from).collect(),
            project_confidence: project.map(|_| 0.9),
            ..Default::default()
        }
    }

    /// A Backlog thread last written to `age_ms` ago, holding Jev's stored verdict.
    fn suggested(env: &Env, age_ms: i64, sg: &Suggestion) -> String {
        let tid = open_thread(env, "1", "backlog", age_ms);
        classify(env, &tid, sg);
        tid
    }

    fn pending(env: &Env, tid: &str) -> Option<Suggestion> {
        env.store.thread(tid).unwrap().suggestion
    }

    #[rstest]
    fn stale_waiting_suggestion_loses_its_status(env: Env) {
        let tid = suggested(&env, TEN_YEARS_MS, &jev_says("waiting", Some("Receipts")));
        env.store.drop_stale_suggestions(now_ms()).unwrap();
        assert_eq!(pending(&env, &tid).unwrap().status, None);
    }

    #[rstest]
    fn stale_suggestion_keeps_its_project(env: Env) {
        let tid = suggested(&env, TEN_YEARS_MS, &jev_says("todo", Some("Receipts")));
        env.store.drop_stale_suggestions(now_ms()).unwrap();
        assert_eq!(pending(&env, &tid).unwrap().projects, vec!["Receipts"]);
    }

    #[rstest]
    fn stale_suggestion_with_nothing_left_is_dropped(env: Env) {
        let tid = suggested(&env, TEN_YEARS_MS, &jev_says("waiting", None));
        env.store.drop_stale_suggestions(now_ms()).unwrap();
        assert_eq!(pending(&env, &tid), None);
    }

    #[rstest]
    fn dropped_stale_suggestion_is_not_asked_again(env: Env) {
        suggested(&env, TEN_YEARS_MS, &jev_says("waiting", None));
        env.store.drop_stale_suggestions(now_ms()).unwrap();
        assert!(to_classify(&env).is_empty());
    }

    #[rstest]
    #[case::recent_waiting(10 * crate::jev::DAY_MS, "waiting", Some("waiting"))]
    #[case::stale_archive(TEN_YEARS_MS, "archive", Some("archive"))]
    #[case::stale_done(TEN_YEARS_MS, "done", Some("done"))]
    #[case::stale_in_progress(TEN_YEARS_MS, "in_progress", None)]
    fn only_stale_live_statuses_are_dropped(
        env: Env,
        #[case] age_ms: i64,
        #[case] status: &str,
        #[case] want: Option<&str>,
    ) {
        let tid = suggested(&env, age_ms, &jev_says(status, Some("Receipts")));
        env.store.drop_stale_suggestions(now_ms()).unwrap();
        assert_eq!(pending(&env, &tid).unwrap().status.as_deref(), want);
    }

    #[rstest]
    fn accepted_stale_move_stays(env: Env) {
        let tid = suggested(&env, TEN_YEARS_MS, &jev_says("waiting", None));
        env.store
            .accept_suggestions(std::slice::from_ref(&tid))
            .unwrap();
        env.store.drop_stale_suggestions(now_ms()).unwrap();
        assert_eq!(env.store.thread(&tid).unwrap().status, "waiting");
    }

    #[rstest]
    fn dropping_counts_the_suggestions_it_changed(env: Env) {
        suggested(&env, TEN_YEARS_MS, &jev_says("waiting", None));
        assert_eq!(env.store.drop_stale_suggestions(now_ms()).unwrap(), 1);
    }

    #[rstest]
    fn reopening_the_store_drops_stale_live_suggestions(env: Env) {
        let tid = suggested(&env, TEN_YEARS_MS, &jev_says("waiting", Some("Receipts")));
        let dir = env._dir.path();
        let reopened = Store::open(&dir.join("db.sqlite"), &dir.join("blobs")).unwrap();
        assert_eq!(
            reopened.thread(&tid).unwrap().suggestion.unwrap().status,
            None
        );
    }

    #[rstest]
    fn route_input_tells_the_last_activity(env: Env) {
        let tid = open_thread(&env, "1", "backlog", TEN_YEARS_MS);
        assert_eq!(
            env.store.route_input(&tid).unwrap().last_activity_days_ago,
            3650
        );
    }

    #[rstest]
    fn unclassified_backlog_thread_is_classified(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        assert_eq!(to_classify(&env), vec![tid]);
    }

    #[rstest]
    #[case::unread_is_the_new_mail_router_s_job("unread")]
    #[case::archive_is_terminal("archive")]
    #[case::done_is_terminal("done")]
    fn closed_and_unread_columns_are_not_classified(env: Env, #[case] status: &str) {
        open_thread(&env, "1", status, 0);
        assert!(to_classify(&env).is_empty());
    }

    #[rstest]
    fn classified_thread_is_a_ready_card(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        classify(&env, &tid, &jev_archive());
        assert!(
            env.store
                .suggest_candidates(std::slice::from_ref(&tid))
                .unwrap()[0]
                .ready
        );
    }

    #[rstest]
    fn no_change_verdict_is_not_asked_again(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        let same = Suggestion {
            status: Some("backlog".into()),
            status_confidence: Some(0.9),
            ..Default::default()
        };
        classify(&env, &tid, &same);
        assert!(to_classify(&env).is_empty());
    }

    #[rstest]
    fn new_mail_makes_the_classification_stale(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        let last = env.store.thread(&tid).unwrap().last_date;
        env.store
            .save_column_suggestion(&tid, &jev_archive(), "jev", last - 1)
            .unwrap();
        assert_eq!(to_classify(&env), vec![tid]);
    }

    #[rstest]
    fn thread_the_user_moved_after_its_suggestion_is_left_alone(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        classify(&env, &tid, &jev_archive());
        env.store
            .update_threads(
                std::slice::from_ref(&tid),
                &ThreadPatch {
                    status: Some("todo".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(to_classify(&env).is_empty());
    }

    /// The thread moves without the user deciding (e.g. a label set on another device).
    fn move_silently(env: &Env, tid: &str, status: &str) {
        env.store
            .w
            .lock()
            .unwrap()
            .execute(
                "UPDATE threads SET status = ?2 WHERE id = ?1",
                params![tid, status],
            )
            .unwrap();
    }

    #[rstest]
    fn suggestion_for_another_column_is_reclassified(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        classify(&env, &tid, &jev_archive());
        move_silently(&env, &tid, "waiting");
        assert_eq!(to_classify(&env), vec![tid]);
    }

    #[rstest]
    fn suggestion_for_another_column_is_hidden(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        classify(&env, &tid, &jev_archive());
        move_silently(&env, &tid, "waiting");
        assert_eq!(env.store.thread(&tid).unwrap().suggestion, None);
    }

    #[rstest]
    fn newest_thread_is_classified_first(env: Env) {
        open_thread(&env, "1", "backlog", 5_000);
        let newer = open_thread(&env, "2", "waiting", 0);
        assert_eq!(to_classify(&env)[0], newer);
    }

    #[rstest]
    fn confident_move_is_shown(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        assert!(classify(&env, &tid, &jev_archive()));
    }

    #[rstest]
    fn no_change_is_not_shown(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        let same = Suggestion {
            status: Some("backlog".into()),
            status_confidence: Some(0.9),
            ..Default::default()
        };
        classify(&env, &tid, &same);
        assert_eq!(env.store.thread(&tid).unwrap().suggestion, None);
    }

    #[rstest]
    fn shown_suggestion_keeps_only_the_change(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        classify(&env, &tid, &jev_archive());
        assert_eq!(
            env.store.thread(&tid).unwrap().suggestion.unwrap().status,
            Some("archive".into())
        );
    }

    #[rstest]
    fn thread_closed_meanwhile_gets_no_suggestion(env: Env) {
        let tid = open_thread(&env, "1", "done", 0);
        assert!(!classify(&env, &tid, &jev_archive()));
    }

    fn two_archive_one_todo(env: &Env) -> (String, String, String) {
        let a = open_thread(env, "1", "backlog", 0);
        let b = open_thread(env, "2", "backlog", 1_000);
        let c = open_thread(env, "3", "backlog", 2_000);
        classify(env, &a, &jev_archive());
        classify(env, &b, &jev_archive());
        let todo = Suggestion {
            status: Some("todo".into()),
            status_confidence: Some(0.8),
            ..Default::default()
        };
        classify(env, &c, &todo);
        (a, b, c)
    }

    #[rstest]
    fn column_groups_suggestions_by_target(env: Env) {
        two_archive_one_todo(&env);
        let col = env
            .store
            .column_suggestions("backlog", &ColumnScope::default())
            .unwrap();
        assert_eq!(
            col.groups,
            vec![
                SuggestionGroup {
                    status: Some("archive".into()),
                    projects: vec!["Receipts".into()],
                    count: 2
                },
                SuggestionGroup {
                    status: Some("todo".into()),
                    projects: vec![],
                    count: 1
                },
            ]
        );
    }

    #[rstest]
    fn column_counts_pending_suggestions(env: Env) {
        two_archive_one_todo(&env);
        let col = env
            .store
            .column_suggestions("backlog", &ColumnScope::default())
            .unwrap();
        assert_eq!(col.pending, 3);
    }

    #[rstest]
    fn column_scope_limits_to_a_project(env: Env) {
        two_archive_one_todo(&env);
        let scope = ColumnScope {
            project: Some("Elsewhere".into()),
            account_id: None,
        };
        let col = env.store.column_suggestions("backlog", &scope).unwrap();
        assert_eq!(col.pending, 0);
    }

    #[rstest]
    fn accepting_a_group_moves_only_that_group(env: Env) {
        let (_, _, c) = two_archive_one_todo(&env);
        let target = SuggestionGroup {
            status: Some("archive".into()),
            projects: vec!["Receipts".into()],
            count: 0,
        };
        env.store
            .accept_column("backlog", &ColumnScope::default(), Some(&target))
            .unwrap();
        assert_eq!(env.store.thread(&c).unwrap().status, "backlog");
    }

    #[rstest]
    fn accepting_a_group_applies_its_target(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        let target = SuggestionGroup {
            status: Some("archive".into()),
            projects: vec!["Receipts".into()],
            count: 0,
        };
        env.store
            .accept_column("backlog", &ColumnScope::default(), Some(&target))
            .unwrap();
        assert_eq!(env.store.thread(&a).unwrap().status, "archive");
    }

    #[rstest]
    fn accepting_all_returns_every_thread_as_it_was(env: Env) {
        two_archive_one_todo(&env);
        let before = env
            .store
            .accept_column("backlog", &ColumnScope::default(), None)
            .unwrap();
        let statuses: Vec<&str> = before.iter().map(|t| t.status.as_str()).collect();
        assert_eq!(statuses, vec!["backlog"; 3]);
    }

    #[rstest]
    fn restoring_puts_the_status_back(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        let before = env
            .store
            .accept_column("backlog", &ColumnScope::default(), None)
            .unwrap();
        env.store
            .restore_threads(&before.iter().map(ThreadState::from).collect::<Vec<_>>())
            .unwrap();
        assert_eq!(env.store.thread(&a).unwrap().status, "backlog");
    }

    #[rstest]
    fn restoring_removes_the_accepted_project(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        let before = env
            .store
            .accept_column("backlog", &ColumnScope::default(), None)
            .unwrap();
        env.store
            .restore_threads(&before.iter().map(ThreadState::from).collect::<Vec<_>>())
            .unwrap();
        assert!(env.store.thread(&a).unwrap().projects.is_empty());
    }

    #[rstest]
    fn restored_thread_does_not_get_the_suggestion_again(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        let before = env
            .store
            .accept_column("backlog", &ColumnScope::default(), None)
            .unwrap();
        env.store
            .restore_threads(&before.iter().map(ThreadState::from).collect::<Vec<_>>())
            .unwrap();
        assert!(!to_classify(&env).contains(&a));
    }

    #[rstest]
    fn background_calls_add_up_over_the_day(env: Env) {
        env.store.add_background_calls(3).unwrap();
        env.store.add_background_calls(4).unwrap();
        assert_eq!(env.store.background_calls_today().unwrap(), 7);
    }

    #[rstest]
    fn provider_defaults_to_typesafe(env: Env) {
        assert_eq!(
            env.store.jev_provider().unwrap(),
            crate::jev::Provider::TypeSafe
        );
    }

    #[rstest]
    fn chosen_provider_is_kept(env: Env) {
        env.store
            .set_jev_provider(crate::jev::Provider::SubQ)
            .unwrap();
        assert_eq!(
            env.store.jev_provider().unwrap(),
            crate::jev::Provider::SubQ
        );
    }

    #[rstest]
    fn provider_calls_are_counted_apart(env: Env) {
        env.store
            .add_provider_calls(crate::jev::Provider::TypeSafe, 5)
            .unwrap();
        env.store
            .add_provider_calls(crate::jev::Provider::SubQ, 2)
            .unwrap();
        assert_eq!(
            env.store
                .provider_calls_today(crate::jev::Provider::SubQ)
                .unwrap(),
            2
        );
    }

    #[rstest]
    fn daily_cap_defaults_to_a_safety_net(env: Env) {
        assert_eq!(
            env.store.background_daily_cap().unwrap(),
            crate::jev::DEFAULT_DAILY_CAP
        );
    }

    fn chat_line(tid: &str, role: &str, text: &str, created: i64) -> crate::chat::ChatMessage {
        crate::chat::ChatMessage {
            id: new_id("cht"),
            thread_id: tid.into(),
            role: role.into(),
            text: text.into(),
            actions: vec![crate::chat::ChatAction {
                kind: "status".into(),
                label: "Move to Waiting".into(),
                status: Some("waiting".into()),
                ..Default::default()
            }],
            searched: true,
            research_unavailable: false,
            created,
        }
    }

    fn one_thread(env: &Env) -> String {
        env.store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming("m1", "t1", "marc@x.com", &["INBOX"], now_ms(), "hi"),
                SyncMode::Initial,
            )
            .unwrap()
    }

    #[rstest]
    fn chat_is_kept_in_order(env: Env) {
        let tid = one_thread(&env);
        env.store
            .add_chat_message(&chat_line(&tid, "user", "What now?", 5))
            .unwrap();
        env.store
            .add_chat_message(&chat_line(&tid, "assistant", "Wait.", 5))
            .unwrap();
        let roles: Vec<String> = env
            .store
            .chat_messages(&tid)
            .unwrap()
            .into_iter()
            .map(|m| m.role)
            .collect();
        assert_eq!(roles, vec!["user", "assistant"]);
    }

    #[rstest]
    fn chat_keeps_the_proposals(env: Env) {
        let tid = one_thread(&env);
        let m = chat_line(&tid, "assistant", "Wait.", 1);
        env.store.add_chat_message(&m).unwrap();
        assert_eq!(env.store.chat_messages(&tid).unwrap()[0], m);
    }

    #[rstest]
    fn clearing_forgets_the_chat(env: Env) {
        let tid = one_thread(&env);
        env.store
            .add_chat_message(&chat_line(&tid, "user", "Q", 1))
            .unwrap();
        env.store.clear_chat(&tid).unwrap();
        assert!(env.store.chat_messages(&tid).unwrap().is_empty());
    }

    #[rstest]
    fn past_messages_come_from_threads_with_the_same_person(env: Env) {
        let s = &env.store;
        let tid = one_thread(&env);
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming(
                "o1",
                "t2",
                "marc@x.com",
                &["INBOX"],
                now_ms() - DAY,
                "older ask",
            ),
            SyncMode::Initial,
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming(
                "o2",
                "t2",
                "me@example.com",
                &["SENT"],
                now_ms() - DAY + 1,
                "My earlier answer",
            ),
            SyncMode::Initial,
        )
        .unwrap();
        assert_eq!(
            s.my_past_messages(&tid, 5).unwrap()[0].body,
            "My earlier answer"
        );
    }

    #[rstest]
    fn past_messages_do_not_cross_mailbox_consent_boundaries(env: Env) {
        let s = &env.store;
        let tid = one_thread(&env);
        let other = s.add_account("private@example.com", "gmail").unwrap();
        s.ingest(
            &other.id,
            &other.email,
            &incoming(
                "private-ask",
                "private-thread",
                "marc@x.com",
                &["INBOX"],
                now_ms() - DAY,
                "Private question",
            ),
            SyncMode::Initial,
        )
        .unwrap();
        s.ingest(
            &other.id,
            &other.email,
            &incoming(
                "private-answer",
                "private-thread",
                &other.email,
                &["SENT"],
                now_ms() - DAY + 1,
                "Private mailbox history",
            ),
            SyncMode::Initial,
        )
        .unwrap();
        assert!(s.my_past_messages(&tid, 5).unwrap().is_empty());
    }

    #[rstest]
    fn past_messages_ignore_other_people(env: Env) {
        let s = &env.store;
        let tid = one_thread(&env);
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("o1", "t2", "zoe@y.com", &["INBOX"], now_ms() - DAY, "ask"),
            SyncMode::Initial,
        )
        .unwrap();
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming(
                "o2",
                "t2",
                "me@example.com",
                &["SENT"],
                now_ms() - DAY + 1,
                "to Zoe",
            ),
            SyncMode::Initial,
        )
        .unwrap();
        assert!(s.my_past_messages(&tid, 5).unwrap().is_empty());
    }

    // ---- reviewing suggestions --------------------------------------------

    fn archive_group() -> SuggestionGroup {
        SuggestionGroup {
            status: Some("archive".into()),
            projects: vec!["Receipts".into()],
            count: 0,
        }
    }

    fn listed(env: &Env, suggested: SuggestedFilter) -> Vec<String> {
        let f = ThreadFilter {
            suggested: Some(suggested),
            status: Some(vec!["backlog".into()]),
            ..Default::default()
        };
        let mut ids: Vec<String> = env
            .store
            .threads(&f)
            .unwrap()
            .0
            .into_iter()
            .map(|t| t.id)
            .collect();
        ids.sort();
        ids
    }

    fn sorted(ids: &[&String]) -> Vec<String> {
        let mut v: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        v.sort();
        v
    }

    #[rstest]
    fn review_lists_only_one_destination(env: Env) {
        let (a, b, _) = two_archive_one_todo(&env);
        assert_eq!(
            listed(&env, SuggestedFilter::To(archive_group())),
            sorted(&[&a, &b])
        );
    }

    #[rstest]
    fn review_of_everything_lists_every_suggested_thread(env: Env) {
        let (a, b, c) = two_archive_one_todo(&env);
        assert_eq!(listed(&env, SuggestedFilter::Any), sorted(&[&a, &b, &c]));
    }

    #[rstest]
    fn review_skips_threads_without_a_change(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        let same = Suggestion {
            status: Some("backlog".into()),
            status_confidence: Some(0.9),
            ..Default::default()
        };
        classify(&env, &tid, &same);
        assert!(listed(&env, SuggestedFilter::Any).is_empty());
    }

    #[rstest]
    fn rejected_suggestion_is_hidden(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        env.store
            .reject_suggestions(std::slice::from_ref(&a))
            .unwrap();
        assert_eq!(env.store.thread(&a).unwrap().suggestion, None);
    }

    #[rstest]
    fn rejected_thread_stays_where_it_is(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        env.store
            .reject_suggestions(std::slice::from_ref(&a))
            .unwrap();
        assert_eq!(env.store.thread(&a).unwrap().status, "backlog");
    }

    #[rstest]
    fn rejected_thread_drops_out_of_its_group(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        env.store
            .reject_suggestions(std::slice::from_ref(&a))
            .unwrap();
        let col = env
            .store
            .column_suggestions("backlog", &ColumnScope::default())
            .unwrap();
        assert_eq!(col.groups[0].count, 1);
    }

    #[rstest]
    fn rejected_thread_is_not_suggested_again(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        env.store
            .reject_suggestions(std::slice::from_ref(&a))
            .unwrap();
        assert!(!to_classify(&env).contains(&a));
    }

    #[rstest]
    fn rejecting_counts_only_pending_suggestions(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        env.store
            .reject_suggestions(std::slice::from_ref(&a))
            .unwrap();
        assert_eq!(
            env.store
                .reject_suggestions(std::slice::from_ref(&a))
                .unwrap(),
            0
        );
    }

    #[rstest]
    fn reopening_a_rejection_shows_the_suggestion_again(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        env.store
            .reject_suggestions(std::slice::from_ref(&a))
            .unwrap();
        env.store
            .reopen_suggestions(std::slice::from_ref(&a))
            .unwrap();
        assert!(env.store.thread(&a).unwrap().suggestion.is_some());
    }

    #[rstest]
    fn reopening_leaves_accepted_suggestions_decided(env: Env) {
        let (a, _, _) = two_archive_one_todo(&env);
        env.store
            .accept_suggestions(std::slice::from_ref(&a))
            .unwrap();
        assert_eq!(
            env.store
                .reopen_suggestions(std::slice::from_ref(&a))
                .unwrap(),
            0
        );
    }

    // ---- draft attachments ----------------------------------------------

    struct DraftEnv {
        env: Env,
        draft: String,
        file: PathBuf,
    }

    /// A draft plus a tiny fictional file on disk to attach to it.
    #[fixture]
    fn draft_env(env: Env) -> DraftEnv {
        let d = env
            .store
            .create_draft(&Draft {
                id: new_id("dft"),
                account_id: env.acc.clone(),
                thread_id: None,
                to: vec!["marc@example.com".into()],
                cc: vec![],
                subject: "Plan".into(),
                body: "Attached.".into(),
                state: "draft".into(),
                origin: "app".into(),
                created: 0,
                updated: 0,
                attachments: vec![],
                reply_to_message_id: None,
            })
            .unwrap();
        let file = env._dir.path().join("Relevé d'été.pdf");
        std::fs::write(&file, b"%PDF-1.4 fictional").unwrap();
        DraftEnv {
            env,
            draft: d.id,
            file,
        }
    }

    fn attach_path(e: &DraftEnv, p: &Path) -> Result<DraftAttachment> {
        e.env.store.add_draft_attachment(
            &e.draft,
            &NewAttachment {
                source: AttachmentSource::Path(p.to_path_buf()),
                filename: None,
                inline: false,
            },
        )
    }

    #[rstest]
    fn attached_file_persists_with_the_draft(draft_env: DraftEnv) {
        attach_path(&draft_env, &draft_env.file).unwrap();
        assert_eq!(
            draft_env
                .env
                .store
                .draft(&draft_env.draft)
                .unwrap()
                .attachments[0]
                .filename,
            "Relevé d'été.pdf"
        );
    }

    #[rstest]
    fn attached_file_is_a_copy(draft_env: DraftEnv) {
        let a = attach_path(&draft_env, &draft_env.file).unwrap();
        std::fs::remove_file(&draft_env.file).unwrap();
        assert_eq!(
            std::fs::read(a.path.unwrap()).unwrap(),
            b"%PDF-1.4 fictional"
        );
    }

    #[rstest]
    fn attached_file_gets_its_type(draft_env: DraftEnv) {
        assert_eq!(
            attach_path(&draft_env, &draft_env.file).unwrap().mime,
            "application/pdf"
        );
    }

    #[rstest]
    fn drafts_list_carries_attachments(draft_env: DraftEnv) {
        attach_path(&draft_env, &draft_env.file).unwrap();
        assert_eq!(
            draft_env.env.store.drafts(None).unwrap()[0]
                .attachments
                .len(),
            1
        );
    }

    #[rstest]
    fn removed_attachment_leaves_the_draft(draft_env: DraftEnv) {
        let a = attach_path(&draft_env, &draft_env.file).unwrap();
        draft_env
            .env
            .store
            .remove_draft_attachment(&draft_env.draft, &a.id)
            .unwrap();
        assert!(
            draft_env
                .env
                .store
                .draft(&draft_env.draft)
                .unwrap()
                .attachments
                .is_empty()
        );
    }

    #[rstest]
    fn removed_attachment_deletes_its_copy(draft_env: DraftEnv) {
        let a = attach_path(&draft_env, &draft_env.file).unwrap();
        draft_env
            .env
            .store
            .remove_draft_attachment(&draft_env.draft, &a.id)
            .unwrap();
        assert!(!Path::new(&a.path.unwrap()).exists());
    }

    #[rstest]
    fn deleting_the_draft_deletes_its_copies(draft_env: DraftEnv) {
        let a = attach_path(&draft_env, &draft_env.file).unwrap();
        draft_env.env.store.delete_draft(&draft_env.draft).unwrap();
        assert!(!Path::new(&a.path.unwrap()).exists());
    }

    #[rstest]
    fn folders_are_refused(draft_env: DraftEnv) {
        let dir = draft_env.env._dir.path().join("Photos");
        std::fs::create_dir(&dir).unwrap();
        assert!(
            attach_path(&draft_env, &dir)
                .unwrap_err()
                .to_string()
                .contains("is a folder")
        );
    }

    #[rstest]
    fn missing_files_are_refused(draft_env: DraftEnv) {
        let missing = draft_env.env._dir.path().join("nope.pdf");
        assert!(attach_path(&draft_env, &missing).is_err());
    }

    #[rstest]
    fn files_past_the_gmail_limit_are_refused(draft_env: DraftEnv) {
        let big = draft_env.env._dir.path().join("big.bin");
        // Sparse file: the size is what matters, no 25 MB is written.
        std::fs::File::create(&big)
            .unwrap()
            .set_len(crate::compose::MAX_ATTACHMENT_BYTES + 1)
            .unwrap();
        assert!(
            attach_path(&draft_env, &big)
                .unwrap_err()
                .to_string()
                .contains("These files add up to")
        );
    }

    #[rstest]
    fn renamed_attachment_keeps_the_given_name(draft_env: DraftEnv) {
        let a = draft_env
            .env
            .store
            .add_draft_attachment(
                &draft_env.draft,
                &NewAttachment {
                    source: AttachmentSource::Path(draft_env.file.clone()),
                    filename: Some("plan.pdf".into()),
                    inline: false,
                },
            )
            .unwrap();
        assert_eq!(a.filename, "plan.pdf");
    }

    #[rstest]
    fn link_attachment_keeps_its_url(draft_env: DraftEnv) {
        draft_env
            .env
            .store
            .add_draft_attachment(
                &draft_env.draft,
                &NewAttachment {
                    source: AttachmentSource::Link("https://drive.example/f/1".into()),
                    filename: Some("Plan".into()),
                    inline: false,
                },
            )
            .unwrap();
        assert_eq!(
            draft_env
                .env
                .store
                .draft(&draft_env.draft)
                .unwrap()
                .attachments[0]
                .url
                .as_deref(),
            Some("https://drive.example/f/1")
        );
    }

    #[rstest]
    fn message_attachment_can_be_attached(draft_env: DraftEnv) {
        let mut m = incoming("p1", "t1", "marc@example.com", &["INBOX"], 1000, "see file");
        m.parsed.attachments.push(crate::mime::ParsedAttachment {
            filename: "budget.xlsx".into(),
            mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            data: b"PK fictional".to_vec(),
            ..Default::default()
        });
        let s = &draft_env.env.store;
        s.ingest(&draft_env.env.acc, "me@example.com", &m, SyncMode::Initial)
            .unwrap();
        let aid: String =
            s.r.lock()
                .unwrap()
                .query_row("SELECT id FROM attachments", [], |r| r.get(0))
                .unwrap();
        let a = s
            .add_draft_attachment(
                &draft_env.draft,
                &NewAttachment {
                    source: AttachmentSource::Message(aid),
                    filename: None,
                    inline: false,
                },
            )
            .unwrap();
        assert_eq!(std::fs::read(a.path.unwrap()).unwrap(), b"PK fictional");
    }

    #[rstest]
    fn sent_drafts_take_no_attachments(draft_env: DraftEnv) {
        draft_env
            .env
            .store
            .set_draft_state(&draft_env.draft, "sent")
            .unwrap();
        assert!(attach_path(&draft_env, &draft_env.file).is_err());
    }

    // ---- suggestions and what they learn -------------------------------------

    /// A read Backlog thread from `sender`, `n` its unique suffix.
    fn from_sender(env: &Env, n: &str, sender: &str) -> String {
        let tid = env
            .store
            .ingest(
                &env.acc,
                "me@example.com",
                &incoming(
                    &format!("m{n}"),
                    &format!("t{n}"),
                    sender,
                    &["INBOX"],
                    now_ms() - 1000,
                    "hi",
                ),
                SyncMode::Initial,
            )
            .unwrap();
        env.store
            .update_threads(
                std::slice::from_ref(&tid),
                &ThreadPatch {
                    status: Some("backlog".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        tid
    }

    fn to_voyage() -> Suggestion {
        Suggestion {
            projects: vec!["Voyage".into()],
            project_confidence: Some(0.9),
            ..Default::default()
        }
    }

    fn voyage() -> SuggestionGroup {
        SuggestionGroup {
            status: None,
            projects: vec!["Voyage".into()],
            count: 0,
        }
    }

    /// Classifies and answers one card from `sender`.
    fn answered(env: &Env, n: &str, sender: &str, answer: SortAnswer) -> String {
        let tid = from_sender(env, n, sender);
        classify(env, &tid, &to_voyage());
        env.store.answer_sort(&tid, &answer).unwrap();
        tid
    }

    #[rstest]
    fn queue_takes_open_columns_newest_first(env: Env) {
        let old = open_thread(&env, "1", "backlog", 50_000);
        let new = open_thread(&env, "2", "todo", 0);
        assert_eq!(to_classify(&env), vec![new, old]);
    }

    #[rstest]
    fn only_the_asked_threads_are_candidates(env: Env) {
        let a = open_thread(&env, "1", "backlog", 0);
        open_thread(&env, "2", "backlog", 1000);
        let ids: Vec<String> = env
            .store
            .suggest_candidates(std::slice::from_ref(&a))
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, vec![a]);
    }

    #[rstest]
    fn unclassified_card_needs_the_ai(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        assert!(
            !env.store
                .suggest_candidates(std::slice::from_ref(&tid))
                .unwrap()[0]
                .ready
        );
    }

    #[rstest]
    fn answered_card_leaves_the_queue(env: Env) {
        let tid = answered(&env, "1", "ana@travel.example", SortAnswer::Yes);
        assert!(!to_classify(&env).contains(&tid));
    }

    #[rstest]
    fn yes_files_the_thread(env: Env) {
        let tid = answered(&env, "1", "ana@travel.example", SortAnswer::Yes);
        assert_eq!(
            env.store.thread(&tid).unwrap().projects,
            vec!["Voyage".to_string()]
        );
    }

    #[rstest]
    fn no_leaves_the_thread_where_it_is(env: Env) {
        let tid = answered(&env, "1", "ana@travel.example", SortAnswer::No);
        assert!(env.store.thread(&tid).unwrap().projects.is_empty());
    }

    #[rstest]
    fn same_sender_is_not_offered_a_refused_destination_again(env: Env) {
        answered(&env, "1", "ana@travel.example", SortAnswer::No);
        let next = from_sender(&env, "2", "ana@travel.example");
        assert!(!classify(&env, &next, &to_voyage()));
    }

    #[rstest]
    fn another_sender_is_still_offered_that_destination(env: Env) {
        answered(&env, "1", "ana@travel.example", SortAnswer::No);
        let next = from_sender(&env, "2", "bob@other.example");
        assert!(classify(&env, &next, &to_voyage()));
    }

    #[rstest]
    fn refusal_is_remembered_as_a_negative(env: Env) {
        answered(&env, "1", "ana@travel.example", SortAnswer::No);
        assert!(
            env.store
                .refused_before("ana@travel.example", &voyage())
                .unwrap()
        );
    }

    #[rstest]
    fn change_refuses_the_suggestion(env: Env) {
        answered(
            &env,
            "1",
            "ana@travel.example",
            SortAnswer::Change {
                status: None,
                projects: vec!["Immo".into()],
            },
        );
        assert!(
            env.store
                .refused_before("ana@travel.example", &voyage())
                .unwrap()
        );
    }

    #[rstest]
    fn change_files_where_the_user_chose(env: Env) {
        let tid = answered(
            &env,
            "1",
            "ana@travel.example",
            SortAnswer::Change {
                status: None,
                projects: vec!["Immo".into()],
            },
        );
        assert_eq!(
            env.store.thread(&tid).unwrap().projects,
            vec!["Immo".to_string()]
        );
    }

    #[rstest]
    #[case::two_yes(2, None)]
    #[case::three_yes(3, Some(vec!["Voyage".to_string()]))]
    fn sender_rule_after_three_consistent_yes(
        #[case] yes: usize,
        #[case] want: Option<Vec<String>>,
    ) {
        let env = env();
        for i in 0..yes {
            answered(&env, &i.to_string(), "ana@travel.example", SortAnswer::Yes);
        }
        assert_eq!(
            env.store
                .sender_rule("ana@travel.example")
                .unwrap()
                .map(|g| g.projects),
            want
        );
    }

    #[rstest]
    fn a_no_for_that_destination_blocks_the_rule(env: Env) {
        for i in 0..3 {
            answered(&env, &i.to_string(), "ana@travel.example", SortAnswer::Yes);
        }
        answered(&env, "9", "ana@travel.example", SortAnswer::No);
        assert_eq!(env.store.sender_rule("ana@travel.example").unwrap(), None);
    }

    #[rstest]
    fn learned_rule_is_listed_in_plain_terms(env: Env) {
        for i in 0..3 {
            answered(&env, &i.to_string(), "ana@travel.example", SortAnswer::Yes);
        }
        let l = &env.store.learned(10).unwrap()[0];
        assert_eq!(
            (l.kind.as_str(), l.projects.clone()),
            ("rule", vec!["Voyage".to_string()])
        );
    }

    #[rstest]
    fn refusal_is_listed_as_avoided(env: Env) {
        answered(&env, "1", "ana@travel.example", SortAnswer::No);
        assert_eq!(env.store.learned(10).unwrap()[0].kind, "avoid");
    }

    #[rstest]
    fn forgotten_refusal_is_no_longer_listed(env: Env) {
        answered(&env, "1", "ana@travel.example", SortAnswer::No);
        let item = env.store.learned(10).unwrap().remove(0);
        env.store.forget_learned(&item).unwrap();
        assert!(env.store.learned(10).unwrap().is_empty());
    }

    #[rstest]
    fn forgotten_rule_no_longer_applies(env: Env) {
        for n in ["1", "2", "3"] {
            answered(&env, n, "ana@travel.example", SortAnswer::Yes);
        }
        let item = env.store.learned(10).unwrap().remove(0);
        env.store.forget_learned(&item).unwrap();
        assert_eq!(env.store.sender_rule("ana@travel.example").unwrap(), None);
    }

    #[rstest]
    fn undo_puts_the_card_back(env: Env) {
        let tid = from_sender(&env, "1", "ana@travel.example");
        classify(&env, &tid, &to_voyage());
        let undo = env.store.answer_sort(&tid, &SortAnswer::Yes).unwrap();
        env.store.undo_sort(&undo).unwrap();
        assert!(
            env.store
                .suggest_candidates(std::slice::from_ref(&tid))
                .unwrap()[0]
                .ready
        );
    }

    #[rstest]
    fn undo_forgets_the_answer(env: Env) {
        let tid = from_sender(&env, "1", "ana@travel.example");
        classify(&env, &tid, &to_voyage());
        let undo = env.store.answer_sort(&tid, &SortAnswer::No).unwrap();
        env.store.undo_sort(&undo).unwrap();
        assert!(
            !env.store
                .refused_before("ana@travel.example", &voyage())
                .unwrap()
        );
    }

    #[rstest]
    fn undo_unfiles_the_thread(env: Env) {
        let tid = from_sender(&env, "1", "ana@travel.example");
        classify(&env, &tid, &to_voyage());
        let undo = env.store.answer_sort(&tid, &SortAnswer::Yes).unwrap();
        env.store.undo_sort(&undo).unwrap();
        assert!(env.store.thread(&tid).unwrap().projects.is_empty());
    }

    /// Two threads from Ana read as one conversation (older, newer); the newer one
    /// carries a suggestion to archive.
    fn archive_suggested_in_a_conversation(env: &Env) -> (String, String) {
        let old = from_sender(env, "1", "ana@travel.example");
        let new = from_sender(env, "2", "ana@travel.example");
        env.store
            .merge_threads(&[old.clone(), new.clone()])
            .unwrap();
        classify(
            env,
            &new,
            &Suggestion {
                status: Some("archive".into()),
                status_confidence: Some(0.95),
                ..Default::default()
            },
        );
        (old, new)
    }

    #[rstest]
    fn accepting_a_suggestion_moves_the_whole_conversation(env: Env) {
        let (old, new) = archive_suggested_in_a_conversation(&env);
        env.store.answer_sort(&new, &SortAnswer::Yes).unwrap();
        assert_eq!(env.store.thread(&old).unwrap().status, "archive");
    }

    #[rstest]
    fn undoing_an_answer_brings_the_whole_conversation_back(env: Env) {
        let (old, new) = archive_suggested_in_a_conversation(&env);
        let was = env.store.thread(&old).unwrap().status;
        let undo = env.store.answer_sort(&new, &SortAnswer::Yes).unwrap();
        env.store.undo_sort(&undo).unwrap();
        assert_eq!(env.store.thread(&old).unwrap().status, was);
    }

    #[rstest]
    fn earlier_answers_reach_the_router(env: Env) {
        answered(&env, "1", "ana@travel.example", SortAnswer::No);
        let next = from_sender(&env, "2", "ana@travel.example");
        assert_eq!(
            env.store.route_input(&next).unwrap().earlier_answers,
            vec![("no: Voyage".to_string(), 1)]
        );
    }

    #[rstest]
    fn route_input_counts_the_unfiled_threads_with_these_people(env: Env) {
        from_sender(&env, "1", "ana@travel.example");
        from_sender(&env, "2", "ana@travel.example");
        let tid = from_sender(&env, "3", "ana@travel.example");
        assert_eq!(env.store.route_input(&tid).unwrap().unfiled_peers, 2);
    }

    #[rstest]
    fn clearing_drops_unreviewed_suggestions(env: Env) {
        let tid = open_thread(&env, "1", "backlog", 0);
        classify(&env, &tid, &jev_archive());
        env.store.clear_machine_suggestions().unwrap();
        assert_eq!(env.store.thread(&tid).unwrap().suggestion, None);
    }

    #[rstest]
    fn clearing_keeps_unread_suggestions(env: Env) {
        let tid = unread_thread(&env);
        env.store
            .save_suggestion(&tid, &archive_suggestion(), "jev", now_ms())
            .unwrap();
        env.store.clear_machine_suggestions().unwrap();
        assert!(env.store.thread(&tid).unwrap().suggestion.is_some());
    }

    #[rstest]
    fn clearing_keeps_refusals(env: Env) {
        answered(&env, "1", "ana@travel.example", SortAnswer::No);
        env.store.clear_machine_suggestions().unwrap();
        assert!(
            env.store
                .refused_before("ana@travel.example", &voyage())
                .unwrap()
        );
    }

    #[rstest]
    fn prepared_card_is_closed_after_a_no_for_the_same_sender(env: Env) {
        let next = from_sender(&env, "2", "ana@travel.example");
        classify(&env, &next, &to_voyage());
        answered(&env, "1", "ana@travel.example", SortAnswer::No);
        assert!(env.store.close_if_refused(&next).unwrap());
    }

    #[rstest]
    fn prepared_card_of_another_sender_stays(env: Env) {
        let next = from_sender(&env, "2", "bob@other.example");
        classify(&env, &next, &to_voyage());
        answered(&env, "1", "ana@travel.example", SortAnswer::No);
        assert!(!env.store.close_if_refused(&next).unwrap());
    }
}
