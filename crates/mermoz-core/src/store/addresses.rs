//! Every address seen as a sender or recipient, for the composer's recipient check.
//!
//! Asking the messages themselves meant a `LIKE` over every row's recipients: a full
//! scan for any address never written to (1 s warm, 6 s cold on a 48k-message mailbox),
//! on each recipient typed. New mail adds its addresses here as it is stored; mail
//! stored before is added in short background batches ([`Store::index_addresses_batch`]),
//! and the check asks the messages until that is done.

use super::Store;
use crate::error::Result;
use crate::model::Address;
use rusqlite::{Connection, OptionalExtension, params};

pub(super) const MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS known_addresses (email TEXT PRIMARY KEY) WITHOUT ROWID;
-- An empty mailbox is complete from the start: ingest keeps it so.
INSERT OR IGNORE INTO meta (key, value)
  SELECT 'addresses_indexed', '1' WHERE NOT EXISTS (SELECT 1 FROM messages)
"#;

/// Records a stored message's sender and recipients.
pub(super) fn note<'a>(tx: &Connection, addrs: impl Iterator<Item = &'a Address>) -> Result<()> {
    let mut st =
        tx.prepare_cached("INSERT OR IGNORE INTO known_addresses (email) VALUES (lower(?1))")?;
    for a in addrs.filter(|a| !a.email.is_empty()) {
        st.execute([&a.email])?;
    }
    Ok(())
}

fn indexed(conn: &Connection) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM meta WHERE key = 'addresses_indexed'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

impl Store {
    /// Whether the user has exchanged mail with this address.
    pub fn known_address(&self, email: &str) -> Result<bool> {
        let r = self.r.lock().unwrap();
        let email = email.to_lowercase();
        if indexed(&r)? {
            return Ok(r
                .prepare_cached("SELECT 1 FROM known_addresses WHERE email = ?1")?
                .query_row([&email], |_| Ok(()))
                .optional()?
                .is_some());
        }
        let like = format!("%\"{email}\"%");
        Ok(r
            .query_row(
                "SELECT 1 FROM messages WHERE lower(from_email) = ?1 OR lower(to_json) LIKE ?2 OR lower(cc_json) LIKE ?2 LIMIT 1",
                params![email, like],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Adds the addresses of messages with rowids in (after, after + span] in one short
    /// write. Returns the next `after`, or None once every message is covered.
    pub fn index_addresses_batch(&self, after: i64, span: i64) -> Result<Option<i64>> {
        let mut w = self.w.lock().unwrap();
        if indexed(&w)? {
            return Ok(None);
        }
        let tx = w.transaction()?;
        let last: i64 = tx.query_row("SELECT COALESCE(MAX(rowid), 0) FROM messages", [], |r| {
            r.get(0)
        })?;
        if after >= last {
            tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('addresses_indexed', '1')",
                [],
            )?;
            tx.commit()?;
            return Ok(None);
        }
        let upto = after + span;
        tx.execute(
            "INSERT OR IGNORE INTO known_addresses (email)
             SELECT lower(from_email) FROM messages WHERE rowid > ?1 AND rowid <= ?2 AND from_email != ''
             UNION SELECT lower(json_extract(j.value, '$.email')) FROM messages m, json_each(m.to_json) j
               WHERE m.rowid > ?1 AND m.rowid <= ?2 AND json_extract(j.value, '$.email') != ''
             UNION SELECT lower(json_extract(j.value, '$.email')) FROM messages m, json_each(m.cc_json) j
               WHERE m.rowid > ?1 AND m.rowid <= ?2 AND json_extract(j.value, '$.email') != ''",
            params![after, upto],
        )?;
        tx.commit()?;
        Ok(Some(upto))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::ParsedMessage;
    use crate::store::{IncomingMessage, SyncMode};
    use rstest::*;

    struct Env {
        _dir: tempfile::TempDir,
        store: Store,
    }

    fn addr(email: &str) -> Address {
        Address {
            name: String::new(),
            email: email.into(),
        }
    }

    #[fixture]
    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        let acc = store.add_account("me@example.com", "gmail").unwrap().id;
        let msg = IncomingMessage {
            provider_id: "m1".into(),
            provider_thread_id: "t1".into(),
            label_ids: vec!["INBOX".into()],
            internal_date: 1_700_000_000_000,
            parsed: ParsedMessage {
                from: Some(addr("Ada@Example.com")),
                to: vec![addr("me@example.com")],
                cc: vec![addr("grace@example.com")],
                subject: "Hello".into(),
                body_text: "Hi".into(),
                ..Default::default()
            },
        };
        store
            .ingest(&acc, "me@example.com", &msg, SyncMode::Initial)
            .unwrap();
        Env { _dir: dir, store }
    }

    /// Back to a mailbox stored before the index: no flag, no rows.
    fn unindexed(env: &Env) {
        env.store
            .w
            .lock()
            .unwrap()
            .execute_batch(
                "DELETE FROM known_addresses; DELETE FROM meta WHERE key = 'addresses_indexed';",
            )
            .unwrap();
    }

    fn backfill(env: &Env) {
        let mut after = 0;
        while let Some(next) = env.store.index_addresses_batch(after, 1).unwrap() {
            after = next;
        }
    }

    #[rstest]
    #[case::sender("ada@example.com", true)]
    #[case::sender_any_case("ADA@example.COM", true)]
    #[case::cc("grace@example.com", true)]
    #[case::stranger("nobody@example.com", false)]
    fn new_mail_is_known(env: Env, #[case] email: &str, #[case] want: bool) {
        assert_eq!(env.store.known_address(email).unwrap(), want);
    }

    #[rstest]
    #[case::cc("grace@example.com", true)]
    #[case::stranger("nobody@example.com", false)]
    fn unindexed_mail_asks_the_messages(env: Env, #[case] email: &str, #[case] want: bool) {
        unindexed(&env);
        assert_eq!(env.store.known_address(email).unwrap(), want);
    }

    #[rstest]
    fn backfill_indexes_older_mail(env: Env) {
        unindexed(&env);
        backfill(&env);
        let n: i64 = env
            .store
            .r
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM known_addresses", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
    }

    #[rstest]
    fn backfill_ends_on_the_index(env: Env) {
        unindexed(&env);
        backfill(&env);
        assert!(indexed(&env.store.r.lock().unwrap()).unwrap());
    }
}
