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
const MIGRATIONS: &[&str] = &[SCHEMA, THREAD_LABELS, SYNC_FAILURES, AGENT, PROVIDER_AUTH];

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

/// Prefix of every Gmail label Mermoz manages; anything else is left untouched.
pub const LABEL_PREFIX: &str = "Mermoz/";

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
    pub project_id: Option<String>,
    pub account_id: Option<String>,
    pub priority: Option<Vec<i64>>,
    pub label_id: Option<String>,
    /// Threads with at least one message from any of these addresses.
    pub participants: Option<Vec<String>>,
    /// "last_date" (default), "priority", "due" or "created".
    pub order: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ThreadPatch {
    pub status: Option<String>,
    pub priority: Option<i64>,
    pub project_id: Option<Option<String>>,
    pub due: Option<Option<String>>,
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
    status_label: Option<&str>,
    last_date: i64,
    now: i64,
    mode: SyncMode,
) -> String {
    if let Some(s) = status_label {
        return s.to_string();
    }
    if !in_inbox {
        return "done".into();
    }
    // Old inbox mail imported on first sync goes to Backlog so Triage starts manageable.
    const THIRTY_DAYS: i64 = 30 * 24 * 3600 * 1000;
    if mode == SyncMode::Initial && now - last_date > THIRTY_DAYS {
        "backlog".into()
    } else {
        "triage".into()
    }
}

/// Reopen rule: someone else replying on a Done/Waiting thread puts the ball back with you.
pub fn status_after_new_message(current: &str, from_me: bool) -> Option<&'static str> {
    if !from_me && (current == "done" || current == "waiting") {
        Some("todo")
    } else {
        None
    }
}

