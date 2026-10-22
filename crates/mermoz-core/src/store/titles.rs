//! Card titles: the user's own words for a thread, shown on its card instead of the
//! subject. Local only: the mail and its subject stay as they are, and sync never
//! touches the title.

use super::Store;
use crate::error::{Error, Result};
use rusqlite::{OptionalExtension, params};

pub(super) const MIGRATION: &str = "ALTER TABLE threads ADD COLUMN title TEXT;";

/// Longest title kept, in characters: a card shows two lines.
pub const MAX_TITLE_CHARS: usize = 200;

impl Store {
    /// Sets a thread's card title; `None` or blank text gives the subject back. Returns
    /// the title it had, so the change can be undone.
    pub fn set_thread_title(&self, id: &str, title: Option<&str>) -> Result<Option<String>> {
        // One line, trimmed: a pasted newline would break the card's two-line layout.
        let title = title
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|t| !t.is_empty());
        if let Some(t) = &title
            && t.chars().count() > MAX_TITLE_CHARS
        {
            return Err(Error::InvalidParams(format!(
                "title is longer than {MAX_TITLE_CHARS} characters"
            )));
        }
        let w = self.w.lock().unwrap();
        let before: Option<String> = w
            .query_row("SELECT title FROM threads WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("thread {id}")))?;
        w.execute(
            "UPDATE threads SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        Ok(before)
    }
}

#[cfg(test)]
mod tests;
