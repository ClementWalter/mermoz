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

const PRAGMAS: &str =
    "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;";

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
];

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

#[derive(Debug, Clone, Default)]
pub struct ThreadFilter {
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

/// Turns free text into a safe FTS5 query: every word is a quoted prefix term,
/// `from:x` / `to:x` / `subject:x` restrict to a column.
pub fn fts_query(q: &str) -> Option<String> {
    let mut terms = vec![];
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
        if clean.is_empty() {
            continue;
        }
        let term = format!("\"{}\"*", clean.replace('"', ""));
        terms.push(match col {
            Some(c) => format!("{c}:{term}"),
            None => term,
        });
    }
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
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
    blobs: PathBuf,
}

const THREAD_COLS: &str = "t.id, t.number, t.account_id, t.subject, t.snippet, t.participants, t.last_from, t.last_date, \
    t.unread, t.message_count, t.has_attachments, t.status, t.priority, t.due, t.summary, t.next_action";

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
        "SELECT thread_id, status, status_confidence, projects_json, project_confidence, priority, automated
         FROM suggestions WHERE decision IS NULL AND thread_id IN ({marks})"
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
    for t in rows.iter_mut() {
        t.projects = projects.remove(&t.id).unwrap_or_default();
        t.suggestion = suggestions.remove(&t.id);
    }
    Ok(())
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
    })
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

const DRAFT_COLS: &str =
    "id, account_id, thread_id, to_json, cc_json, subject, body, state, origin, created, updated";

