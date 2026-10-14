//! "Ask Mermoz to do" storage: a plan's filter resolved against the local mailbox, and
//! the operation log that keeps each run's targets and the threads' states before it,
//! so one Undo reverts the whole run, after a restart too.

use super::{Store, fts_query};
use crate::error::{Error, Result};
use crate::model::{ThreadState, now_ms};
use crate::plan::senders::{self, Entry, Sender};
use crate::plan::{CandidateSender, CandidateSubject, Candidates, Filter, cutoff};
use rusqlite::{OptionalExtension, params, params_from_iter};
use serde::Serialize;

pub(super) const MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS bulk_ops (
  id TEXT PRIMARY KEY, sentence TEXT NOT NULL, plan_json TEXT NOT NULL, summary TEXT NOT NULL,
  state TEXT NOT NULL, total INTEGER NOT NULL DEFAULT 0, done INTEGER NOT NULL DEFAULT 0,
  origin TEXT NOT NULL DEFAULT 'app', error TEXT,
  created INTEGER NOT NULL, finished INTEGER, undone INTEGER);
CREATE TABLE IF NOT EXISTS bulk_op_targets (
  op_id TEXT NOT NULL REFERENCES bulk_ops(id) ON DELETE CASCADE,
  step INTEGER NOT NULL, thread_id TEXT NOT NULL, PRIMARY KEY (op_id, step, thread_id));
CREATE TABLE IF NOT EXISTS bulk_op_before (
  op_id TEXT NOT NULL REFERENCES bulk_ops(id) ON DELETE CASCADE,
  thread_id TEXT NOT NULL, state_json TEXT NOT NULL, PRIMARY KEY (op_id, thread_id));
"#;

/// Each thread's put-off state before a run; `until` is null when it was not put off.
pub(super) const SNOOZE_MIGRATION: &str = "CREATE TABLE IF NOT EXISTS bulk_op_snoozes (
  op_id TEXT NOT NULL REFERENCES bulk_ops(id) ON DELETE CASCADE,
  thread_id TEXT NOT NULL, until INTEGER, for_date INTEGER, label TEXT, woke INTEGER,
  PRIMARY KEY (op_id, thread_id));";

/// A thread a filter matched, as the preview shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanMatch {
    pub id: String,
    pub number: String,
    pub status: String,
    pub subject: String,
    pub last_from: String,
    pub last_date: i64,
}

/// One logged run: planned, running, done, failed or undone.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BulkOp {
    pub id: String,
    pub sentence: String,
    pub plan: serde_json::Value,
    pub summary: String,
    pub state: String,
    pub total: i64,
    pub done: i64,
    pub origin: String,
    pub error: Option<String>,
    pub created: i64,
    pub finished: Option<i64>,
    pub undone: Option<i64>,
}

