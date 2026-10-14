//! Each message's reading (what its bubble shows, see [`crate::reading`]) is computed
//! once, at ingest, and kept: opening a thread reads it back instead of parsing every
//! message again. Rows carry the pipeline [`VERSION`] that wrote them; older rows (and
//! mail stored before this table existed) are recomputed in the background by
//! [`Store::read_bodies_batch`], and read on the fly until then.
//!
//! Rows are keyed by the message's rowid, so ingest appends to the table instead of
//! writing into a random page of an index of every message id. The message id is kept
//! alongside and every read matches both: rowids can be renumbered (VACUUM), and a
//! row that no longer matches reads as missing and is worked out again.

use super::{Store, bodies, par_map};
use crate::error::Result;
use crate::reading::{self, Mail, Reading, VERSION};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) const MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS message_reading (
  msg INTEGER PRIMARY KEY, message_id TEXT NOT NULL,
  ver INTEGER NOT NULL, text TEXT NOT NULL, flags INTEGER NOT NULL, forwarded_from TEXT);
CREATE TRIGGER IF NOT EXISTS message_reading_gone AFTER DELETE ON messages
  BEGIN DELETE FROM message_reading WHERE msg = old.rowid; END
"#;

/// The stored reading of message `m`, current and still its own (`r` is the reading).
pub(super) const JOIN_READING_OF_M: &str =
    "LEFT JOIN message_reading r ON r.msg = m.rowid AND r.message_id = m.id AND r.ver = ";

