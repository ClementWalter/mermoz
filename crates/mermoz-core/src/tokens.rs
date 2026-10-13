//! Refresh-token storage: macOS Keychain by default; `MERMOZ_TOKEN_STORE=file`
//! keeps them in `MERMOZ_HOME/tokens` (0600) for tests and headless use.

use crate::error::{Error, Result};
use crate::oauth::ClientConfig;
use crate::paths;
use serde::{Deserialize, Serialize};

/// A Google sign-in: refresh tokens only work with the OAuth client that issued them,
/// so the client (and its token endpoint) travels with the token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GoogleGrant {
    pub refresh_token: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    #[serde(default)]
    pub token_url: Option<String>,
}

impl GoogleGrant {
    /// Older entries are a bare refresh token with no client recorded.
    pub fn parse(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_else(|_| GoogleGrant {
            refresh_token: raw.trim().to_string(),
            client_id: None,
            client_secret: None,
            token_url: None,
        })
    }

    pub fn client(&self) -> Option<ClientConfig> {
        let mut c = ClientConfig::with_ids(self.client_id.clone()?, self.client_secret.clone()?);
        if let Some(t) = &self.token_url {
            c.token_url = t.clone();
        }
        Some(c)
    }

    /// Issued by another OAuth client than the app's: it still refreshes with its own
    /// client, but Google shows that client's project on every new consent, so the
    /// user signs in again to move the account onto the app's client.
    pub fn needs_reconnect(&self, app_client_id: &str) -> bool {
        self.client_id
            .as_deref()
            .is_some_and(|id| id != app_client_id)
    }
}

pub fn save_grant(account: &str, refresh_token: &str, cfg: &ClientConfig) -> Result<()> {
    let g = GoogleGrant {
        refresh_token: refresh_token.into(),
        client_id: Some(cfg.client_id.clone()),
        client_secret: Some(cfg.client_secret.clone()),
        token_url: Some(cfg.token_url.clone()),
    };
    save(account, &serde_json::to_string(&g)?)
}

pub fn load_grant(account: &str) -> Result<GoogleGrant> {
    Ok(GoogleGrant::parse(&load(account)?))
}

/// A Google Drive authorisation, kept apart from the Gmail one so connecting,
/// declining or disconnecting Drive never touches mail access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DriveGrant {
    pub refresh_token: String,
    pub client_id: String,
    pub client_secret: String,
    pub token_url: String,
    /// Scopes Google actually granted (the user may untick some).
    pub scopes: Vec<String>,
}

impl DriveGrant {
    pub fn client(&self) -> ClientConfig {
        let mut c = ClientConfig::with_ids(self.client_id.clone(), self.client_secret.clone());
        c.token_url = self.token_url.clone();
        c
    }

