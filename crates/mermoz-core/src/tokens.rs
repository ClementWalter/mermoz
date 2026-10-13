//! Secret storage (refresh tokens, API keys): the macOS Keychain in Developer ID builds,
//! owner-only files under `MERMOZ_HOME/tokens` in ad-hoc builds, tests and headless use.
//! A secret saved by one kind of build moves to the other's store on first read.

use crate::error::{Error, Result};
use crate::oauth::ClientConfig;
use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A Google sign-in: refresh tokens only work with the OAuth client that issued them,
/// so the client (and its token endpoint) travels with the token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GoogleGrant {
    pub refresh_token: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    #[serde(default)]
    pub token_url: Option<String>,
    /// The user chose this client for the account (Settings › Developer, `--client-json`),
    /// e.g. their company's own Google project; it is kept on every new consent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub custom_client: bool,
}

impl GoogleGrant {
    /// Older entries are a bare refresh token with no client recorded.
    pub fn parse(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_else(|_| GoogleGrant {
            refresh_token: raw.trim().to_string(),
            client_id: None,
            client_secret: None,
            token_url: None,
            custom_client: false,
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
    /// A client the user chose on purpose is never flagged.
    pub fn needs_reconnect(&self, app_client_id: &str) -> bool {
        !self.custom_client
            && self
                .client_id
                .as_deref()
                .is_some_and(|id| id != app_client_id)
    }
}

/// `custom` marks a client the user chose for this account instead of the app's.
pub fn save_grant(
    account: &str,
    refresh_token: &str,
    cfg: &ClientConfig,
    custom: bool,
) -> Result<()> {
    let g = GoogleGrant {
        refresh_token: refresh_token.into(),
        client_id: Some(cfg.client_id.clone()),
        client_secret: Some(cfg.client_secret.clone()),
        token_url: Some(cfg.token_url.clone()),
        custom_client: custom,
    };
    save(account, &serde_json::to_string(&g)?)
}

/// The client a new consent for `account` uses (Gmail again, Drive, the file
/// picker): the user's own client when they chose one, else the app's.
pub fn consent_client(account: &str) -> Result<(ClientConfig, bool)> {
    match load_grant(account) {
        Ok(g) if g.custom_client => match g.client() {
            Some(c) => Ok((c, true)),
            None => Ok((ClientConfig::load()?, false)),
        },
        _ => Ok((ClientConfig::load()?, false)),
    }
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

/// Where secrets live. Developer ID builds (compiled with MERMOZ_USE_KEYCHAIN by
/// `scripts/build_app.py --sign developer-id`) use the Keychain: their signature is
/// stable, so each item's access list keeps trusting the daemon across updates. Ad-hoc
/// signed builds change identity on every rebuild, so the Keychain would ask for the
/// password again and again: they use owner-only files.
/// `MERMOZ_TOKEN_STORE=file|keychain` overrides at runtime.
fn use_file() -> bool {
    match std::env::var("MERMOZ_TOKEN_STORE").as_deref() {
        Ok("file") => true,
        Ok("keychain") => false,
        _ => option_env!("MERMOZ_USE_KEYCHAIN").is_none(),
    }
}

/// The Keychain as the secret store sees it; a trait so moves between the file store and
/// the Keychain are testable without touching a real keychain.
trait Keychain {
    fn get(&self, service: &str, account: &str) -> Option<Vec<u8>>;
    fn set(&self, service: &str, account: &str, value: &[u8]) -> Result<()>;
}

/// The user's login Keychain (generic passwords, file-based keychain): each item trusts
/// the creating binary's designated requirement, i.e. identifier + Team ID once signed.
struct SystemKeychain;

impl Keychain for SystemKeychain {
    fn get(&self, service: &str, account: &str) -> Option<Vec<u8>> {
        security_framework::passwords::get_generic_password(service, account).ok()
    }

    fn set(&self, service: &str, account: &str, value: &[u8]) -> Result<()> {
        security_framework::passwords::set_generic_password(service, account, value)
            .map_err(|e| Error::other(format!("keychain write failed: {e}")))
    }
}

fn secret_path(dir: &Path, service: &str, account: &str) -> PathBuf {
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
    dir.join(service).join(safe)
}

fn utf8(b: Vec<u8>) -> Result<String> {
    String::from_utf8(b).map_err(|_| Error::other("secret is not utf-8"))
}

fn not_found(service: &str, account: &str) -> Error {
    Error::NotFound(format!("{service} secret for {account}"))
}

/// Atomic 0600 write under `dir` (the tokens folder).
fn file_set(dir: &Path, service: &str, account: &str, value: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let p = secret_path(dir, service, account);
    let sub = p.parent().expect("secret dir");
    std::fs::create_dir_all(sub)?;
    // Owner-only all the way down: the tokens folder and each service folder.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    std::fs::set_permissions(sub, std::fs::Permissions::from_mode(0o700))?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, value)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(tmp, p)?;
    Ok(())
}

fn file_get(dir: &Path, service: &str, account: &str) -> Option<String> {
    std::fs::read_to_string(secret_path(dir, service, account))
        .ok()
        .map(|v| v.trim().to_string())
}

/// File-store read. A miss falls back once to `keychain` (secrets saved by a Keychain
/// build) and copies the value into the file store; the Keychain item is left alone,
/// since deleting it could prompt once more.
fn file_mode_get(
    keychain: Option<&dyn Keychain>,
    dir: &Path,
    service: &str,
    account: &str,
) -> Result<String> {
    if let Some(v) = file_get(dir, service, account) {
        return Ok(v);
    }
    let v = utf8(
        keychain
            .and_then(|k| k.get(service, account))
            .ok_or_else(|| not_found(service, account))?,
    )?;
    file_set(dir, service, account, &v)?;
    Ok(v)
}

/// Keychain write. Any file copy goes, so it can never shadow this newer value.
fn keychain_mode_set(
    keychain: &dyn Keychain,
    dir: &Path,
    service: &str,
    account: &str,
    value: &str,
) -> Result<()> {
    keychain.set(service, account, value.as_bytes())?;
    let _ = std::fs::remove_file(secret_path(dir, service, account));
    Ok(())
}

/// Keychain read. A secret in the file store was written by an ad-hoc build (every beta
/// before Developer ID signing): Keychain writes remove the file, so the file holds the
/// newest value. It moves into the Keychain and the file goes once the Keychain holds
/// it; if the Keychain refuses (locked, headless), the file stays and serves the read.
fn keychain_mode_get(
    keychain: &dyn Keychain,
    dir: &Path,
    service: &str,
    account: &str,
) -> Result<String> {
    if let Some(v) = file_get(dir, service, account) {
        match keychain_mode_set(keychain, dir, service, account, &v) {
            Ok(()) => tracing::info!(service, "moved a saved secret into the Keychain"),
            Err(e) => tracing::warn!(service, "secret stays in the file store: {e}"),
        }
        return Ok(v);
    }
    utf8(
        keychain
            .get(service, account)
            .ok_or_else(|| not_found(service, account))?,
    )
}

/// Stores a secret (atomic 0600 file, or the Keychain).
pub fn secret_set(service: &str, account: &str, value: &str) -> Result<()> {
    let dir = paths::tokens_dir();
    if use_file() {
        return file_set(&dir, service, account, value);
    }
    keychain_mode_set(&SystemKeychain, &dir, service, account, value)
}

/// Reads a secret from the store in use, moving it over from the other store when a
/// build using that one saved it.
pub fn secret_get(service: &str, account: &str) -> Result<String> {
    let dir = paths::tokens_dir();
    if use_file() {
        // An explicit file store (tests, headless) never touches the Keychain.
        let explicit = std::env::var("MERMOZ_TOKEN_STORE").as_deref() == Ok("file");
        let keychain: Option<&dyn Keychain> = if explicit {
            None
        } else {
            Some(&SystemKeychain)
        };
        return file_mode_get(keychain, &dir, service, account);
    }
    keychain_mode_get(&SystemKeychain, &dir, service, account)
}

pub fn secret_delete(service: &str, account: &str) -> Result<()> {
    let _ = std::fs::remove_file(secret_path(&paths::tokens_dir(), service, account));
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
    use rstest::{fixture, rstest};

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
            custom_client: false,
        }
    }

    /// In-memory Keychain; `refuse_writes` stands for a locked or unreachable keychain.
    #[derive(Default)]
    struct StubKeychain {
        items: std::cell::RefCell<std::collections::HashMap<(String, String), Vec<u8>>>,
        refuse_writes: bool,
    }

    impl StubKeychain {
        fn holding(value: &str) -> Self {
            let k = StubKeychain::default();
            k.set(SVC, ACCOUNT, value.as_bytes()).unwrap();
            k
        }

        fn value(&self) -> Option<String> {
            self.get(SVC, ACCOUNT)
                .map(|b| String::from_utf8(b).unwrap())
        }
    }

    impl Keychain for StubKeychain {
        fn get(&self, service: &str, account: &str) -> Option<Vec<u8>> {
            self.items
                .borrow()
                .get(&(service.into(), account.into()))
                .cloned()
        }

        fn set(&self, service: &str, account: &str, value: &[u8]) -> Result<()> {
            if self.refuse_writes {
                return Err(Error::other("keychain locked"));
            }
            self.items
                .borrow_mut()
                .insert((service.into(), account.into()), value.to_vec());
            Ok(())
        }
    }

    const SVC: &str = "ai.mermoz.oauth";
    const ACCOUNT: &str = "ada@example.com";

    #[fixture]
    fn tokens() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// A tokens folder as an ad-hoc beta build left it: the secret in a 0600 file.
    #[fixture]
    fn adhoc_tokens(tokens: tempfile::TempDir) -> tempfile::TempDir {
        file_set(tokens.path(), SVC, ACCOUNT, "file-token").unwrap();
        tokens
    }

    fn file_exists(dir: &tempfile::TempDir) -> bool {
        secret_path(dir.path(), SVC, ACCOUNT).exists()
    }

    #[rstest]
    fn signed_build_reads_the_adhoc_file_secret(adhoc_tokens: tempfile::TempDir) {
        let kc = StubKeychain::default();
        assert_eq!(
            keychain_mode_get(&kc, adhoc_tokens.path(), SVC, ACCOUNT).unwrap(),
            "file-token"
        );
    }

    #[rstest]
    fn signed_build_moves_the_file_secret_into_the_keychain(adhoc_tokens: tempfile::TempDir) {
        let kc = StubKeychain::default();
        keychain_mode_get(&kc, adhoc_tokens.path(), SVC, ACCOUNT).unwrap();
        assert_eq!(kc.value().as_deref(), Some("file-token"));
    }

    #[rstest]
    fn moved_file_secret_is_removed(adhoc_tokens: tempfile::TempDir) {
        let kc = StubKeychain::default();
        keychain_mode_get(&kc, adhoc_tokens.path(), SVC, ACCOUNT).unwrap();
        assert!(!file_exists(&adhoc_tokens));
    }

    #[rstest]
    fn moved_secret_reads_back_from_the_keychain(adhoc_tokens: tempfile::TempDir) {
        let kc = StubKeychain::default();
        keychain_mode_get(&kc, adhoc_tokens.path(), SVC, ACCOUNT).unwrap();
        assert_eq!(
            keychain_mode_get(&kc, adhoc_tokens.path(), SVC, ACCOUNT).unwrap(),
            "file-token"
        );
    }

    #[rstest]
    fn file_secret_beats_a_stale_keychain_item(adhoc_tokens: tempfile::TempDir) {
        let kc = StubKeychain::holding("stale-token");
        assert_eq!(
            keychain_mode_get(&kc, adhoc_tokens.path(), SVC, ACCOUNT).unwrap(),
            "file-token"
        );
    }

    #[rstest]
    fn file_secret_replaces_a_stale_keychain_item(adhoc_tokens: tempfile::TempDir) {
        let kc = StubKeychain::holding("stale-token");
        keychain_mode_get(&kc, adhoc_tokens.path(), SVC, ACCOUNT).unwrap();
        assert_eq!(kc.value().as_deref(), Some("file-token"));
    }

    #[rstest]
    fn locked_keychain_still_serves_the_file_secret(adhoc_tokens: tempfile::TempDir) {
        let kc = StubKeychain {
            refuse_writes: true,
            ..Default::default()
        };
        assert_eq!(
            keychain_mode_get(&kc, adhoc_tokens.path(), SVC, ACCOUNT).unwrap(),
            "file-token"
        );
    }

    #[rstest]
    fn locked_keychain_keeps_the_file_secret(adhoc_tokens: tempfile::TempDir) {
        let kc = StubKeychain {
            refuse_writes: true,
            ..Default::default()
        };
        keychain_mode_get(&kc, adhoc_tokens.path(), SVC, ACCOUNT).unwrap();
        assert!(file_exists(&adhoc_tokens));
    }

    #[rstest]
    fn signed_build_reads_the_keychain(tokens: tempfile::TempDir) {
        let kc = StubKeychain::holding("kc-token");
        assert_eq!(
            keychain_mode_get(&kc, tokens.path(), SVC, ACCOUNT).unwrap(),
            "kc-token"
        );
    }

    #[rstest]
    fn signed_build_reports_a_missing_secret(tokens: tempfile::TempDir) {
        let kc = StubKeychain::default();
        assert!(matches!(
            keychain_mode_get(&kc, tokens.path(), SVC, ACCOUNT),
            Err(Error::NotFound(_))
        ));
    }

    #[rstest]
    fn keychain_write_removes_a_leftover_file(adhoc_tokens: tempfile::TempDir) {
        let kc = StubKeychain::default();
        keychain_mode_set(&kc, adhoc_tokens.path(), SVC, ACCOUNT, "new-token").unwrap();
        assert!(!file_exists(&adhoc_tokens));
    }

    #[rstest]
    fn adhoc_build_copies_a_keychain_secret_into_a_file(tokens: tempfile::TempDir) {
        let kc = StubKeychain::holding("kc-token");
        file_mode_get(Some(&kc), tokens.path(), SVC, ACCOUNT).unwrap();
        assert_eq!(
            file_get(tokens.path(), SVC, ACCOUNT).as_deref(),
            Some("kc-token")
        );
    }

    #[rstest]
    fn explicit_file_store_ignores_the_keychain(tokens: tempfile::TempDir) {
        assert!(matches!(
            file_mode_get(None, tokens.path(), SVC, ACCOUNT),
            Err(Error::NotFound(_))
        ));
    }

    #[rstest]
    fn file_store_is_owner_only(adhoc_tokens: tempfile::TempDir) {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(secret_path(adhoc_tokens.path(), SVC, ACCOUNT))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
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

    #[rstest]
    fn custom_client_never_needs_reconnect() {
        let g = GoogleGrant {
            custom_client: true,
            ..grant(Some("company-cid"))
        };
        assert!(!g.needs_reconnect("app-cid"));
    }

    #[rstest]
    fn custom_client_marker_survives_a_round_trip() {
        let g = GoogleGrant {
            custom_client: true,
            ..grant(Some("company-cid"))
        };
        assert!(GoogleGrant::parse(&serde_json::to_string(&g).unwrap()).custom_client);
    }

    #[rstest]
    fn app_client_grant_stores_no_marker() {
        assert!(
            !serde_json::to_string(&grant(Some("app-cid")))
                .unwrap()
                .contains("custom_client")
        );
    }
}
