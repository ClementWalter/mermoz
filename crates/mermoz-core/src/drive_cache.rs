//! Local cache of Google Drive data, in its own SQLite file (`drive.sqlite`): file
//! metadata, document text and thumbnails (capped in total size, least recently
//! used evicted first), the folders Mermoz created and the attachments it saved.
//!
//! It is a cache: deleting the file loses nothing that Drive cannot give back,
//! except the record of which attachments were saved where.

use crate::drive::FileMeta;
use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS files (
  account TEXT NOT NULL, id TEXT NOT NULL, meta_json TEXT, access TEXT NOT NULL,
  fetched INTEGER NOT NULL, PRIMARY KEY (account, id));
CREATE TABLE IF NOT EXISTS content (
  account TEXT NOT NULL, id TEXT NOT NULL, kind TEXT NOT NULL, modified INTEGER NOT NULL,
  mime TEXT NOT NULL, data BLOB NOT NULL, bytes INTEGER NOT NULL, accessed INTEGER NOT NULL,
  PRIMARY KEY (account, id, kind));
CREATE INDEX IF NOT EXISTS content_accessed ON content(accessed);
CREATE TABLE IF NOT EXISTS folders (
  account TEXT NOT NULL, path TEXT NOT NULL, folder_id TEXT NOT NULL, PRIMARY KEY (account, path));
CREATE TABLE IF NOT EXISTS saved (
  account TEXT NOT NULL, attachment_id TEXT NOT NULL, file_id TEXT NOT NULL, name TEXT NOT NULL,
  web_link TEXT, folder TEXT NOT NULL, created INTEGER NOT NULL, PRIMARY KEY (account, attachment_id));
"#;

/// Text and thumbnails kept on disk, all accounts together.
pub const DEFAULT_CAP_BYTES: i64 = 50 * 1024 * 1024;

/// What Mermoz knows about a file it tried to open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedFile {
    /// "ok", or "no_access" when Drive refused or does not know the file.
    pub access: String,
    pub meta: Option<FileMeta>,
    pub fetched: i64,
}

/// An attachment Mermoz put in Drive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub attachment_id: String,
    pub file_id: String,
    pub name: String,
    pub web_link: Option<String>,
    /// e.g. "Mermoz/Clients/Acme".
    pub folder: String,
    pub created: i64,
}

pub struct DriveCache {
    conn: Mutex<Connection>,
    cap: AtomicI64,
    /// Recency counter: strictly increasing, so ties never make eviction ambiguous.
    clock: AtomicI64,
}