/// Stores the reading of the message at `rowid` with id `message_id`.
pub(super) fn save(conn: &Connection, rowid: i64, message_id: &str, r: &Reading) -> Result<()> {
    conn.prepare_cached(
        "INSERT OR REPLACE INTO message_reading
           (msg, message_id, ver, text, flags, forwarded_from, reaction, reaction_quote)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?
    .execute(params![
        rowid,
        message_id,
        VERSION,
        r.text,
        r.flags(),
        r.forwarded_from,
        r.reaction,
        r.reaction_quote
    ])?;
    Ok(())
}

/// A message read from its stored parts (`listed`: it came through a mailing list).
pub(super) fn read(subject: &str, text: &str, html: Option<&str>, listed: bool) -> Reading {
    reading::read(&Mail {
        subject,
        text,
        html,
        listed,
    })
}

/// (rowid, id, subject, body_text, html, listed, from_email) of a message to read.
type Pending = (i64, String, String, String, Option<String>, bool, String);

impl Store {
    /// Reads up to `limit` messages after rowid `after` whose reading is missing or
    /// from an older pipeline, then stores them in one short write; the parsing runs
    /// outside the write lock. Returns the last rowid looked at, or None once all are
    /// current.
    pub fn read_bodies_batch(&self, after: i64, limit: i64) -> Result<Option<i64>> {
        let (pending, images): (Vec<Pending>, Vec<Vec<(String, bool)>>) = {
            let bg = self.bg.lock().unwrap();
            // Once done for this version it stays done: new mail is read at ingest.
            let done: Option<String> = bg
                .query_row(
                    "SELECT value FROM meta WHERE key = 'reading_version'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            if done.as_deref() == Some(VERSION.to_string().as_str()) {
                return Ok(None);
            }
            let pending = bg.prepare_cached(&format!(
                "SELECT m.rowid, m.id, m.subject, m.body_text, {}, m.list_id IS NOT NULL, m.from_email
                 FROM messages m WHERE m.rowid > ?1 AND NOT EXISTS
                   (SELECT 1 FROM message_reading r
                    WHERE r.msg = m.rowid AND r.message_id = m.id AND r.ver = ?2)
                 ORDER BY m.rowid LIMIT ?3",
                bodies::HTML_OF_M
            ))?
            .query_map(params![after, VERSION, limit], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get::<_, i64>(5)? != 0,
                    r.get(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<Pending>>>()?;
            // Their images are looked at here, on the background connection: reading
            // the files never holds the write lock.
            let images = pending
                .iter()
                .map(|(_, id, _, _, html, _, from)| {
                    let list = super::images::unclassified(&bg, id)?;
                    super::images::classify_stored(&bg, from, id, html.as_deref(), &list)
                })
                .collect::<Result<Vec<_>>>()?;
            (pending, images)
        };
        let Some(last) = pending.last().map(|p| p.0) else {
            self.w.lock().unwrap().execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('reading_version', ?1)",
                [VERSION.to_string()],
            )?;
            return Ok(None);
        };
        let readings = par_map(&pending, |(_, _, subject, text, html, listed, _)| {
            read(subject, text, html.as_deref(), *listed)
        });
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        for (((rowid, id, ..), r), flags) in pending.iter().zip(&readings).zip(&images) {
            // The message may have been deleted since it was read.
            let exists: bool = tx
                .prepare_cached("SELECT 1 FROM messages WHERE rowid = ?1 AND id = ?2")?
                .exists(params![rowid, id])?;
            if exists {
                save(&tx, *rowid, id, r)?;
                for (aid, decorative) in flags {
                    tx.prepare_cached("UPDATE attachments SET decorative = ?2 WHERE id = ?1")?
                        .execute(params![aid, *decorative as i64])?;
                }
            }
        }
        tx.commit()?;
        Ok(Some(last))
    }
}

#[cfg(test)]
mod tests {
    use crate::mime::ParsedMessage;
    use crate::model::Address;
    use crate::store::{IncomingMessage, Store, SyncMode};
    use rstest::*;

    struct Env {
        _dir: tempfile::TempDir,
        store: Store,
        thread: String,
    }

    fn incoming(pid: &str, date: i64, text: &str) -> IncomingMessage {
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
                subject: "Re: Lunch".into(),
                body_text: text.into(),
                ..Default::default()
            },
        }
    }

    const REPLY: &str = "Yes, noon works.\r\n\r\nOn Mon, Oct 12, 2026 at 9:00 AM Bo <bo@example.com> wrote:\r\n> Lunch?\r\n";

    #[fixture]
    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        let acc = store.add_account("me@example.com", "gmail").unwrap().id;
        let thread = store
            .ingest_many(
                &acc,
                "me@example.com",
                &[incoming("m1", 1_700_000_000_000, REPLY)],
                SyncMode::Initial,
            )
            .unwrap()
            .remove(0);
        Env {
            _dir: dir,
            store,
            thread,
        }
    }

    fn stored(env: &Env) -> i64 {
        env.store
            .w
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM message_reading", [], |r| r.get(0))
            .unwrap()
    }

    fn forget(env: &Env) {
        env.store
            .w
            .lock()
            .unwrap()
            .execute_batch(
                "DELETE FROM message_reading; DELETE FROM meta WHERE key = 'reading_version'",
            )
            .unwrap();
    }

    #[rstest]
    fn ingest_stores_the_reading(env: Env) {
        assert_eq!(stored(&env), 1);
    }

    #[rstest]
    fn a_thread_shows_the_stored_reading(env: Env) {
        assert_eq!(
            env.store.messages(&env.thread).unwrap()[0].body_new,
            "Yes, noon works."
        );
    }

    #[rstest]
    fn a_message_without_a_stored_reading_is_read_on_the_fly(env: Env) {
        forget(&env);
        assert_eq!(
            env.store.messages(&env.thread).unwrap()[0].body_new,
            "Yes, noon works."
        );
    }

    #[rstest]
    fn the_flags_come_back_with_the_reading(env: Env) {
        assert!(env.store.messages(&env.thread).unwrap()[0].has_quoted);
    }

    #[rstest]
    fn the_background_pass_fills_missing_readings(env: Env) {
        forget(&env);
        env.store.read_bodies_batch(0, 10).unwrap();
        assert_eq!(stored(&env), 1);
    }

    #[rstest]
    fn the_background_pass_ends_once_all_are_current(env: Env) {
        env.store.read_bodies_batch(0, 10).unwrap();
        assert_eq!(env.store.read_bodies_batch(0, 10).unwrap(), None);
    }

    #[rstest]
    fn a_deleted_message_takes_its_reading_along(env: Env) {
        env.store
            .w
            .lock()
            .unwrap()
            .execute_batch("DELETE FROM messages")
            .unwrap();
        assert_eq!(stored(&env), 0);
    }

    #[rstest]
    fn a_reading_left_on_a_renumbered_row_is_not_shown(env: Env) {
        env.store
            .w
            .lock()
            .unwrap()
            .execute_batch("UPDATE message_reading SET message_id = 'msg_other', text = 'not mine'")
            .unwrap();
        assert_eq!(
            env.store.messages(&env.thread).unwrap()[0].body_new,
            "Yes, noon works."
        );
    }

    #[rstest]
    fn an_older_pipeline_reading_is_recomputed(env: Env) {
        env.store
            .w
            .lock()
            .unwrap()
            .execute_batch("UPDATE message_reading SET ver = 0, text = 'stale'")
            .unwrap();
        env.store.read_bodies_batch(0, 10).unwrap();
        assert_eq!(
            env.store.messages(&env.thread).unwrap()[0].body_new,
            "Yes, noon works."
        );
    }
}