impl Store {
    pub fn open(path: &Path, blobs: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::create_dir_all(blobs)?;
        let w = Connection::open(path)?;
        w.execute_batch(PRAGMAS)?;
        migrate(&w)?;
        w.busy_timeout(std::time::Duration::from_secs(5))?;
        let r = Connection::open(path)?;
        r.execute_batch("PRAGMA foreign_keys = ON;")?;
        r.busy_timeout(std::time::Duration::from_secs(5))?;
        let store = Store {
            w: Mutex::new(w),
            r: Mutex::new(r),
            blobs: blobs.to_path_buf(),
        };
        store.seed()?;
        Ok(store)
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
        let mut st = r.prepare("SELECT id, email, provider, sync_state, synced, total, error FROM accounts ORDER BY created")?;
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
        Ok(())
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
        Ok(r.query_row(
            "SELECT 1 FROM messages WHERE account_id = ?1 AND provider_id = ?2",
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
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let thread_id = Self::ingest_tx(&tx, &self.blobs, account_id, me, msg, mode)?;
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
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let mut ids = vec![];
        for m in msgs {
            // One malformed message must not stall the mailbox: isolate each in a savepoint,
            // record failures for a later retry, and keep importing.
            tx.execute_batch("SAVEPOINT msg")?;
            match Self::ingest_tx(&tx, &self.blobs, account_id, me, m, mode) {
                Ok(id) => {
                    tx.execute_batch("RELEASE msg")?;
                    ids.push(id);
                }
                Err(e) => {
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
        tx.commit()?;
        Ok(ids)
    }

    fn ingest_tx(
        tx: &Connection,
        blobs: &Path,
        account_id: &str,
        me: &str,
        msg: &IncomingMessage,
        mode: SyncMode,
    ) -> Result<String> {
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
                    "INSERT INTO threads (id, account_id, provider_thread_id, number, status) VALUES (?1, ?2, ?3, ?4, 'unread')",
                    params![tid, account_id, msg.provider_thread_id, number],
                )?;
                (tid, true)
            }
        };

        let mid = new_id("msg");
        tx.execute(
            "INSERT INTO messages (id, account_id, provider_id, thread_id, from_name, from_email, to_json, cc_json, date, subject,
               body_text, body_html, labels_json, is_from_me, unread, message_id_header, references_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
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
                p.body_html,
                labels_json,
                is_from_me as i64,
                unread as i64,
                p.message_id,
                serde_json::to_string(&p.references)?,
            ],
        )?;
        let fts_rowid = fts_rowid(date, tx.last_insert_rowid());
        tx.execute(
            "UPDATE messages SET fts_rowid = ?2 WHERE id = ?1",
            params![mid, fts_rowid],
        )?;
        let recipients: Vec<String> = p.to.iter().chain(p.cc.iter()).map(fmt_addr).collect();
        tx.execute(
            "INSERT INTO messages_fts (rowid, subject, sender, recipients, body) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![fts_rowid, p.subject, fmt_addr(&from), recipients.join(", "), p.body_text],
        )?;
        for a in &p.attachments {
            let aid = new_id("att");
            let dir = blobs.join(&aid);
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(safe_filename(&a.filename));
            std::fs::write(&path, &a.data)?;
            tx.execute(
                "INSERT INTO attachments (id, message_id, filename, mime, size, path) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![aid, mid, a.filename, a.mime, a.data.len() as i64, path.to_string_lossy()],
            )?;
        }
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
        let mut st = tx.prepare_cached(
            "SELECT from_name, from_email, is_from_me, unread, date, subject, body_text,
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
        let snippet: String = last
            .6
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(200)
            .collect();
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
                rows.len() as i64,
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
            "SELECT l.account_id, l.label_id, l.name, (SELECT COUNT(*) FROM thread_labels tl WHERE tl.account_id = l.account_id AND tl.label_id = l.label_id)
             FROM labels l ORDER BY l.name COLLATE NOCASE",
        )?;
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
        let mut st = r.prepare_cached(
            "SELECT m.id, from_name, from_email, to_json, cc_json, date, subject, body_text, body_html IS NOT NULL, is_from_me,
               (SELECT d.clean_body FROM digests d WHERE d.message_id = m.id AND d.status = 'ok')
             FROM messages m WHERE thread_id = ?1 ORDER BY date",
        )?;
        let mut msgs = st
            .query_map([thread_id], |r| {
                let to: String = r.get(3)?;
                let cc: String = r.get(4)?;
                Ok(Message {
                    id: r.get(0)?,
                    from: Address {
                        name: r.get(1)?,
                        email: r.get(2)?,
                    },
                    to: serde_json::from_str(&to).unwrap_or_default(),
                    cc: serde_json::from_str(&cc).unwrap_or_default(),
                    date: r.get(5)?,
                    subject: r.get(6)?,
                    body_new: String::new(),
                    has_quoted: false,
                    body_clean: r.get(10)?,
                    body_text: r.get(7)?,
                    has_html: r.get::<_, i64>(8)? != 0,
                    is_from_me: r.get::<_, i64>(9)? != 0,
                    attachments: vec![],
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut st = r.prepare_cached(
            "SELECT id, filename, mime, size FROM attachments WHERE message_id = ?1",
        )?;
        for m in &mut msgs {
            (m.body_new, m.has_quoted) = crate::quote::readable(&m.body_text);
            m.attachments = st
                .query_map([&m.id], |r| {
                    Ok(AttachmentInfo {
                        id: r.get(0)?,
                        filename: r.get(1)?,
                        mime: r.get(2)?,
                        size: r.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(msgs)
    }

    pub fn message_html(&self, id: &str) -> Result<Option<String>> {
        let r = self.r.lock().unwrap();
        r.query_row("SELECT body_html FROM messages WHERE id = ?1", [id], |r| {
            r.get::<_, Option<String>>(0)
        })
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

    /// Applies an issue edit locally right away and queues the Gmail label push.
    pub fn update_threads(&self, ids: &[String], patch: &ThreadPatch) -> Result<usize> {
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
        let mut n = 0;
        for id in ids {
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
        let mut st = tx.prepare_cached(
            "SELECT tl.label_id, l.name FROM thread_labels tl LEFT JOIN labels l ON l.account_id = tl.account_id AND l.label_id = tl.label_id
             WHERE tl.thread_id = ?1 AND tl.account_id = ?2",
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

    pub fn counts(&self) -> Result<serde_json::Value> {
        let r = self.r.lock().unwrap();
        let open = "t.status IN (SELECT key FROM statuses WHERE archived = 0)";
        let mut status = serde_json::Map::new();
        let mut st = r.prepare_cached(&format!(
            "SELECT t.status, COUNT(*) FROM threads t WHERE {open} GROUP BY t.status"
        ))?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (k, n) = row?;
            status.insert(k, n.into());
        }
        let mut project = serde_json::Map::new();
        let mut st = r.prepare_cached(&format!(
            "SELECT tp.name, COUNT(*) FROM thread_projects tp JOIN threads t ON t.id = tp.thread_id WHERE {open} GROUP BY tp.name"
        ))?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (k, n) = row?;
            project.insert(k, n.into());
        }
        let mut account = serde_json::Map::new();
        let mut st = r.prepare_cached(&format!(
            "SELECT t.account_id, COUNT(*) FROM threads t WHERE {open} GROUP BY t.account_id"
        ))?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (k, n) = row?;
            account.insert(k, n.into());
        }
        let mut priority = serde_json::Map::new();
        let mut st = r.prepare_cached(&format!(
            "SELECT t.priority, COUNT(*) FROM threads t WHERE {open} GROUP BY t.priority"
        ))?;
        for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (k, n) = row?;
            priority.insert(k.to_string(), n.into());
        }
        Ok(
            serde_json::json!({ "status": status, "project": project, "account": account, "priority": priority }),
        )
    }

    /// Full-text search, most recent matching threads first. FTS rowids are
    /// date-ordered, so even a word in every message returns in milliseconds.
    pub fn search(&self, q: &str, limit: i64) -> Result<Vec<ThreadSummary>> {
        let Some(fq) = fts_query(q) else {
            return Ok(vec![]);
        };
        let r = self.r.lock().unwrap();
        let sql = format!(
            "WITH hits AS (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1 ORDER BY rowid DESC LIMIT ?2)
             SELECT {THREAD_COLS} FROM hits JOIN messages m ON m.fts_rowid = hits.rowid JOIN threads t ON t.id = m.thread_id
             GROUP BY t.id ORDER BY t.last_date DESC LIMIT ?3"
        );
        let mut st = r.prepare_cached(&sql)?;
        let mut rows = st
            .query_map(params![fq, limit * 4, limit], thread_row)?
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
        let mut st = r.prepare_cached("SELECT label_id, name FROM labels")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, name) = row?;
            if is_user_label(&id, &name) {
                names.insert(name);
            }
        }
        let mut st = r.prepare_cached(
            "SELECT name FROM project_colors UNION SELECT DISTINCT name FROM thread_projects",
        )?;
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
            "INSERT INTO suggestions (thread_id, status, status_confidence, projects_json, project_confidence, priority, automated, model, for_date, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(thread_id) DO UPDATE SET status = excluded.status, status_confidence = excluded.status_confidence,
               projects_json = excluded.projects_json, project_confidence = excluded.project_confidence, priority = excluded.priority,
               automated = excluded.automated, model = excluded.model, for_date = excluded.for_date, created = excluded.created,
               decision = NULL, decided = NULL",
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
                now_ms()
            ],
        )?;
        Ok(())
    }

    /// Unread threads without a prediction for their latest message, newest first.
    pub fn threads_to_route(&self, limit: i64) -> Result<Vec<(String, i64)>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT t.id, t.last_date FROM threads t LEFT JOIN suggestions s ON s.thread_id = t.id
             WHERE t.status = 'unread' AND (s.thread_id IS NULL OR s.for_date < t.last_date)
             ORDER BY t.last_date DESC LIMIT ?1",
        )?;
        Ok(st
            .query_map([limit], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// What the router tells Jev about a thread: the last three messages without quoted
    /// history, and where earlier threads with the same people were filed.
    pub fn route_input(&self, thread_id: &str) -> Result<crate::jev::RouteInput> {
        let r = self.r.lock().unwrap();
        let rows: Vec<(String, String, bool, i64, String, String)> = {
            let mut st = r.prepare_cached(
                "SELECT from_name, from_email, is_from_me, date, subject, body_text FROM messages WHERE thread_id = ?1 ORDER BY date",
            )?;
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
                    days_ago: (now - date) / 86_400_000,
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
        Ok(crate::jev::RouteInput {
            subject: rows
                .iter()
                .map(|r| r.4.clone())
                .find(|s| !s.is_empty())
                .unwrap_or_default(),
            messages_in_thread: rows.len() as i64,
            last_message_from_me: rows.last().map(|r| r.2).unwrap_or(false),
            recent_messages: recent,
            sender_history: history,
        })
    }

    /// Every project with a description built from the mail filed in it: its four most
    /// frequent correspondents and five latest subjects.
    pub fn project_descriptions(&self) -> Result<Vec<(String, String)>> {
        let names: Vec<String> = self.projects()?.into_iter().map(|p| p.name).collect();
        let r = self.r.lock().unwrap();
        let mut subjects: std::collections::HashMap<String, Vec<String>> = Default::default();
        let mut st = r.prepare_cached(
            "SELECT tp.name, t.subject FROM thread_projects tp JOIN threads t ON t.id = tp.thread_id
             WHERE t.subject != '' ORDER BY t.last_date DESC",
        )?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (n, subj) = row?;
            let v = subjects.entry(n).or_default();
            if v.len() < 5 {
                v.push(subj);
            }
        }
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
                let d = crate::jev::describe(
                    &n,
                    people.get(&n).map(Vec::as_slice).unwrap_or(&[]),
                    subjects.get(&n).map(Vec::as_slice).unwrap_or(&[]),
                );
                (n, d)
            })
            .collect())
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
                    "SELECT t.account_id, s.status, s.projects_json, s.priority FROM suggestions s JOIN threads t ON t.id = s.thread_id
                     WHERE s.thread_id = ?1 AND s.decision IS NULL",
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
            if let Some(st) = status {
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
            Self::enqueue_label_sync_tx(&tx, &acc, id)?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
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
            &format!("INSERT INTO drafts ({DRAFT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
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
                d.updated
            ],
        )?;
        drop(w);
        self.draft(&d.id)
    }

    pub fn draft(&self, id: &str) -> Result<Draft> {
        let r = self.r.lock().unwrap();
        r.query_row(
            &format!("SELECT {DRAFT_COLS} FROM drafts WHERE id = ?1"),
            [id],
            draft_row,
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("draft {id}")))
    }

    pub fn drafts(&self, state: Option<&str>) -> Result<Vec<Draft>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(&format!(
            "SELECT {DRAFT_COLS} FROM drafts WHERE (?1 IS NULL OR state = ?1) AND state != 'sent' ORDER BY updated DESC"
        ))?;
        Ok(st
            .query_map([state], draft_row)?
            .collect::<rusqlite::Result<_>>()?)
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
        Ok(())
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
    pub fn reply_headers(&self, thread_id: &str) -> Result<(Option<String>, Vec<String>)> {
        let r = self.r.lock().unwrap();
        let row: Option<(Option<String>, String)> = r
            .query_row(
                "SELECT message_id_header, references_json FROM messages WHERE thread_id = ?1 ORDER BY date DESC LIMIT 1",
                [thread_id],
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

    /// Defaults for a reply: everyone on the last message except me.
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
        let Some(last) = msgs.iter().rev().find(|m| !m.is_from_me).or(msgs.last()) else {
            return Ok((vec![], vec![], subject));
        };
        let not_me = |a: &Address| !a.email.eq_ignore_ascii_case(me);
        let mut to: Vec<String> = if last.is_from_me {
            last.to
                .iter()
                .filter(|a| not_me(a))
                .map(|a| a.email.clone())
                .collect()
        } else {
            vec![last.from.email.clone()]
        };
        to.dedup();
        let cc: Vec<String> = last
            .to
            .iter()
            .chain(last.cc.iter())
            .filter(|a| not_me(a) && !to.contains(&a.email))
            .map(|a| a.email.clone())
            .collect();
        Ok((to, cc, subject))
    }

    /// Every address seen in local mail, for the recipient check.
    pub fn known_address(&self, email: &str) -> Result<bool> {
        let r = self.r.lock().unwrap();
        let like = format!("%\"{}\"%", email.to_lowercase());
        Ok(r.query_row(
            "SELECT 1 FROM messages WHERE lower(from_email) = ?1 OR lower(to_json) LIKE ?2 OR lower(cc_json) LIKE ?2 LIMIT 1",
            params![email.to_lowercase(), like],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
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
            "SELECT lower(from_email), MAX(from_name), COUNT(*), MAX(date) FROM messages WHERE is_from_me = 0 AND from_email != '' GROUP BY lower(from_email)",
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
        let mut st = r.prepare_cached(
            "SELECT lower(json_extract(j.value, '$.email')), MAX(json_extract(j.value, '$.name')), COUNT(*), MAX(m.date)
             FROM messages m, json_each(m.to_json) j WHERE m.is_from_me = 1 GROUP BY 1",
        )?;
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
        let mut st = r.prepare_cached(
            "SELECT lower(from_email), thread_id FROM messages WHERE is_from_me = 0 GROUP BY 1, 2
             UNION SELECT lower(json_extract(j.value, '$.email')), m.thread_id FROM messages m, json_each(m.to_json) j
             WHERE m.is_from_me = 1 GROUP BY 1, 2",
        )?;
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
        emails.sort_by(|a, b| b.1.2.cmp(&a.1.2));
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

    /// What the triage model sees for the newest threads in a status, per account.
    pub fn triage_items(
        &self,
        status: &str,
        limit: i64,
    ) -> Result<Vec<(String, String, crate::agent::TriageItem)>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(&format!(
            "SELECT {THREAD_COLS}, (SELECT is_from_me FROM messages m WHERE m.thread_id = t.id ORDER BY date DESC LIMIT 1)
             FROM threads t WHERE t.status = ?1 ORDER BY t.last_date DESC LIMIT ?2"
        ))?;
        let now = now_ms();
        let rows = st
            .query_map(params![status, limit], |r| {
                Ok((
                    thread_row(r)?,
                    r.get::<_, Option<i64>>(16)?.unwrap_or(0) != 0,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .map(|(t, mine)| {
                let item = crate::agent::TriageItem {
                    number: t.number.clone(),
                    subject: t.subject.clone(),
                    from: t.last_from.clone(),
                    last_from_me: mine,
                    unread: t.unread,
                    messages: t.message_count,
                    days_since_last: (now - t.last_date) / 86_400_000,
                    current_status: t.status.clone(),
                    excerpt: t.snippet.chars().take(240).collect(),
                };
                (t.id, t.account_id, item)
            })
            .collect())
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
        // One pending op per (kind, key): a newer edit supersedes an unsent one.
        tx.execute(
            "INSERT INTO ops (account_id, kind, key, payload, created) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(kind, key) DO UPDATE SET payload = excluded.payload, seq = seq + 1, attempts = 0, last_error = NULL",
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
        let mut st = r.prepare_cached("SELECT id, account_id, kind, key, payload, attempts, seq FROM ops ORDER BY id LIMIT ?1")?;
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
    #[case::words_are_prefix_terms("audit report", Some("\"audit\"* \"report\"*"))]
    #[case::from_maps_to_sender("from:marc", Some("sender:\"marc\"*"))]
    #[case::punctuation_is_dropped("\"hello\" (world)", Some("\"hello\"* \"world\"*"))]
    #[case::empty_is_none("  ", None)]
    fn fts_query_building(#[case] q: &str, #[case] want: Option<&str>) {
        assert_eq!(fts_query(q).as_deref(), want);
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
        assert_eq!(env.store.search("remed", 10).unwrap().len(), 1);
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
        assert_eq!(s.search("budget", 10).unwrap()[0].id, newer);
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
        assert_eq!(env.store.search("reunion", 10).unwrap().len(), 1);
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
        assert_eq!(s.counts().unwrap()["status"].get("archive"), None);
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
            vec![("Immo".to_string(), "Folder 'Immo'. Frequent correspondents: Marc Petit. Example subjects: Audit report v2.".to_string())]
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
}