impl Store {
    /// Threads the filter matches, newest first. Trash is left out unless the filter
    /// asks for Deleted, so a bulk change never pulls mail back out of the Trash.
    pub fn plan_matches(&self, f: &Filter, now: i64) -> Result<Vec<PlanMatch>> {
        let mut c: Vec<String> = vec![];
        let mut a: Vec<rusqlite::types::Value> = vec![];
        let marks = |n: usize| vec!["?"; n].join(",");
        if f.status_in.is_empty() {
            c.push("t.status != 'deleted'".into());
        } else {
            c.push(format!("t.status IN ({})", marks(f.status_in.len())));
            a.extend(f.status_in.iter().map(|s| s.clone().into()));
        }
        if !f.status_not_in.is_empty() {
            c.push(format!(
                "t.status NOT IN ({})",
                marks(f.status_not_in.len())
            ));
            a.extend(f.status_not_in.iter().map(|s| s.clone().into()));
        }
        if let Some(age) = f.older_than {
            c.push("t.last_date < ?".into());
            a.push(cutoff(now, age).into());
        }
        if let Some(age) = f.newer_than {
            c.push("t.last_date >= ?".into());
            a.push(cutoff(now, age).into());
        }
        if !f.from.is_empty() {
            // Senders are matched by their words first (see plan::senders), then their
            // mail is found by address.
            let emails: Vec<String> = self
                .resolve_senders(&f.from)?
                .into_iter()
                .map(|s| s.email)
                .collect();
            if emails.is_empty() {
                c.push("0".into());
            } else {
                c.push(format!(
                    "EXISTS (SELECT 1 FROM messages m WHERE m.thread_id = t.id AND m.is_from_me = 0
                       AND lower(m.from_email) IN ({}))",
                    marks(emails.len())
                ));
                a.extend(emails.into_iter().map(Into::into));
            }
        }
        if let Some(p) = &f.subject_prefix {
            let esc = p
                .to_lowercase()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            c.push("lower(t.subject) LIKE ? ESCAPE '\\'".into());
            a.push(format!("{esc}%").into());
        }
        if !f.projects.is_empty() {
            c.push(format!(
                "t.id IN (SELECT tp.thread_id FROM thread_projects tp WHERE tp.name IN ({}))",
                marks(f.projects.len())
            ));
            a.extend(f.projects.iter().map(|s| s.clone().into()));
        }
        if !f.not_projects.is_empty() {
            c.push(format!(
                "t.id NOT IN (SELECT tp.thread_id FROM thread_projects tp WHERE tp.name IN ({}))",
                marks(f.not_projects.len())
            ));
            a.extend(f.not_projects.iter().map(|s| s.clone().into()));
        }
        if let Some(u) = f.unread {
            c.push("t.unread = ?".into());
            a.push((u as i64).into());
        }
        if let Some(h) = f.has_attachment {
            c.push(if h {
                "t.has_attachments != 0".into()
            } else {
                "t.has_attachments = 0".into()
            });
        }
        if let Some(acc) = &f.account {
            c.push("t.account_id = ?".into());
            a.push(acc.clone().into());
        }
        if let Some(q) = &f.query {
            let fq = fts_query(q, false)
                .ok_or_else(|| Error::InvalidParams(format!("'{q}' has no words to look for")))?;
            c.push(
                "t.id IN (SELECT m.thread_id FROM messages m WHERE m.fts_rowid IN
                   (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?))"
                    .into(),
            );
            a.push(fq.all.into());
        }
        let sql = format!(
            "SELECT t.id, t.number, t.status, t.subject, t.last_from, t.last_date FROM threads t
             WHERE {} ORDER BY t.last_date DESC",
            c.join(" AND ")
        );
        let r = self.r.lock().unwrap();
        let mut st = r.prepare(&sql)?;
        Ok(st
            .query_map(params_from_iter(a.iter()), |r| {
                Ok(PlanMatch {
                    id: r.get(0)?,
                    number: format!("M-{}", r.get::<_, i64>(1)?),
                    status: r.get(2)?,
                    subject: r.get(3)?,
                    last_from: r.get(4)?,
                    last_date: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Every address that wrote to the user, with its names, list and thread count.
    fn senders(&self) -> Result<Vec<Sender>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT lower(from_email), group_concat(DISTINCT from_name), max(list_id), count(DISTINCT thread_id)
             FROM messages WHERE is_from_me = 0 AND from_email != '' GROUP BY lower(from_email)",
        )?;
        Ok(st
            .query_map([], |r| {
                Ok(Sender {
                    email: r.get(0)?,
                    names: r
                        .get::<_, Option<String>>(1)?
                        .unwrap_or_default()
                        .replace(',', " "),
                    list: r.get(2)?,
                    threads: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// The senders a plan's "from" entries name (any entry), most threads first.
    pub fn resolve_senders(&self, from: &[String]) -> Result<Vec<CandidateSender>> {
        let entries: Vec<Entry> = from.iter().map(|f| senders::entry(f)).collect();
        let mut hits: Vec<CandidateSender> = self
            .senders()?
            .into_iter()
            .filter(|s| entries.iter().any(|e| senders::matches(e, s)))
            .map(candidate)
            .collect();
        hits.sort_by(|a, b| b.threads.cmp(&a.threads));
        Ok(hits)
    }

    /// For "from" entries that matched no one: the closest senders by their words
    /// (any word found), else the senders of mail with those words in its subject or
    /// sender fields.
    pub fn sender_suggestions(
        &self,
        from: &[String],
        limit: usize,
    ) -> Result<Vec<CandidateSender>> {
        let entries: Vec<Entry> = from.iter().map(|f| senders::entry(f)).collect();
        let all = self.senders()?;
        let close: Vec<CandidateSender> = senders::closest(&entries, &all, limit)
            .into_iter()
            .cloned()
            .map(candidate)
            .collect();
        if !close.is_empty() {
            return Ok(close);
        }
        let words: Vec<String> = from.iter().flat_map(|f| senders::words(f)).collect();
        self.senders_of_hits(&words, "{subject sender}", limit)
    }

    /// How a sender's recent subjects begin, the shared beginnings (2+ threads) first.
    fn sender_subjects(&self, email: &str) -> Result<Vec<String>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT t.subject FROM threads t WHERE t.id IN
               (SELECT m.thread_id FROM messages m WHERE lower(m.from_email) = ?1)
             ORDER BY t.last_date DESC LIMIT 300",
        )?;
        let rows: Vec<String> = st
            .query_map([email], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut counts: Vec<(String, usize)> = vec![];
        for s in rows {
            let p = subject_pattern(&s);
            match counts.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&p)) {
                Some((_, n)) => *n += 1,
                None if !p.is_empty() => counts.push((p, 1)),
                None => {}
            }
        }
        counts.retain(|(_, n)| *n >= 2);
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(counts.into_iter().take(2).map(|(p, _)| p).collect())
    }

    /// Senders whose mail has any of `words` in `columns`, most threads first.
    fn senders_of_hits(
        &self,
        words: &[String],
        columns: &str,
        limit: usize,
    ) -> Result<Vec<CandidateSender>> {
        let Some(q) = any_of(words, columns) else {
            return Ok(vec![]);
        };
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT lower(m.from_email), max(m.from_name), count(DISTINCT m.thread_id) AS n
             FROM messages_fts f JOIN messages m ON m.fts_rowid = f.rowid
             WHERE messages_fts MATCH ?1 AND m.is_from_me = 0 AND m.from_email != ''
             GROUP BY lower(m.from_email) ORDER BY n DESC LIMIT ?2",
        )?;
        Ok(st
            .query_map(params![q, limit as i64], |r| {
                Ok(CandidateSender {
                    email: r.get(0)?,
                    name: r.get(1)?,
                    threads: r.get(2)?,
                    subjects: vec![],
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// What the mailbox holds for an instruction's content words: senders named by
    /// them, and subject beginnings shared by threads mentioning them. Aggregates only.
    pub fn candidates(&self, sentence: &str) -> Result<Candidates> {
        let words = senders::content_words(sentence);
        if words.is_empty() {
            return Ok(Candidates::default());
        }
        let all = self.senders()?;
        let mut found: Vec<CandidateSender> =
            senders::closest(&[Entry::Words(words.clone())], &all, 5)
                .into_iter()
                .cloned()
                .map(candidate)
                .collect();
        for s in self.senders_of_hits(&words, "sender", 5)? {
            if found.len() < 5 && !found.iter().any(|f| f.email == s.email) {
                found.push(s);
            }
        }
        let mut subjects: Vec<CandidateSubject> = vec![];
        if let Some(q) = any_of(&words, "subject") {
            let r = self.r.lock().unwrap();
            let mut st = r.prepare_cached(
                "SELECT DISTINCT t.id, t.subject FROM messages_fts f
                 JOIN messages m ON m.fts_rowid = f.rowid JOIN threads t ON t.id = m.thread_id
                 WHERE messages_fts MATCH ?1 AND t.status != 'deleted' LIMIT 5000",
            )?;
            let rows: Vec<String> = st
                .query_map([q], |r| r.get(1))?
                .collect::<rusqlite::Result<_>>()?;
            let mut counts: Vec<(String, i64)> = vec![];
            for s in rows {
                let p = subject_pattern(&s);
                if p.is_empty() {
                    continue;
                }
                match counts.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&p)) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((p, 1)),
                }
            }
            counts.retain(|(_, n)| *n >= 2);
            counts.sort_by(|a, b| b.1.cmp(&a.1));
            subjects = counts
                .into_iter()
                .take(4)
                .map(|(prefix, threads)| CandidateSubject { prefix, threads })
                .collect();
        }
        for s in &mut found {
            s.subjects = self.sender_subjects(&s.email)?;
        }
        Ok(Candidates {
            senders: found,
            subjects,
        })
    }

    /// Logs a planned run with the threads each step will change.
    pub fn save_bulk_op(&self, op: &BulkOp, targets: &[Vec<String>]) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        tx.execute(
            "INSERT INTO bulk_ops (id, sentence, plan_json, summary, state, total, done, origin, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8)",
            params![
                op.id,
                op.sentence,
                op.plan.to_string(),
                op.summary,
                op.state,
                op.total,
                op.origin,
                op.created
            ],
        )?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO bulk_op_targets (op_id, step, thread_id) VALUES (?1, ?2, ?3)",
            )?;
            for (i, ids) in targets.iter().enumerate() {
                for id in ids {
                    ins.execute(params![op.id, i as i64, id])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn bulk_op(&self, id: &str) -> Result<BulkOp> {
        let r = self.r.lock().unwrap();
        r.query_row(
            &format!("SELECT {OP_COLS} FROM bulk_ops WHERE id = ?1"),
            [id],
            op_row,
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("operation {id}")))
    }

    /// Runs, newest first (planned-only ones left out).
    pub fn bulk_ops(&self, limit: i64) -> Result<Vec<BulkOp>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(&format!(
            "SELECT {OP_COLS} FROM bulk_ops WHERE state != 'planned' ORDER BY created DESC LIMIT ?1"
        ))?;
        Ok(st
            .query_map([limit], op_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// The threads step `step` of a run changes, in the order they were matched.
    pub fn bulk_op_targets(&self, id: &str, step: usize) -> Result<Vec<String>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT thread_id FROM bulk_op_targets WHERE op_id = ?1 AND step = ?2 ORDER BY rowid",
        )?;
        Ok(st
            .query_map(params![id, step as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Every thread a planned run would change (all steps, once each), newest first,
    /// with its current status: the preview's full list.
    pub fn bulk_op_threads(&self, id: &str) -> Result<Vec<PlanMatch>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT t.id, t.number, t.status, t.subject, t.last_from, t.last_date FROM threads t
             WHERE t.id IN (SELECT thread_id FROM bulk_op_targets WHERE op_id = ?1)
             ORDER BY t.last_date DESC",
        )?;
        Ok(st
            .query_map([id], |r| {
                Ok(PlanMatch {
                    id: r.get(0)?,
                    number: format!("M-{}", r.get::<_, i64>(1)?),
                    status: r.get(2)?,
                    subject: r.get(3)?,
                    last_from: r.get(4)?,
                    last_date: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Moves a run to `state`; `done` counts threads changed so far.
    pub fn set_bulk_op_state(
        &self,
        id: &str,
        state: &str,
        done: i64,
        error: Option<&str>,
    ) -> Result<()> {
        let w = self.w.lock().unwrap();
        let now = now_ms();
        w.execute(
            "UPDATE bulk_ops SET state = ?2, done = ?3, error = ?4,
               finished = CASE WHEN ?2 IN ('done', 'failed') THEN ?5 ELSE finished END,
               undone = CASE WHEN ?2 = 'undone' THEN ?5 ELSE undone END
             WHERE id = ?1",
            params![id, state, done, error, now],
        )?;
        Ok(())
    }

    /// Keeps each thread's state before the run first touched it (later steps keep the first).
    pub fn save_bulk_before(&self, id: &str, states: &[ThreadState]) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO bulk_op_before (op_id, thread_id, state_json) VALUES (?1, ?2, ?3)",
            )?;
            for s in states {
                ins.execute(params![id, s.id, serde_json::to_string(s)?])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Keeps each thread's put-off state before the run (later calls keep the first).
    pub fn save_bulk_snoozes(&self, id: &str, thread_ids: &[String]) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO bulk_op_snoozes (op_id, thread_id, until, for_date, label, woke)
                 SELECT ?1, t.id, z.until, z.for_date, z.label, z.woke
                 FROM threads t LEFT JOIN feed_snoozes z ON z.thread_id = t.id WHERE t.id = ?2",
            )?;
            for t in thread_ids {
                ins.execute(params![id, t])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Puts every logged thread back as put off (or not) as before the run; `None` when
    /// the run logged no put-off state.
    pub fn restore_bulk_snoozes(&self, id: &str) -> Result<Option<usize>> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let logged: i64 = tx.query_row(
            "SELECT COUNT(*) FROM bulk_op_snoozes WHERE op_id = ?1",
            [id],
            |r| r.get(0),
        )?;
        if logged == 0 {
            return Ok(None);
        }
        let mut n = tx.execute(
            "DELETE FROM feed_snoozes WHERE thread_id IN
               (SELECT thread_id FROM bulk_op_snoozes WHERE op_id = ?1 AND until IS NULL)",
            [id],
        )?;
        // `AND true` lets SQLite read the upsert after a SELECT.
        n += tx.execute(
            "INSERT INTO feed_snoozes (thread_id, until, for_date, label, woke)
             SELECT b.thread_id, b.until, b.for_date, b.label, b.woke FROM bulk_op_snoozes b
             JOIN threads t ON t.id = b.thread_id WHERE b.op_id = ?1 AND b.until IS NOT NULL AND true
             ON CONFLICT(thread_id) DO UPDATE SET until = excluded.until, for_date = excluded.for_date,
               label = excluded.label, woke = excluded.woke",
            [id],
        )?;
        tx.commit()?;
        Ok(Some(n))
    }

    /// The states a run's threads had before it, as `threads.restore` takes them.
    pub fn bulk_before(&self, id: &str) -> Result<Vec<ThreadState>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT state_json FROM bulk_op_before WHERE op_id = ?1 ORDER BY rowid",
        )?;
        let rows: Vec<String> = st
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        rows.iter().map(|s| Ok(serde_json::from_str(s)?)).collect()
    }
}

fn candidate(s: Sender) -> CandidateSender {
    CandidateSender {
        name: s.names.split_whitespace().collect::<Vec<_>>().join(" "),
        email: s.email,
        threads: s.threads,
        subjects: vec![],
    }
}

/// An FTS5 query for any of `words` in `columns` ("subject", "{subject sender}").
fn any_of(words: &[String], columns: &str) -> Option<String> {
    let terms: Vec<String> = words
        .iter()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\""))
        .collect();
    (!terms.is_empty()).then(|| format!("{columns} : ({})", terms.join(" OR ")))
}

/// How a subject begins, for grouping look-alike mail: "Re:"/"Fwd:" dropped, then
/// the text through an early colon ("Notes:"), else its first three words.
pub fn subject_pattern(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_lowercase();
        match ["re:", "fwd:", "fw:", "tr:"]
            .iter()
            .find(|p| lower.starts_with(*p))
        {
            Some(p) => s = s[p.len()..].trim_start(),
            None => break,
        }
    }
    if let Some(i) = s.find(':')
        && i > 0
        && s[..i].chars().count() <= 30
    {
        return s[..=i].to_string();
    }
    s.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
}

const OP_COLS: &str = "id, sentence, plan_json, summary, state, total, done, origin, error, created, finished, undone";

fn op_row(r: &rusqlite::Row) -> rusqlite::Result<BulkOp> {
    let plan: String = r.get(2)?;
    Ok(BulkOp {
        id: r.get(0)?,
        sentence: r.get(1)?,
        plan: serde_json::from_str(&plan).unwrap_or_default(),
        summary: r.get(3)?,
        state: r.get(4)?,
        total: r.get(5)?,
        done: r.get(6)?,
        origin: r.get(7)?,
        error: r.get(8)?,
        created: r.get(9)?,
        finished: r.get(10)?,
        undone: r.get(11)?,
    })
}

#[cfg(test)]
mod tests;
