//! Drafts the user writes in Gmail itself (web, phone). Gmail keeps them as messages
//! labelled `DRAFT`; here they are drafts (origin `gmail`), never messages, so a draft
//! never reads as sent mail. Each row follows one Gmail message (`provider_id`): a new
//! version of the draft is a new message in Gmail, the old one is deleted, and sending
//! it turns it into a sent message, which syncs in as mail.

use super::Store;
use crate::error::Result;
use crate::model::{Address, new_id, now_ms};
use crate::store::IncomingMessage;
use rusqlite::{Connection, OptionalExtension, params};

/// The Gmail message each Gmail draft stands for.
pub(super) const MIGRATION: &str = "ALTER TABLE drafts ADD COLUMN provider_id TEXT;
ALTER TABLE drafts ADD COLUMN provider_thread_id TEXT;
CREATE INDEX IF NOT EXISTS drafts_provider ON drafts(account_id, provider_id);";

pub const ORIGIN: &str = "gmail";

/// Gmail shows this message as a draft, not as mail.
pub fn is_draft(labels: &[String]) -> bool {
    labels.iter().any(|l| l == "DRAFT")
}

fn emails(v: &[Address]) -> Vec<String> {
    v.iter().map(|a| a.email.clone()).collect()
}

/// Stores or refreshes the draft a Gmail `DRAFT` message is; returns the Mermoz thread
/// it answers, when that thread is stored.
pub(super) fn upsert_tx(
    tx: &Connection,
    account_id: &str,
    msg: &IncomingMessage,
) -> Result<Option<String>> {
    let thread_id: Option<String> = tx
        .query_row(
            "SELECT id FROM threads WHERE account_id = ?1 AND provider_thread_id = ?2",
            params![account_id, msg.provider_thread_id],
            |r| r.get(0),
        )
        .optional()?;
    let p = &msg.parsed;
    let (to, cc) = (
        serde_json::to_string(&emails(&p.to))?,
        serde_json::to_string(&emails(&p.cc))?,
    );
    let when = if msg.internal_date > 0 {
        msg.internal_date
    } else {
        now_ms()
    };
    let updated = tx.execute(
        "UPDATE drafts SET thread_id = ?3, to_json = ?4, cc_json = ?5, subject = ?6, body = ?7, updated = ?8,
           provider_thread_id = ?9
         WHERE account_id = ?1 AND provider_id = ?2",
        params![account_id, msg.provider_id, thread_id, to, cc, p.subject, p.body_text, when, msg.provider_thread_id],
    )?;
    if updated == 0 {
        tx.execute(
            "INSERT INTO drafts (id, account_id, thread_id, to_json, cc_json, subject, body, state, origin, created, updated, provider_id, provider_thread_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'draft', ?8, ?9, ?9, ?10, ?11)",
            params![new_id("dft"), account_id, thread_id, to, cc, p.subject, p.body_text, ORIGIN, when, msg.provider_id, msg.provider_thread_id],
        )?;
    }
    Ok(thread_id)
}

/// Gmail drafts answering a thread stored only now (the import brings a reply draft
/// before the older mail it answers) join it.
pub(super) fn attach_tx(
    tx: &Connection,
    account_id: &str,
    provider_thread_id: &str,
    thread_id: &str,
) -> Result<()> {
    tx.execute(
        "UPDATE drafts SET thread_id = ?3 WHERE account_id = ?1 AND provider_thread_id = ?2 AND thread_id IS NULL",
        params![account_id, provider_thread_id, thread_id],
    )?;
    Ok(())
}

/// Forgets the Gmail draft this message was (deleted, replaced by a newer version, or
/// sent); returns its thread when there was one. The draft's own rows only: nothing
/// it was sent as is touched.
pub(super) fn remove_tx(
    tx: &Connection,
    account_id: &str,
    provider_id: &str,
) -> Result<Option<Option<String>>> {
    let found: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT id, thread_id FROM drafts WHERE account_id = ?1 AND provider_id = ?2 AND origin = ?3",
            params![account_id, provider_id, ORIGIN],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((id, thread)) = found else {
        return Ok(None);
    };
    tx.execute("DELETE FROM drafts WHERE id = ?1", [&id])?;
    Ok(Some(thread))
}

