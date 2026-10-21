//! What an Outlook account's sync keeps besides mail: one Microsoft Graph delta link
//! per mail folder (the Outlook counterpart of Gmail's history id), and per-message
//! label rewrites after a write-back, since one Outlook change (a move) gives each
//! message of a thread its own labels.

use super::Store;
use crate::error::Result;
use rusqlite::{OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};

/// `link` is the folder's delta link once a pass finished, else the page to resume.
pub(super) const MIGRATION: &str = "CREATE TABLE IF NOT EXISTS delta_links (
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  folder_id TEXT NOT NULL, link TEXT NOT NULL, PRIMARY KEY (account_id, folder_id));";

impl Store {
    /// Saved delta links of the account, by folder id.
    pub fn delta_links(&self, account_id: &str) -> Result<BTreeMap<String, String>> {
        let r = self.r.lock().unwrap();
        let mut st =
            r.prepare_cached("SELECT folder_id, link FROM delta_links WHERE account_id = ?1")?;
        Ok(st
            .query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_delta_link(&self, account_id: &str, folder_id: &str, link: &str) -> Result<()> {
        self.w.lock().unwrap().execute(
            "INSERT INTO delta_links (account_id, folder_id, link) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, folder_id) DO UPDATE SET link = excluded.link",
            params![account_id, folder_id, link],
        )?;
        Ok(())
    }

    pub fn remove_delta_link(&self, account_id: &str, folder_id: &str) -> Result<()> {
        self.w.lock().unwrap().execute(
            "DELETE FROM delta_links WHERE account_id = ?1 AND folder_id = ?2",
            params![account_id, folder_id],
        )?;
        Ok(())
    }

    /// Replaces the stored labels of these messages (by provider id), keeping their
    /// threads' label index current; statuses are left as they are, like
    /// [`Store::apply_label_delta`]. Returns the threads touched.
    pub fn set_message_labels(
        &self,
        account_id: &str,
        labels: &[(String, BTreeSet<String>)],
    ) -> Result<Vec<String>> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let mut threads = BTreeSet::new();
        for (provider_id, ids) in labels {
            let tid: Option<String> = tx
                .prepare_cached(
                    "SELECT thread_id FROM messages WHERE account_id = ?1 AND provider_id = ?2",
                )?
                .query_row(params![account_id, provider_id], |r| r.get(0))
                .optional()?;
            let Some(tid) = tid else { continue };
            tx.prepare_cached(
                "UPDATE messages SET labels_json = ?3 WHERE account_id = ?1 AND provider_id = ?2",
            )?
            .execute(params![
                account_id,
                provider_id,
                serde_json::to_string(ids)?
            ])?;
            threads.insert(tid);
        }
        for t in &threads {
            Self::index_thread_labels(&tx, t)?;
        }
        tx.commit()?;
        Ok(threads.into_iter().collect())
    }

    /// Imported threads whose status came from a message that isn't in the Inbox (a
    /// sent reply listed before the mail it answers) take the status their whole
    /// thread gives, as if imported together. Threads with local edits waiting are
    /// left alone.
    pub fn settle_imported(&self, account_id: &str, thread_ids: &[String]) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        for tid in thread_ids {
            let row: Option<(String, i64, bool)> = tx
                .query_row(
                    "SELECT status, last_date, unread FROM threads WHERE id = ?1",
                    [tid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
                )
                .optional()?;
            let Some((status, last_date, unread)) = row else {
                continue;
            };
            let pending: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM ops WHERE key = ?1)",
                [tid],
                |r| r.get(0),
            )?;
            if status != "archive" || pending {
                continue;
            }
            let (in_inbox, names) = Self::thread_remote_labels(&tx, account_id, tid)?;
            if !in_inbox {
                continue;
            }
            let label = Self::status_from_labels(&tx, &names)?;
            let next = super::initial_status(
                true,
                names.contains("TRASH"),
                unread,
                label.as_deref(),
                last_date,
                crate::model::now_ms(),
                super::SyncMode::Initial,
            );
            tx.execute(
                "UPDATE threads SET status = ?2 WHERE id = ?1",
                params![tid, next],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Provider ids of the account's messages carrying `label_id`.
    pub fn messages_with_label(&self, account_id: &str, label_id: &str) -> Result<Vec<String>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT m.provider_id FROM messages m, json_each(m.labels_json) j
             WHERE m.account_id = ?1 AND j.value = ?2",
        )?;
        Ok(st
            .query_map(params![account_id, label_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// The name of a label, `None` when it isn't known.
    pub fn label_name(&self, account_id: &str, label_id: &str) -> Result<Option<String>> {
        let r = self.r.lock().unwrap();
        Ok(r.query_row(
            "SELECT name FROM labels WHERE account_id = ?1 AND label_id = ?2",
            params![account_id, label_id],
            |r| r.get(0),
        )
        .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{IncomingMessage, SyncMode};
    use rstest::{fixture, rstest};

    struct Env {
        _dir: tempfile::TempDir,
        store: Store,
        acc: String,
    }

    #[fixture]
    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        let acc = store.add_account("me@example.com", "outlook").unwrap().id;
        let raw = b"From: a@example.com\r\nTo: me@example.com\r\nSubject: Hi\r\n\r\nHello\r\n";
        store
            .ingest(
                &acc,
                "me@example.com",
                &IncomingMessage {
                    provider_id: "m1".into(),
                    provider_thread_id: "c1".into(),
                    label_ids: vec!["INBOX".into(), "cat:Clients".into()],
                    internal_date: 1,
                    parsed: crate::mime::parse(raw).unwrap(),
                },
                SyncMode::Incremental,
            )
            .unwrap();
        Env {
            _dir: dir,
            store,
            acc,
        }
    }

    #[rstest]
    fn a_delta_link_is_kept_per_folder(env: Env) {
        env.store.set_delta_link(&env.acc, "f1", "link-a").unwrap();
        env.store.set_delta_link(&env.acc, "f1", "link-b").unwrap();
        assert_eq!(
            env.store.delta_links(&env.acc).unwrap(),
            BTreeMap::from([("f1".to_string(), "link-b".to_string())])
        );
    }

    #[rstest]
    fn a_removed_delta_link_is_gone(env: Env) {
        env.store.set_delta_link(&env.acc, "f1", "link-a").unwrap();
        env.store.remove_delta_link(&env.acc, "f1").unwrap();
        assert!(env.store.delta_links(&env.acc).unwrap().is_empty());
    }

    #[rstest]
    fn message_labels_are_replaced(env: Env) {
        let new: BTreeSet<String> = ["folder:F".to_string()].into();
        env.store
            .set_message_labels(&env.acc, &[("m1".into(), new.clone())])
            .unwrap();
        assert_eq!(
            env.store.message_label_ids(&env.acc, "m1").unwrap(),
            Some(new)
        );
    }

    #[rstest]
    fn messages_are_found_by_label(env: Env) {
        assert_eq!(
            env.store
                .messages_with_label(&env.acc, "cat:Clients")
                .unwrap(),
            vec!["m1"]
        );
    }
}
