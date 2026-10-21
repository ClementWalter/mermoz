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

/// `MERMOZ_SOCKET` overrides it, since macOS caps socket paths at 104 bytes.
pub fn socket_path() -> PathBuf {
    std::env::var_os("MERMOZ_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("mermozd.sock"))
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

/// The Microsoft sign-in client for builds without one compiled in (development, tests).
pub fn microsoft_client_path() -> PathBuf {
    home().join("microsoft_client.json")
}

pub fn tokens_dir() -> PathBuf {
    home().join("tokens")
}

/// Drive metadata, document text and thumbnails: a cache, safe to delete.
pub fn drive_cache_path() -> PathBuf {
    home().join("drive.sqlite")
}
