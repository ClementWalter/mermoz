//! The write-back queue's order and batching: ops a bulk run queued wait behind the
//! user's own edits, and label changes go to Gmail many messages per call.

use super::{Op, Store};
use crate::error::Result;
use rusqlite::params;
use std::collections::{BTreeMap, BTreeSet};

/// `bulk` marks ops queued by an "Ask Mermoz to do" run; any later edit of the same
/// thread clears it (see `enqueue_tx`).
pub(super) const MIGRATION: &str = "ALTER TABLE ops ADD COLUMN bulk INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS ops_queue ON ops(account_id, bulk, id);";

impl Store {
    /// The account's next ops: the user's own edits first, then bulk ones, oldest first.
    pub fn pending_ops_for(&self, account_id: &str, limit: i64) -> Result<Vec<Op>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT id, account_id, kind, key, payload, attempts, seq FROM ops
             WHERE account_id = ?1 ORDER BY bulk, id LIMIT ?2",
        )?;
        Ok(st
            .query_map(params![account_id, limit], |r| {
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

    /// Queues these threads' pending write-backs behind the user's own edits.
    pub fn mark_ops_bulk(&self, thread_ids: &[String]) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "UPDATE ops SET bulk = 1 WHERE key = ?1 AND kind IN ('labels', 'read', 'projects')",
            )?;
            for id in thread_ids {
                st.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Gmail ids of the messages stored for each thread.
    pub fn provider_message_ids(
        &self,
        thread_ids: &[String],
    ) -> Result<BTreeMap<String, Vec<String>>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached("SELECT provider_id FROM messages WHERE thread_id = ?1")?;
        let mut out = BTreeMap::new();
        for t in thread_ids {
            let ids = st
                .query_map([t], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            out.insert(t.clone(), ids);
        }
        Ok(out)
    }

    /// Labels stored on a message, by Gmail label id; `None` when it is not stored.
    pub fn message_label_ids(
        &self,
        account_id: &str,
        provider_id: &str,
    ) -> Result<Option<BTreeSet<String>>> {
        let r = self.r.lock().unwrap();
        let lj: Option<String> = r
            .prepare_cached(
                "SELECT labels_json FROM messages WHERE account_id = ?1 AND provider_id = ?2",
            )?
            .query_row(params![account_id, provider_id], |r| r.get(0))
            .ok();
        Ok(lj.map(|j| {
            serde_json::from_str::<Vec<String>>(&j)
                .unwrap_or_default()
                .into_iter()
                .collect()
        }))
    }

    /// [`Store::apply_label_delta`] for many threads in one write.
    pub fn apply_label_delta_many(
        &self,
        thread_ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        for thread_id in thread_ids {
            let rows: Vec<(String, String)> = {
                let mut st =
                    tx.prepare_cached("SELECT id, labels_json FROM messages WHERE thread_id = ?1")?;
                st.query_map([thread_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for (mid, lj) in rows {
                let mut v: BTreeSet<String> = serde_json::from_str::<Vec<String>>(&lj)
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                v.extend(add.iter().cloned());
                for r in remove {
                    v.remove(r);
                }
                tx.prepare_cached("UPDATE messages SET labels_json = ?2 WHERE id = ?1")?
                    .execute(params![mid, serde_json::to_string(&v)?])?;
            }
            Self::index_thread_labels(&tx, thread_id)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes these ops, each only if it was not superseded while in flight.
    pub fn ops_done(&self, ops: &[&Op]) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        {
            let mut st = tx.prepare_cached("DELETE FROM ops WHERE id = ?1 AND seq = ?2")?;
            for op in ops {
                st.execute(params![op.id, op.seq])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
