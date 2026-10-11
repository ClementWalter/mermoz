//! Refresh-token storage: macOS Keychain by default; `MERMOZ_TOKEN_STORE=file`
//! keeps them in `MERMOZ_HOME/tokens` (0600) for tests and headless use.

use crate::error::{Error, Result};
use crate::paths;

const SERVICE: &str = "ai.mermoz.oauth";

fn use_file() -> bool {
    std::env::var("MERMOZ_TOKEN_STORE")
        .map(|v| v == "file")
        .unwrap_or(false)
}

fn file_path(account: &str) -> std::path::PathBuf {
    paths::tokens_dir().join(format!("{account}.token"))
}

pub fn save(account: &str, refresh_token: &str) -> Result<()> {
    if use_file() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(paths::tokens_dir())?;
        let p = file_path(account);
        std::fs::write(&p, refresh_token)?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
        return Ok(());
    }
    security_framework::passwords::set_generic_password(SERVICE, account, refresh_token.as_bytes())
        .map_err(|e| Error::other(format!("keychain write failed: {e}")))
}

pub fn load(account: &str) -> Result<String> {
    if use_file() {
        return Ok(std::fs::read_to_string(file_path(account))
            .map_err(|_| Error::NotFound(format!("token for {account}")))?
            .trim()
            .to_string());
    }
    let bytes = security_framework::passwords::get_generic_password(SERVICE, account)
        .map_err(|e| Error::NotFound(format!("keychain token for {account}: {e}")))?;
    String::from_utf8(bytes).map_err(|_| Error::other("token is not utf-8"))
}

pub fn delete(account: &str) -> Result<()> {
    if use_file() {
        let _ = std::fs::remove_file(file_path(account));
        return Ok(());
    }
    let _ = security_framework::passwords::delete_generic_password(SERVICE, account);
    Ok(())
}

const PROVIDER_SERVICE: &str = "ai.mermoz.provider";

/// Provider API keys: same storage rules as refresh tokens, separate keychain service.
pub fn save_provider_key(provider_id: &str, key: &str) -> Result<()> {
    if use_file() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(paths::tokens_dir())?;
        let p = paths::tokens_dir().join(format!("{provider_id}.apikey"));
        std::fs::write(&p, key)?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
        return Ok(());
    }
    security_framework::passwords::set_generic_password(
        PROVIDER_SERVICE,
        provider_id,
        key.as_bytes(),
    )
    .map_err(|e| Error::other(format!("keychain write failed: {e}")))
}

pub fn load_provider_key(provider_id: &str) -> Result<String> {
    if use_file() {
        return Ok(std::fs::read_to_string(
            paths::tokens_dir().join(format!("{provider_id}.apikey")),
        )
        .map_err(|_| Error::NotFound(format!("API key for {provider_id}")))?
        .trim()
        .to_string());
    }
    let bytes = security_framework::passwords::get_generic_password(PROVIDER_SERVICE, provider_id)
        .map_err(|_| Error::NotFound(format!("API key for {provider_id}")))?;
    String::from_utf8(bytes).map_err(|_| Error::other("API key is not utf-8"))
}

pub fn delete_provider_key(provider_id: &str) -> Result<()> {
    if use_file() {
        let _ = std::fs::remove_file(paths::tokens_dir().join(format!("{provider_id}.apikey")));
        return Ok(());
    }
    let _ = security_framework::passwords::delete_generic_password(PROVIDER_SERVICE, provider_id);
    Ok(())
}