impl DriveCache {
    pub fn open(path: &Path, cap_bytes: i64) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let clock: i64 =
            conn.query_row("SELECT COALESCE(MAX(accessed), 0) FROM content", [], |r| {
                r.get(0)
            })?;
        Ok(Self {
            conn: Mutex::new(conn),
            cap: AtomicI64::new(cap_bytes),
            clock: AtomicI64::new(clock),
        })
    }

    fn tick(&self) -> i64 {
        self.clock.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn cap(&self) -> i64 {
        self.cap.load(Ordering::SeqCst)
    }

    /// Changes the cap and evicts down to it right away.
    pub fn set_cap(&self, bytes: i64) -> Result<()> {
        self.cap.store(bytes.max(0), Ordering::SeqCst);
        let c = self.conn.lock().unwrap();
        evict(&c, self.cap())
    }

    pub fn file(&self, account: &str, id: &str) -> Result<Option<CachedFile>> {
        let c = self.conn.lock().unwrap();
        Ok(c.query_row(
            "SELECT meta_json, access, fetched FROM files WHERE account = ?1 AND id = ?2",
            params![account, id],
            |r| {
                let meta: Option<String> = r.get(0)?;
                Ok(CachedFile {
                    meta: meta.and_then(|m| serde_json::from_str(&m).ok()),
                    access: r.get(1)?,
                    fetched: r.get(2)?,
                })
            },
        )
        .optional()?)
    }

    pub fn put_file(&self, account: &str, id: &str, f: &CachedFile) -> Result<()> {
        let meta = f.meta.as_ref().map(serde_json::to_string).transpose()?;
        self.conn.lock().unwrap().execute(
            "INSERT INTO files (account, id, meta_json, access, fetched) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account, id) DO UPDATE SET meta_json = excluded.meta_json,
               access = excluded.access, fetched = excluded.fetched",
            params![account, id, meta, f.access, f.fetched],
        )?;
        Ok(())
    }

    /// Cached text or thumbnail of a file, if made from the version modified at `modified`.
    pub fn content(
        &self,
        account: &str,
        id: &str,
        kind: &str,
        modified: i64,
    ) -> Result<Option<(Vec<u8>, String)>> {
        let c = self.conn.lock().unwrap();
        let hit: Option<(Vec<u8>, String)> = c
            .query_row(
                "SELECT data, mime FROM content WHERE account = ?1 AND id = ?2 AND kind = ?3 AND modified = ?4",
                params![account, id, kind, modified],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if hit.is_some() {
            c.execute(
                "UPDATE content SET accessed = ?4 WHERE account = ?1 AND id = ?2 AND kind = ?3",
                params![account, id, kind, self.tick()],
            )?;
        }
        Ok(hit)
    }

    /// Stores text or a thumbnail, then evicts the least recently used entries until
    /// the cache fits its cap. An entry larger than the whole cap is not kept.
    pub fn put_content(
        &self,
        account: &str,
        id: &str,
        kind: &str,
        modified: i64,
        mime: &str,
        data: &[u8],
    ) -> Result<bool> {
        let c = self.conn.lock().unwrap();
        let bytes = data.len() as i64;
        if bytes > self.cap() {
            c.execute(
                "DELETE FROM content WHERE account = ?1 AND id = ?2 AND kind = ?3",
                params![account, id, kind],
            )?;
            return Ok(false);
        }
        c.execute(
            "INSERT INTO content (account, id, kind, modified, mime, data, bytes, accessed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(account, id, kind) DO UPDATE SET modified = excluded.modified, mime = excluded.mime,
               data = excluded.data, bytes = excluded.bytes, accessed = excluded.accessed",
            params![account, id, kind, modified, mime, data, bytes, self.tick()],
        )?;
        evict(&c, self.cap())?;
        Ok(true)
    }

    /// Bytes of text and thumbnails on disk.
    pub fn used_bytes(&self) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COALESCE(SUM(bytes), 0) FROM content",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn folder(&self, account: &str, path: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT folder_id FROM folders WHERE account = ?1 AND path = ?2",
                params![account, path],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn put_folder(&self, account: &str, path: &str, folder_id: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO folders (account, path, folder_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(account, path) DO UPDATE SET folder_id = excluded.folder_id",
            params![account, path, folder_id],
        )?;
        Ok(())
    }

    /// Forgets a folder id Drive no longer knows (deleted or trashed by the user).
    pub fn forget_folders(&self, account: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM folders WHERE account = ?1", [account])?;
        Ok(())
    }

    pub fn saved(&self, account: &str, attachment_id: &str) -> Result<Option<Saved>> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT attachment_id, file_id, name, web_link, folder, created FROM saved
                 WHERE account = ?1 AND attachment_id = ?2",
                params![account, attachment_id],
                saved_row,
            )
            .optional()?)
    }

    pub fn put_saved(&self, account: &str, s: &Saved) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO saved (account, attachment_id, file_id, name, web_link, folder, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(account, attachment_id) DO UPDATE SET file_id = excluded.file_id, name = excluded.name,
               web_link = excluded.web_link, folder = excluded.folder, created = excluded.created",
            params![account, s.attachment_id, s.file_id, s.name, s.web_link, s.folder, s.created],
        )?;
        Ok(())
    }

    /// Everything about one account, when its Drive is disconnected.
    pub fn forget_account(&self, account: &str) -> Result<()> {
        let c = self.conn.lock().unwrap();
        for t in ["files", "content", "folders"] {
            c.execute(&format!("DELETE FROM {t} WHERE account = ?1"), [account])?;
        }
        Ok(())
    }
}

fn saved_row(r: &rusqlite::Row) -> rusqlite::Result<Saved> {
    Ok(Saved {
        attachment_id: r.get(0)?,
        file_id: r.get(1)?,
        name: r.get(2)?,
        web_link: r.get(3)?,
        folder: r.get(4)?,
        created: r.get(5)?,
    })
}

