//! Hidden accounts: an account the user hides (for a screenshot or a demo) keeps
//! syncing, but its mail is left out of every list, count, search, feed and suggestion
//! the app shows. One flag on the account; read queries add [`Store::visible`] to their
//! conditions, which is empty while nothing is hidden so they run unchanged.

use super::Store;
use crate::error::{Error, Result};
use rusqlite::params;
use std::sync::atomic::Ordering;

pub(super) const MIGRATION: &str =
    "ALTER TABLE accounts ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;";

/// Condition on a threads alias `t`: its account is not hidden.
const VISIBLE_T: &str = "t.account_id NOT IN (SELECT id FROM accounts WHERE hidden = 1)";

/// Condition on a threads alias `t`: its account is not the local demo account, whose
/// mail never goes to an AI (its cards, chips and summaries come with the demo).
pub(super) const NOT_DEMO_T: &str =
    "t.account_id NOT IN (SELECT id FROM accounts WHERE provider = 'demo')";

impl Store {
    /// `" AND <threads t of visible accounts>"` while an account is hidden, else `""`.
    pub(super) fn visible(&self) -> String {
        self.visible_on("t")
    }

    /// The same for a messages alias `m`.
    pub(super) fn visible_m(&self) -> String {
        self.visible_on("m")
    }

    /// The same for any alias with an `account_id` column.
    pub(super) fn visible_on(&self, alias: &str) -> String {
        if self.any_hidden.load(Ordering::Relaxed) {
            format!(" AND {alias}.account_id NOT IN (SELECT id FROM accounts WHERE hidden = 1)")
        } else {
            String::new()
        }
    }

    /// Bare condition for a `WHERE` list built clause by clause, when an account is hidden.
    pub(super) fn visible_clause(&self) -> Option<String> {
        self.any_hidden
            .load(Ordering::Relaxed)
            .then(|| VISIBLE_T.to_string())
    }

    /// Reads whether any account is hidden into the cached flag.
    pub(super) fn refresh_hidden(&self) -> Result<()> {
        let w = self.w.lock().unwrap();
        let any: bool = w.query_row(
            "SELECT EXISTS (SELECT 1 FROM accounts WHERE hidden = 1)",
            [],
            |r| r.get(0),
        )?;
        self.any_hidden.store(any, Ordering::Relaxed);
        Ok(())
    }

    /// Hides an account's mail from everything shown, or shows it again. Its sidebar
    /// counts are recomputed on the next ask.
    pub fn set_account_hidden(&self, id: &str, hidden: bool) -> Result<()> {
        let acc = self.account(id)?;
        {
            let w = self.w.lock().unwrap();
            let n = w.execute(
                "UPDATE accounts SET hidden = ?2 WHERE id = ?1",
                params![acc.id, hidden as i64],
            )?;
            if n == 0 {
                return Err(Error::NotFound(format!("account {id}")));
            }
            w.execute("UPDATE counts_gen SET n = n + 1", [])?;
        }
        self.refresh_hidden()
    }

    /// Ids of the hidden accounts.
    pub fn hidden_accounts(&self) -> Result<Vec<String>> {
        Ok(self
            .accounts()?
            .into_iter()
            .filter(|a| a.hidden)
            .map(|a| a.id)
            .collect())
    }
}

#[cfg(test)]
mod tests;