/// Gmail labels (by name) a thread should carry for its issue state; INBOX included.
pub fn desired_labels(
    status: &str,
    archived: bool,
    status_name: &str,
    priority: i64,
    project: Option<&str>,
) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    if !archived {
        s.insert("INBOX".to_string());
    }
    if status != "triage" && status != "done" {
        s.insert(format!("{LABEL_PREFIX}{status_name}"));
    }
    if let Some(p) = priority_name(priority).filter(|_| priority != 3) {
        s.insert(format!("{LABEL_PREFIX}Priority/{p}"));
    }
    if let Some(p) = project {
        s.insert(format!("{LABEL_PREFIX}Project/{p}"));
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
pub fn order_clause(order: Option<&str>) -> &'static str {
    match order {
        // No priority sorts after Low, like Linear.
        Some("priority") => "CASE t.priority WHEN 0 THEN 9 ELSE t.priority END, t.last_date DESC",
        Some("due") => "t.due IS NULL, t.due, t.last_date DESC",
        Some("created") => "t.number DESC",
        _ => "t.last_date DESC",
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
    t.unread, t.message_count, t.has_attachments, t.status, t.priority, t.project_id, t.due, t.summary, t.next_action";

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
        project_id: r.get(13)?,
        due: r.get(14)?,
        summary: r.get(15)?,
        next_action: r.get(16)?,
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
    pub messages: i64,
    pub last_seen: i64,
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
            tx.execute(
                "UPDATE messages SET labels_json = ?2, unread = ?3 WHERE id = ?1",
                params![mid, labels_json, unread as i64],
            )?;
            Self::refresh_thread(tx, &tid)?;
            if mode == SyncMode::Incremental {
                Self::adopt_remote_labels(tx, account_id, &tid)?;
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
                if mode == SyncMode::Incremental
                    && let Some(s) = status_after_new_message(&status, is_from_me)
                {
                    tx.execute(
                        "UPDATE threads SET status = ?2 WHERE id = ?1",
                        params![tid, s],
                    )?;
                    Self::enqueue_label_sync_tx(tx, account_id, &tid)?;
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
                    "INSERT INTO threads (id, account_id, provider_thread_id, number, status) VALUES (?1, ?2, ?3, ?4, 'triage')",
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
            let last_date: i64 = tx.query_row(
                "SELECT last_date FROM threads WHERE id = ?1",
                [&thread_id],
                |r| r.get(0),
            )?;
            let status =
                initial_status(in_inbox, status_label.as_deref(), last_date, now_ms(), mode);
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
               unread = ?7, message_count = ?8, has_attachments = ?9 WHERE id = ?1",
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

    /// Priority and project labels set on another device become local fields.
    fn adopt_meta_labels(tx: &Connection, thread_id: &str, names: &BTreeSet<String>) -> Result<()> {
        for n in names {
            if let Some(p) = n.strip_prefix(&format!("{LABEL_PREFIX}Priority/")) {
                let v = (1..=4).find(|i| priority_name(*i) == Some(p)).unwrap_or(0);
                tx.execute(
                    "UPDATE threads SET priority = ?2 WHERE id = ?1",
                    params![thread_id, v],
                )?;
            } else if let Some(p) = n.strip_prefix(&format!("{LABEL_PREFIX}Project/")) {
                let pid = Self::project_id_tx(tx, p)?;
                tx.execute(
                    "UPDATE threads SET project_id = ?2 WHERE id = ?1",
                    params![thread_id, pid],
                )?;
            }
        }
        Ok(())
    }

    fn project_id_tx(tx: &Connection, name: &str) -> Result<String> {
        if let Some(id) = tx
            .query_row("SELECT id FROM projects WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()?
        {
            return Ok(id);
        }
        let id = new_id("prj");
        const COLORS: &[&str] = &[
            "#5B8DEF", "#E0782F", "#2E9A6B", "#8A6FD1", "#C2568C", "#2F8FCF",
        ];
        let n: i64 = tx.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))?;
        tx.execute(
            "INSERT INTO projects (id, name, color) VALUES (?1, ?2, ?3)",
            params![id, name, COLORS[n as usize % COLORS.len()]],
        )?;
        Ok(id)
    }

    /// Label changes made elsewhere (phone, webmail) update the issue, unless a
    /// local change is still waiting to be pushed — local intent wins.
    fn adopt_remote_labels(tx: &Connection, account_id: &str, thread_id: &str) -> Result<()> {
        let pending: bool = tx
            .query_row(
                "SELECT 1 FROM ops WHERE kind = 'labels' AND key = ?1",
                [thread_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if pending {
            return Ok(());
        }
        let (in_inbox, names) = Self::thread_remote_labels(tx, account_id, thread_id)?;
        let current: String = tx.query_row(
            "SELECT status FROM threads WHERE id = ?1",
            [thread_id],
            |r| r.get(0),
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
        let next = match Self::status_from_labels(tx, &names)? {
            Some(s) => Some(s),
            None if !in_inbox && !archived => Some("done".to_string()),
            None if in_inbox && archived => Some("triage".to_string()),
            None => None,
        };
        if let Some(s) = next.filter(|s| *s != current) {
            tx.execute(
                "UPDATE threads SET status = ?2 WHERE id = ?1",
                params![thread_id, s],
            )?;
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
        if let Some(p) = &f.project_id {
            clauses.push("t.project_id = ?".into());
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
            clauses.push(format!(
                "t.id IN (SELECT m.thread_id FROM messages m WHERE lower(m.from_email) IN ({}))",
                vec!["lower(?)"; es.len()].join(",")
            ));
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
            order_clause(f.order.as_deref()),
            f.offset.max(0)
        );
        let mut st = r.prepare_cached(&sql)?;
        let rows = st
            .query_map(params_from_iter(args.iter()), thread_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((rows, total))
    }

    pub fn thread(&self, id: &str) -> Result<ThreadSummary> {
        let r = self.r.lock().unwrap();
        r.query_row(
            &format!("SELECT {THREAD_COLS} FROM threads t WHERE t.id = ?1"),
            [id],
            thread_row,
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("thread {id}")))
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
        if let Some(Some(p)) = &patch.project_id {
            let ok = tx
                .query_row("SELECT 1 FROM projects WHERE id = ?1", [p], |_| Ok(()))
                .optional()?
                .is_some();
            if !ok {
                return Err(Error::InvalidParams(format!("unknown project '{p}'")));
            }
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
                    "UPDATE threads SET status = ?2 WHERE id = ?1",
                    params![id, s],
                )?;
            }
            if let Some(p) = patch.priority {
                tx.execute(
                    "UPDATE threads SET priority = ?2 WHERE id = ?1",
                    params![id, p],
                )?;
            }
            if let Some(p) = &patch.project_id {
                tx.execute(
                    "UPDATE threads SET project_id = ?2 WHERE id = ?1",
                    params![id, p],
                )?;
            }
            if let Some(d) = &patch.due {
                tx.execute("UPDATE threads SET due = ?2 WHERE id = ?1", params![id, d])?;
            }
            if patch.status.is_some() || patch.priority.is_some() || patch.project_id.is_some() {
                Self::enqueue_label_sync_tx(&tx, &acc, id)?;
            }
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn mark_read(&self, thread_id: &str, read: bool) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let acc: String = tx
            .query_row(
                "SELECT account_id FROM threads WHERE id = ?1",
                [thread_id],
                |r| r.get(0),
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

    /// Desired Gmail labels for a thread, from its current issue fields.
    pub fn thread_desired_labels(&self, thread_id: &str) -> Result<BTreeSet<String>> {
        let r = self.r.lock().unwrap();
        let (status, name, archived, priority, project): (String, String, i64, i64, Option<String>) = r.query_row(
            "SELECT t.status, s.name, s.archived, t.priority, p.name FROM threads t JOIN statuses s ON s.key = t.status
             LEFT JOIN projects p ON p.id = t.project_id WHERE t.id = ?1",
            [thread_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        Ok(desired_labels(
            &status,
            archived != 0,
            &name,
            priority,
            project.as_deref(),
        ))
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
        let mut st = r.prepare_cached(&format!("SELECT t.project_id, COUNT(*) FROM threads t WHERE {open} AND t.project_id IS NOT NULL GROUP BY t.project_id"))?;
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
        let rows = st
            .query_map(params![fq, limit * 4, limit], thread_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
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

    pub fn projects(&self) -> Result<Vec<Project>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached("SELECT id, name, color FROM projects ORDER BY name")?;
        Ok(st
            .query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    color: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn create_project(&self, name: &str, color: Option<&str>) -> Result<Project> {
        let name = name.trim();
        if name.is_empty() || name.contains('/') {
            return Err(Error::InvalidParams(
                "project name must be non-empty and contain no '/'".into(),
            ));
        }
        let w = self.w.lock().unwrap();
        let id = Self::project_id_tx(&w, name)?;
        if let Some(c) = color {
            w.execute(
                "UPDATE projects SET color = ?2 WHERE id = ?1",
                params![id, c],
            )?;
        }
        let p = w.query_row(
            "SELECT id, name, color FROM projects WHERE id = ?1",
            [&id],
            |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    color: r.get(2)?,
                })
            },
        )?;
        Ok(p)
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
        let mut cards: std::collections::HashMap<String, Contact> = Default::default();
        let mut st = r.prepare_cached("SELECT email, name, title, company, phones_json, address, website, socials_json, last_seen FROM contacts")?;
        for c in st.query_map([], contact_row)? {
            let c = c?;
            cards.insert(c.email.clone(), c);
        }
        // People only: someone the user wrote to, or whose signature carried details.
        let mut people: Vec<Person> = vec![];
        let mut index: std::collections::HashMap<String, usize> = Default::default();
        let mut emails: Vec<_> = by_email.into_iter().collect();
        emails.sort_by(|a, b| b.1.2.cmp(&a.1.2));
        for (email, (name, count, last, written)) in emails {
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
            let merge_key = if key.split(' ').count() >= 2 {
                key
            } else {
                email.clone()
            };
            match index.get(&merge_key) {
                Some(&i) => {
                    let p = &mut people[i];
                    p.emails.push(email.clone());
                    p.messages += count;
                    p.last_seen = p.last_seen.max(last);
                    p.card = crate::agent::merge_card(&card_data, &p.card);
                }
                None => {
                    let mut c = card_data;
                    c.name = display;
                    index.insert(merge_key, people.len());
                    people.push(Person {
                        email: email.clone(),
                        emails: vec![email],
                        card: c,
                        messages: count,
                        last_seen: last,
                    });
                }
            }
        }
        // Alphabetical by family name, then first name, like Contacts (accents and
        // "Last, First" folded); entries
        // without a real name (just an address) go last.
        people.sort_by_cached_key(|p| {
            let key = person_key(&p.card.name);
            let last = key.rsplit(' ').next().unwrap_or("").to_string();
            (p.card.name.contains('@'), last, key)
        });
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
                    r.get::<_, Option<i64>>(17)?.unwrap_or(0) != 0,
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

    /// Every message of a thread for a forced re-clean, mine included.
    pub fn thread_digest_inputs(&self, thread_id: &str) -> Result<Vec<PendingDigest>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached("SELECT id, from_name, from_email, subject, body_text FROM messages WHERE thread_id = ?1 AND length(body_text) > 0 ORDER BY date")?;
        Ok(st
            .query_map([thread_id], |r| {
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
    #[case::status_label_wins(true, Some("waiting"), 0, SyncMode::Initial, "waiting")]
    #[case::archived_is_done(false, None, 0, SyncMode::Initial, "done")]
    #[case::recent_inbox_is_triage(true, None, 0, SyncMode::Initial, "triage")]
    #[case::old_inbox_initial_is_backlog(true, None, 40, SyncMode::Initial, "backlog")]
    #[case::old_inbox_incremental_is_triage(true, None, 40, SyncMode::Incremental, "triage")]
    fn initial_status_rules(
        #[case] inbox: bool,
        #[case] label: Option<&str>,
        #[case] age_days: i64,
        #[case] mode: SyncMode,
        #[case] want: &str,
    ) {
        let now = 1_000 * DAY;
        assert_eq!(
            initial_status(inbox, label, now - age_days * DAY, now, mode),
            want
        );
    }

    #[rstest]
    #[case::done_reopens("done", false, Some("todo"))]
    #[case::waiting_reopens("waiting", false, Some("todo"))]
    #[case::my_reply_does_not_reopen("done", true, None)]
    #[case::todo_stays("todo", false, None)]
    #[case::canceled_stays("canceled", false, None)]
    fn reopen_rule(#[case] current: &str, #[case] from_me: bool, #[case] want: Option<&str>) {
        assert_eq!(status_after_new_message(current, from_me), want);
    }

    #[rstest]
    #[case::triage_inbox_only("triage", false, "Triage", 0, None, vec!["INBOX"])]
    #[case::done_has_nothing("done", true, "Done", 0, None, vec![])]
    #[case::todo_label("todo", false, "Todo", 0, None, vec!["INBOX", "Mermoz/Todo"])]
    #[case::canceled_archived_with_label("canceled", true, "Canceled", 0, None, vec!["Mermoz/Canceled"])]
    #[case::priority_and_project("todo", false, "Todo", 2, Some("Q4"), vec!["INBOX", "Mermoz/Priority/High", "Mermoz/Project/Q4", "Mermoz/Todo"])]
    #[case::normal_priority_no_label("triage", false, "Triage", 3, None, vec!["INBOX"])]
    fn desired_label_sets(
        #[case] status: &str,
        #[case] archived: bool,
        #[case] name: &str,
        #[case] prio: i64,
        #[case] project: Option<&str>,
        #[case] want: Vec<&str>,
    ) {
        let got: Vec<String> = desired_labels(status, archived, name, prio, project)
            .into_iter()
            .collect();
        assert_eq!(got, want);
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
    fn new_inbox_thread_is_triage(env: Env) {
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
        assert_eq!(env.store.thread(&tid).unwrap().status, "triage");
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
        assert_eq!(s.thread(&tid).unwrap().status, "todo");
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

    #[rstest]
    #[case::default(None, "t.last_date DESC")]
    #[case::unknown(Some("weird"), "t.last_date DESC")]
    #[case::created(Some("created"), "t.number DESC")]
    fn order_clauses(#[case] order: Option<&str>, #[case] want: &str) {
        assert_eq!(order_clause(order), want);
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
    fn remote_archive_marks_done(env: Env) {
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
        assert_eq!(s.thread(&tid).unwrap().status, "done");
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
    fn counts_exclude_done(env: Env) {
        let s = &env.store;
        s.ingest(
            &env.acc,
            "me@example.com",
            &incoming("m1", "t1", "marc@x.com", &[], now_ms(), "hi"),
            SyncMode::Initial,
        )
        .unwrap();
        assert_eq!(s.counts().unwrap()["status"].get("done"), None);
    }
}
