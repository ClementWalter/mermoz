//! Filesystem layout under `MERMOZ_HOME` (default `~/Library/Application Support/Mermoz`).

use std::path::PathBuf;

/// Root of all Mermoz state; overridable so tests and multiple profiles never collide.
pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("MERMOZ_HOME") {
        return PathBuf::from(h);
    }
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Library/Application Support/Mermoz")
}

pub fn socket_path() -> PathBuf {
    home().join("mermozd.sock")
}

pub fn db_path() -> PathBuf {
    home().join("mermoz.sqlite")
}

/// Attachment blobs live outside SQLite so the DB stays small and agents get real file paths.
pub fn blobs_dir() -> PathBuf {
    home().join("blobs")
}

pub fn oauth_client_path() -> PathBuf {
    home().join("oauth_client.json")
}

pub fn tokens_dir() -> PathBuf {
    home().join("tokens")
}
