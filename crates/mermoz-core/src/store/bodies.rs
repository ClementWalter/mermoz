//! HTML bodies live in their own table, out of the message row.
//!
//! SQLite stores a row's columns in order and spills a large value to a chain of
//! overflow pages; reading any later column (labels, auth, Bcc, the FTS rowid) walks
//! that whole chain. With newsletters' HTML in the row, opening a thread or ingesting
//! a reply read every earlier message's HTML from disk. Rows written before this table
//! existed are moved over in small batches by [`Store::move_html_batch`]; until then a
//! message's HTML is in either place.

use super::Store;
use crate::error::Result;
use rusqlite::params;

pub(super) const MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS message_html (
  message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  html TEXT NOT NULL)
"#;

/// A message's HTML, wherever it is stored (`m` is the message row).
pub(super) const HTML_OF_M: &str =
    "COALESCE((SELECT h.html FROM message_html h WHERE h.message_id = m.id), m.body_html)";

/// Whether a message has HTML, without reading it (`m` is the message row).
pub(super) const HAS_HTML_OF_M: &str =
    "(m.body_html IS NOT NULL OR EXISTS (SELECT 1 FROM message_html h WHERE h.message_id = m.id))";

impl Store {
    /// Moves the HTML of up to `limit` messages after rowid `after` out of their rows,
    /// in one short write. Returns the last rowid looked at, or None once none is left.
    pub fn move_html_batch(&self, after: i64, limit: i64) -> Result<Option<i64>> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        // `body_html IS NOT NULL` reads only the row header, never the HTML itself.
        let rows: Vec<(i64, String)> = tx
            .prepare_cached(
                "SELECT rowid, id FROM messages WHERE rowid > ?1 AND body_html IS NOT NULL
                 ORDER BY rowid LIMIT ?2",
            )?
            .query_map(params![after, limit], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let Some(&(last, _)) = rows.last() else {
            return Ok(None);
        };
        for (_, id) in &rows {
            tx.prepare_cached(
                "INSERT OR IGNORE INTO message_html (message_id, html)
                 SELECT id, body_html FROM messages WHERE id = ?1",
            )?
            .execute([id])?;
            tx.prepare_cached("UPDATE messages SET body_html = NULL WHERE id = ?1")?
                .execute([id])?;
        }
        tx.commit()?;
        Ok(Some(last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::ParsedMessage;
    use crate::model::Address;
    use crate::store::{IncomingMessage, SyncMode};
    use rstest::*;

    struct Env {
        _dir: tempfile::TempDir,
        store: Store,
        thread: String,
        /// Message ids: the first has HTML, the second is plain text.
        ids: Vec<String>,
    }

    fn incoming(pid: &str, date: i64, html: Option<&str>) -> IncomingMessage {
        IncomingMessage {
            provider_id: pid.into(),
            provider_thread_id: "t1".into(),
            label_ids: vec!["INBOX".into()],
            internal_date: date,
            parsed: ParsedMessage {
                from: Some(Address {
                    name: "Ada".into(),
                    email: "ada@example.com".into(),
                }),
                subject: "Hello".into(),
                body_text: "Hi".into(),
                body_html: html.map(str::to_string),
                ..Default::default()
            },
        }
    }

    #[fixture]
    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        let acc = store.add_account("me@example.com", "gmail").unwrap().id;
        let msgs = [
            incoming("m1", 1_700_000_000_000, Some("<p>Hi</p>")),
            incoming("m2", 1_700_000_100_000, None),
        ];
        let thread = store
            .ingest_many(&acc, "me@example.com", &msgs, SyncMode::Initial)
            .unwrap()
            .remove(0);
        let ids = store
            .messages(&thread)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        Env {
            _dir: dir,
            store,
            thread,
            ids,
        }
    }

    /// Puts the HTML back in the message row, where older mailboxes still have it.
    fn inline_html(env: &Env) {
        env.store
            .w
            .lock()
            .unwrap()
            .execute_batch(
                "UPDATE messages SET body_html =
                   (SELECT html FROM message_html h WHERE h.message_id = messages.id);
                 DELETE FROM message_html;",
            )
            .unwrap();
    }

    fn row_html_count(env: &Env) -> i64 {
        env.store
            .r
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE body_html IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[rstest]
    fn ingest_keeps_html_out_of_the_row(env: Env) {
        assert_eq!(row_html_count(&env), 0);
    }

    #[rstest]
    fn ingested_html_reads_back(env: Env) {
        assert_eq!(
            env.store.message_html(&env.ids[0]).unwrap().0.as_deref(),
            Some("<p>Hi</p>")
        );
    }

    #[rstest]
    fn plain_message_has_no_html(env: Env) {
        assert_eq!(env.store.message_html(&env.ids[1]).unwrap().0, None);
    }

    #[rstest]
    fn plain_message_is_not_html(env: Env) {
        assert!(!env.store.messages(&env.thread).unwrap()[1].has_html);
    }

    #[rstest]
    fn moved_html_counts_as_html(env: Env) {
        assert!(env.store.messages(&env.thread).unwrap()[0].has_html);
    }

    #[rstest]
    fn inline_html_still_reads(env: Env) {
        inline_html(&env);
        assert_eq!(
            env.store.message_html(&env.ids[0]).unwrap().0.as_deref(),
            Some("<p>Hi</p>")
        );
    }

    #[rstest]
    fn inline_html_still_counts_as_html(env: Env) {
        inline_html(&env);
        assert!(env.store.messages(&env.thread).unwrap()[0].has_html);
    }

    #[rstest]
    fn a_batch_empties_the_rows(env: Env) {
        inline_html(&env);
        env.store.move_html_batch(0, 10).unwrap();
        assert_eq!(row_html_count(&env), 0);
    }

    #[rstest]
    fn moved_inline_html_reads_back(env: Env) {
        inline_html(&env);
        env.store.move_html_batch(0, 10).unwrap();
        assert_eq!(
            env.store.message_html(&env.ids[0]).unwrap().0.as_deref(),
            Some("<p>Hi</p>")
        );
    }

    #[rstest]
    fn nothing_left_ends_the_move(env: Env) {
        assert_eq!(env.store.move_html_batch(0, 10).unwrap(), None);
    }

    #[rstest]
    fn deleting_a_message_deletes_its_html(env: Env) {
        let w = env.store.w.lock().unwrap();
        w.execute("DELETE FROM messages WHERE id = ?1", [&env.ids[0]])
            .unwrap();
        let n: i64 = w
            .query_row("SELECT COUNT(*) FROM message_html", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
}
