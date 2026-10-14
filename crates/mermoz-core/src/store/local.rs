//! Store writes for mail that exists only on this Mac (the demo account): a thread's
//! summary written ahead of time, and starting an account's mail over.

use super::Store;
use crate::error::Result;
use rusqlite::params;

impl Store {
    /// Sets the summary and next step the reading pane shows above a conversation.
    pub fn set_thread_gist(&self, thread_id: &str, summary: &str, next: &str) -> Result<()> {
        let w = self.w.lock().unwrap();
        w.execute(
            "UPDATE threads SET summary = ?2, next_action = ?3 WHERE id = ?1",
            params![thread_id, summary, next],
        )?;
        Ok(())
    }

    /// Removes an account's mail, folders, drafts and files but keeps the account itself
    /// (its id, place in the list and whether it is hidden).
    pub fn clear_account_mail(&self, account_id: &str) -> Result<()> {
        let acc = self.account(account_id)?;
        let files: Vec<String> = {
            let r = self.r.lock().unwrap();
            let mut st = r.prepare(
                "SELECT a.path FROM attachments a JOIN messages m ON m.id = a.message_id WHERE m.account_id = ?1",
            )?;
            st.query_map([&acc.id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let (created, hidden): (i64, i64) = {
            let r = self.r.lock().unwrap();
            r.query_row(
                "SELECT created, hidden FROM accounts WHERE id = ?1",
                [&acc.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?
        };
        self.remove_account(&acc.id)?;
        {
            let w = self.w.lock().unwrap();
            w.execute(
                "INSERT INTO accounts (id, email, provider, created, hidden) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![acc.id, acc.email, acc.provider, created, hidden],
            )?;
        }
        self.refresh_hidden()?;
        // Each attachment sits alone in its own folder under the blob store.
        for f in files {
            if let Some(dir) = std::path::Path::new(&f).parent()
                && dir.starts_with(&self.blobs)
            {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        Ok(())
    }
}
