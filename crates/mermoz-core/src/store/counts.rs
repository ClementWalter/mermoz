//! Sidebar counts: open threads (or conversations) per status, project, account,
//! priority and kind.
//!
//! Triggers bump `counts_gen` on every write that can move a count (a thread's status,
//! lead, account, priority or kind; its projects; the statuses' open flags). The last
//! answer is kept per generation, so the sidebar asking again after an unrelated write
//! (a read flag, a new snippet) costs one row read. Recomputing is one covering-index
//! scan of open threads plus the project join.

use super::Store;
use crate::error::Result;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub(super) const MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS counts_gen (n INTEGER NOT NULL);
INSERT INTO counts_gen (n) SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM counts_gen);
CREATE TRIGGER IF NOT EXISTS counts_threads_insert AFTER INSERT ON threads BEGIN
  UPDATE counts_gen SET n = n + 1;
END;
CREATE TRIGGER IF NOT EXISTS counts_threads_delete AFTER DELETE ON threads BEGIN
  UPDATE counts_gen SET n = n + 1;
END;
CREATE TRIGGER IF NOT EXISTS counts_threads_update
  AFTER UPDATE OF status, conv_lead, account_id, priority, conv_kind ON threads
  WHEN OLD.status IS NOT NEW.status OR OLD.conv_lead IS NOT NEW.conv_lead
    OR OLD.account_id IS NOT NEW.account_id OR OLD.priority IS NOT NEW.priority
    OR OLD.conv_kind IS NOT NEW.conv_kind
BEGIN
  UPDATE counts_gen SET n = n + 1;
END;
CREATE TRIGGER IF NOT EXISTS counts_projects_insert AFTER INSERT ON thread_projects BEGIN
  UPDATE counts_gen SET n = n + 1;
END;
CREATE TRIGGER IF NOT EXISTS counts_projects_delete AFTER DELETE ON thread_projects BEGIN
  UPDATE counts_gen SET n = n + 1;
END;
CREATE TRIGGER IF NOT EXISTS counts_projects_update AFTER UPDATE ON thread_projects BEGIN
  UPDATE counts_gen SET n = n + 1;
END;
CREATE TRIGGER IF NOT EXISTS counts_statuses_insert AFTER INSERT ON statuses BEGIN
  UPDATE counts_gen SET n = n + 1;
END;
CREATE TRIGGER IF NOT EXISTS counts_statuses_delete AFTER DELETE ON statuses BEGIN
  UPDATE counts_gen SET n = n + 1;
END;
CREATE TRIGGER IF NOT EXISTS counts_statuses_update AFTER UPDATE OF key, archived ON statuses BEGIN
  UPDATE counts_gen SET n = n + 1;
END;
-- Open leads carry their id in the index, so project counts never read the table.
DROP INDEX IF EXISTS threads_conv_lead;
CREATE INDEX threads_conv_lead ON threads(conv_lead, status, account_id, priority, conv_kind, id);
"#;

/// The last counts answer per mode (threads, conversations) and the generation it saw.
#[derive(Default)]
pub(super) struct CountsCache([Option<(i64, Value)>; 2]);

impl Store {
    /// Open threads per status, project, account and priority; with `conversations`,
    /// open conversations instead (one per lead thread), plus feeds and lists by kind.
    pub fn counts(&self, conversations: bool) -> Result<Value> {
        let r = self.r.lock().unwrap();
        // Read before counting: a write landing in between bumps the generation past
        // this one, so the next call recounts instead of trusting a newer answer.
        let generation: i64 = r.query_row("SELECT n FROM counts_gen", [], |r| r.get(0))?;
        let slot = conversations as usize;
        if let Some((g, v)) = &self.counts_cache.lock().unwrap().0[slot]
            && *g == generation
        {
            return Ok(v.clone());
        }
        let open = if conversations {
            "t.conv_lead = 1 AND t.status IN (SELECT key FROM statuses WHERE archived = 0)"
        } else {
            "t.status IN (SELECT key FROM statuses WHERE archived = 0)"
        };
        // One pass over open threads; the per-dimension totals are summed here.
        let mut status: BTreeMap<String, i64> = BTreeMap::new();
        let mut account: BTreeMap<String, i64> = BTreeMap::new();
        let mut priority: BTreeMap<String, i64> = BTreeMap::new();
        let mut kind: BTreeMap<String, i64> = BTreeMap::new();
        let mut st = r.prepare_cached(&format!(
            "SELECT t.status, t.account_id, t.priority, t.conv_kind, COUNT(*) FROM threads t
             WHERE {open} GROUP BY t.status, t.account_id, t.priority, t.conv_kind"
        ))?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let n: i64 = row.get(4)?;
            *status.entry(row.get(0)?).or_default() += n;
            *account.entry(row.get(1)?).or_default() += n;
            *priority
                .entry(row.get::<_, i64>(2)?.to_string())
                .or_default() += n;
            let k: String = row.get(3)?;
            if conversations && k != "person" {
                *kind.entry(k).or_default() += n;
            }
        }
        drop(rows);
        let mut project = Map::new();
        let mut st = r.prepare_cached(&format!(
            "SELECT tp.name, COUNT(*) FROM thread_projects tp JOIN threads t ON t.id = tp.thread_id WHERE {open} GROUP BY tp.name"
        ))?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (k, n) = row?;
            project.insert(k, n.into());
        }
        let obj = |m: BTreeMap<String, i64>| -> Map<String, Value> {
            m.into_iter().map(|(k, n)| (k, n.into())).collect()
        };
        let v = json!({
            "status": obj(status), "project": project, "account": obj(account),
            "priority": obj(priority), "kind": obj(kind),
        });
        self.counts_cache.lock().unwrap().0[slot] = Some((generation, v.clone()));
        Ok(v)
    }
}

#[cfg(test)]
mod tests;