    pub fn has(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

pub fn save_drive_grant(account: &str, g: &DriveGrant) -> Result<()> {
    secret_set(DRIVE_SERVICE, account, &serde_json::to_string(g)?)
}

pub fn load_drive_grant(account: &str) -> Result<DriveGrant> {
    Ok(serde_json::from_str(&secret_get(DRIVE_SERVICE, account)?)?)
}

pub fn delete_drive_grant(account: &str) -> Result<()> {
    secret_delete(DRIVE_SERVICE, account)
}

const SERVICE: &str = "ai.mermoz.oauth";
const DRIVE_SERVICE: &str = "ai.mermoz.drive";
const PROVIDER_SERVICE: &str = "ai.mermoz.provider";

/// Where secrets live. Signed release builds (compiled with MERMOZ_USE_KEYCHAIN) use the
/// Keychain. Ad-hoc signed development builds change identity on every rebuild, so the
/// Keychain would ask for the password again and again: they use owner-only files.
/// `MERMOZ_TOKEN_STORE=file|keychain` overrides at runtime.
fn use_file() -> bool {
    match std::env::var("MERMOZ_TOKEN_STORE").as_deref() {
        Ok("file") => true,
        Ok("keychain") => false,
        _ => option_env!("MERMOZ_USE_KEYCHAIN").is_none(),
    }
}

fn secret_path(service: &str, account: &str) -> std::path::PathBuf {
    let safe: String = account
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || "@._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    paths::tokens_dir().join(service).join(safe)
}

/// Stores a secret (atomic 0600 file, or the Keychain).
pub fn secret_set(service: &str, account: &str, value: &str) -> Result<()> {
    if use_file() {
        use std::os::unix::fs::PermissionsExt;
        let p = secret_path(service, account);
        let dir = p.parent().expect("secret dir");
        std::fs::create_dir_all(dir)?;
        // Owner-only all the way down: the tokens folder and each service folder.
        std::fs::set_permissions(paths::tokens_dir(), std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, value)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(tmp, p)?;
        return Ok(());
    }
    security_framework::passwords::set_generic_password(service, account, value.as_bytes())
        .map_err(|e| Error::other(format!("keychain write failed: {e}")))
}

/// Reads a secret. A file-store miss falls back once to the Keychain (secrets saved by an
/// earlier build) and moves the value into the file store.
pub fn secret_get(service: &str, account: &str) -> Result<String> {
    if use_file() {
        if let Ok(v) = std::fs::read_to_string(secret_path(service, account)) {
            return Ok(v.trim().to_string());
        }
        if std::env::var("MERMOZ_TOKEN_STORE").as_deref() != Ok("file")
            && let Ok(b) = security_framework::passwords::get_generic_password(service, account)
        {
            let v = String::from_utf8(b).map_err(|_| Error::other("secret is not utf-8"))?;
            // The old Keychain item is left alone: deleting it could prompt once more.
            secret_set(service, account, &v)?;
            return Ok(v);
        }
        return Err(Error::NotFound(format!("{service} secret for {account}")));
    }
    let b = security_framework::passwords::get_generic_password(service, account)
        .map_err(|_| Error::NotFound(format!("{service} secret for {account}")))?;
    String::from_utf8(b).map_err(|_| Error::other("secret is not utf-8"))
}

pub fn secret_delete(service: &str, account: &str) -> Result<()> {
    let _ = std::fs::remove_file(secret_path(service, account));
    let _ = security_framework::passwords::delete_generic_password(service, account);
    Ok(())
}

pub fn save(account: &str, refresh_token: &str) -> Result<()> {
    secret_set(SERVICE, account, refresh_token)
}

pub fn load(account: &str) -> Result<String> {
    secret_get(SERVICE, account)
}

pub fn delete(account: &str) -> Result<()> {
    secret_delete(SERVICE, account)
}

/// Provider API keys: same storage, separate service.
pub fn save_provider_key(provider_id: &str, key: &str) -> Result<()> {
    secret_set(PROVIDER_SERVICE, provider_id, key)
}

pub fn load_provider_key(provider_id: &str) -> Result<String> {
    secret_get(PROVIDER_SERVICE, provider_id)
}

pub fn delete_provider_key(provider_id: &str) -> Result<()> {
    secret_delete(PROVIDER_SERVICE, provider_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn bare_token_has_no_client() {
        assert_eq!(GoogleGrant::parse("1//abc").client_id, None);
    }

    #[rstest]
    fn bare_token_is_kept() {
        assert_eq!(GoogleGrant::parse(" 1//abc\n").refresh_token, "1//abc");
    }

    #[rstest]
    fn json_grant_restores_client() {
        let g =
            GoogleGrant::parse(r#"{"refresh_token":"r","client_id":"cid","client_secret":"s"}"#);
        assert_eq!(g.client().unwrap().client_id, "cid");
    }

    fn grant(client_id: Option<&str>) -> GoogleGrant {
        GoogleGrant {
            refresh_token: "r".into(),
            client_id: client_id.map(String::from),
            client_secret: Some("s".into()),
            token_url: None,
        }
    }

    #[rstest]
    #[case::other_client(Some("other-cid"), true)]
    #[case::app_client(Some("app-cid"), false)]
    #[case::no_client_recorded(None, false)]
    fn needs_reconnect_only_for_another_client(
        #[case] client_id: Option<&str>,
        #[case] expected: bool,
    ) {
        assert_eq!(grant(client_id).needs_reconnect("app-cid"), expected);
    }
}