impl Store {
    /// Forgets the Gmail draft held for this Gmail message; `Some(thread)` when there
    /// was one (its thread, if stored).
    pub fn remove_gmail_draft(
        &self,
        account_id: &str,
        provider_id: &str,
    ) -> Result<Option<Option<String>>> {
        let w = self.w.lock().unwrap();
        remove_tx(&w, account_id, provider_id)
    }

    /// The Gmail message behind a draft written in Gmail.
    pub fn draft_provider_id(&self, draft_id: &str) -> Result<Option<String>> {
        let r = self.r.lock().unwrap();
        Ok(r.query_row(
            "SELECT provider_id FROM drafts WHERE id = ?1",
            [draft_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten())
    }

    /// A Gmail draft is held for this Gmail message.
    pub fn is_gmail_draft(&self, account_id: &str, provider_id: &str) -> Result<bool> {
        let r = self.r.lock().unwrap();
        Ok(r.query_row(
            "SELECT 1 FROM drafts WHERE account_id = ?1 AND provider_id = ?2",
            params![account_id, provider_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
    }

    /// Unsent drafts answering these threads, oldest first.
    pub fn thread_drafts(&self, thread_ids: &[String]) -> Result<Vec<crate::model::Draft>> {
        let ids: Vec<String> = {
            let r = self.r.lock().unwrap();
            let mut st = r.prepare_cached(
                "SELECT id FROM drafts WHERE thread_id = ?1 AND state = 'draft' ORDER BY created",
            )?;
            let mut out = vec![];
            for t in thread_ids {
                out.extend(
                    st.query_map([t], |r| r.get(0))?
                        .collect::<rusqlite::Result<Vec<String>>>()?,
                );
            }
            out
        };
        ids.iter().map(|id| self.draft(id)).collect()
    }

    /// Turns Gmail drafts stored as messages (before drafts were told apart) into
    /// drafts, once: their threads are recomputed, and a thread that only held a draft
    /// goes. Returns how many were moved.
    pub(super) fn adopt_stored_gmail_drafts(&self) -> Result<usize> {
        const KEY: &str = "gmail_drafts_adopted";
        if self.setting(KEY)?.is_some() {
            return Ok(0);
        }
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        type Row = (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            Option<i64>,
        );
        let rows: Vec<Row> = {
            let mut st = tx.prepare(
                "SELECT id, account_id, provider_id, thread_id, to_json, cc_json, subject, body_text, date, fts_rowid
                 FROM messages WHERE labels_json LIKE '%\"DRAFT\"%'",
            )?;
            st.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        for (mid, account, pid, thread, to, cc, subject, body, date, fts) in &rows {
            let addr = |j: &str| -> Result<String> {
                let v: Vec<Address> = serde_json::from_str(j).unwrap_or_default();
                Ok(serde_json::to_string(&emails(&v))?)
            };
            tx.execute(
                "INSERT INTO drafts (id, account_id, thread_id, to_json, cc_json, subject, body, state, origin, created, updated, provider_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'draft', ?8, ?9, ?9, ?10)",
                params![new_id("dft"), account, thread, addr(to)?, addr(cc)?, subject, body, ORIGIN, date, pid],
            )?;
            if let Some(f) = fts {
                tx.execute("DELETE FROM messages_fts WHERE rowid = ?1", [f])?;
            }
            tx.execute("DELETE FROM messages WHERE id = ?1", [mid])?;
            let left: i64 = tx.query_row(
                "SELECT COUNT(*) FROM messages WHERE thread_id = ?1",
                [thread],
                |r| r.get(0),
            )?;
            if left == 0 {
                tx.execute("DELETE FROM threads WHERE id = ?1", [thread])?;
            } else {
                Store::refresh_thread(&tx, thread)?;
            }
        }
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, '1') ON CONFLICT(key) DO NOTHING",
            [KEY],
        )?;
        tx.commit()?;
        Ok(rows.len())
    }
}

#[cfg(test)]
mod tests;
