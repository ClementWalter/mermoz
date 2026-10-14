//! What search-by-meaning reads from the mail database: which threads need a vector
//! (and in what order), each thread's document, and the threads a search found.
//! Indexing reads on the background connection, so it never queues the UI's reads.

use super::{THREAD_COLS, attach, readings, thread_row};
use crate::error::Result;
use crate::model::ThreadSummary;
use crate::semantic::{DocMessage, doc_text};
use rusqlite::params_from_iter;
use std::collections::HashMap;

/// A thread's text without the quoted history (`>` lines) a plain-text reply carries.
fn unquoted(body: &str) -> String {
    body.lines()
        .filter(|l| !l.trim_start().starts_with('>'))
        .collect::<Vec<_>>()
        .join("\n")
}

impl super::Store {
    /// Every thread worth a vector with its signature (`last_date:message_count`, which
    /// changes with new mail), newest first. Deleted threads are left out.
    pub fn semantic_sigs(&self) -> Result<Vec<(String, String)>> {
        let bg = self.bg.lock().unwrap();
        let mut st = bg.prepare_cached(
            "SELECT id, last_date || ':' || message_count FROM threads
             WHERE status != 'deleted' ORDER BY last_date DESC",
        )?;
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Each thread's document (see [`doc_text`]), in the order of `ids`; unknown ids are
    /// left out. A message's stored reading is its text; mail not read yet uses its
    /// plain body without quoted lines.
    pub fn semantic_docs(&self, ids: &[String]) -> Result<Vec<(String, String)>> {
        let bg = self.bg.lock().unwrap();
        // Only the first two messages' text is read: later rows skip the body columns.
        let mut st = bg.prepare_cached(&format!(
            "SELECT t.subject, m.from_name, m.from_email, m.is_from_me,
               CASE WHEN ROW_NUMBER() OVER (ORDER BY m.date, m.rowid) <= 2
                    THEN COALESCE(r.text, m.body_text) END,
               r.text IS NOT NULL
             FROM threads t JOIN messages m ON m.thread_id = t.id {}{}
             WHERE t.id = ?1 ORDER BY m.date, m.rowid",
            readings::JOIN_READING_OF_M,
            crate::reading::VERSION
        ))?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let mut subject = String::new();
            let mut msgs = vec![];
            let rows = st.query_map([id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, bool>(5)?,
                ))
            })?;
            for row in rows {
                let (s, name, email, me, text, read) = row?;
                subject = s;
                let text = text.unwrap_or_default();
                msgs.push(DocMessage {
                    sender: if name.trim().is_empty() { email } else { name },
                    is_from_me: me,
                    text: if read { text } else { unquoted(&text) },
                });
            }
            if !msgs.is_empty() {
                out.push((id.clone(), doc_text(&subject, &msgs)));
            }
        }
        Ok(out)
    }

    /// The threads with these ids, in the order given; unknown ids and hidden
    /// accounts' threads are left out.
    pub fn threads_by_ids(&self, ids: &[String]) -> Result<Vec<ThreadSummary>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let visible = self.visible();
        let r = self.r.lock().unwrap();
        let marks = vec!["?"; ids.len()].join(",");
        let mut st = r.prepare(&format!(
            "SELECT {THREAD_COLS} FROM threads t WHERE t.id IN ({marks}){visible}"
        ))?;
        let mut rows = st
            .query_map(params_from_iter(ids.iter()), thread_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let order: HashMap<&str, usize> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        rows.sort_by_key(|t| order.get(t.id.as_str()).copied().unwrap_or(usize::MAX));
        attach(&r, &mut rows)?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests;
