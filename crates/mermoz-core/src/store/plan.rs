//! "Ask Mermoz to do" storage: a plan's filter resolved against the local mailbox, and
//! the operation log that keeps each run's targets and the threads' states before it,
//! so one Undo reverts the whole run, after a restart too.

use super::{Store, fts_query};
use crate::error::{Error, Result};
use crate::model::{ThreadState, now_ms};
use crate::plan::{Filter, cutoff};
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
            // An address matches exactly, a domain its addresses and subdomains, else a name.
            let mut any: Vec<String> = vec![];
            for s in &f.from {
                if s.contains('@') {
                    any.push("lower(m.from_email) = ?".into());
                    a.push(s.clone().into());
                } else if s.contains('.') && !s.contains(' ') {
                    any.push("(lower(m.from_email) LIKE ? OR lower(m.from_email) LIKE ?)".into());
                    a.push(format!("%@{s}").into());
                    a.push(format!("%.{s}").into());
                } else {
                    any.push("lower(m.from_name) LIKE ?".into());
                    a.push(format!("%{s}%").into());
                }
            }
            c.push(format!(
                "EXISTS (SELECT 1 FROM messages m WHERE m.thread_id = t.id AND m.is_from_me = 0 AND ({}))",
                any.join(" OR ")
            ));
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