fn evict(c: &Connection, cap: i64) -> Result<()> {
    let mut used: i64 = c.query_row("SELECT COALESCE(SUM(bytes), 0) FROM content", [], |r| {
        r.get(0)
    })?;
    if used <= cap {
        return Ok(());
    }
    let victims: Vec<(String, String, String, i64)> = {
        let mut st = c.prepare("SELECT account, id, kind, bytes FROM content ORDER BY accessed")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (account, id, kind, bytes) in victims {
        if used <= cap {
            break;
        }
        c.execute(
            "DELETE FROM content WHERE account = ?1 AND id = ?2 AND kind = ?3",
            params![account, id, kind],
        )?;
        used -= bytes;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::{fixture, rstest};

    struct Cache {
        _dir: tempfile::TempDir,
        c: DriveCache,
    }

    #[fixture]
    fn cache() -> Cache {
        let dir = tempfile::tempdir().unwrap();
        let c = DriveCache::open(&dir.path().join("drive.sqlite"), 100).unwrap();
        Cache { _dir: dir, c }
    }

    fn put(c: &DriveCache, id: &str, bytes: usize) -> bool {
        c.put_content("acc", id, "text", 1, "text/plain", &vec![b'x'; bytes])
            .unwrap()
    }

    #[rstest]
    fn cached_text_is_returned(cache: Cache) {
        put(&cache.c, "a", 10);
        assert_eq!(
            cache
                .c
                .content("acc", "a", "text", 1)
                .unwrap()
                .unwrap()
                .0
                .len(),
            10
        );
    }

    #[rstest]
    fn a_newer_version_misses(cache: Cache) {
        put(&cache.c, "a", 10);
        assert_eq!(cache.c.content("acc", "a", "text", 2).unwrap(), None);
    }

    #[rstest]
    fn the_cache_stays_under_its_cap(cache: Cache) {
        for id in ["a", "b", "c", "d"] {
            put(&cache.c, id, 40);
        }
        assert_eq!(cache.c.used_bytes().unwrap(), 80);
    }

    #[rstest]
    fn the_oldest_entry_is_evicted_first(cache: Cache) {
        for id in ["a", "b", "c"] {
            put(&cache.c, id, 40);
        }
        assert_eq!(cache.c.content("acc", "a", "text", 1).unwrap(), None);
    }

    #[rstest]
    fn reading_an_entry_keeps_it(cache: Cache) {
        put(&cache.c, "a", 40);
        put(&cache.c, "b", 40);
        cache.c.content("acc", "a", "text", 1).unwrap();
        put(&cache.c, "c", 40);
        assert!(cache.c.content("acc", "a", "text", 1).unwrap().is_some());
    }

    #[rstest]
    fn an_entry_larger_than_the_cap_is_not_kept(cache: Cache) {
        assert!(!put(&cache.c, "huge", 101));
    }

    #[rstest]
    fn lowering_the_cap_evicts_at_once(cache: Cache) {
        put(&cache.c, "a", 40);
        put(&cache.c, "b", 40);
        cache.c.set_cap(50).unwrap();
        assert_eq!(cache.c.used_bytes().unwrap(), 40);
    }

    #[rstest]
    fn replacing_an_entry_does_not_count_it_twice(cache: Cache) {
        put(&cache.c, "a", 40);
        put(&cache.c, "a", 40);
        assert_eq!(cache.c.used_bytes().unwrap(), 40);
    }

    #[rstest]
    fn saved_attachments_are_remembered(cache: Cache) {
        let s = Saved {
            attachment_id: "att_1".into(),
            file_id: "f1".into(),
            name: "report.pdf".into(),
            web_link: Some("https://drive.example/f1".into()),
            folder: "Mermoz/Clients/Acme".into(),
            created: 1,
        };
        cache.c.put_saved("acc", &s).unwrap();
        assert_eq!(cache.c.saved("acc", "att_1").unwrap(), Some(s));
    }

    #[rstest]
    fn file_access_is_remembered(cache: Cache) {
        let f = CachedFile {
            access: "no_access".into(),
            meta: None,
            fetched: 5,
        };
        cache.c.put_file("acc", "f1", &f).unwrap();
        assert_eq!(cache.c.file("acc", "f1").unwrap(), Some(f));
    }
}
