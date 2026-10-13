//! Google OAuth for installed apps: loopback redirect + PKCE, as Google
//! recommends for desktop clients. No server involved.

use crate::error::{Error, Result};
use crate::paths;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";

#[derive(Debug, Clone, Deserialize)]
pub struct ClientConfig {
    pub client_id: String,
    pub client_secret: String,
    #[serde(default = "default_auth_url")]
    pub auth_url: String,
    #[serde(default = "default_token_url")]
    pub token_url: String,
}

fn default_auth_url() -> String {
    "https://accounts.google.com/o/oauth2/v2/auth".into()
}

fn default_token_url() -> String {
    std::env::var("MERMOZ_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://oauth2.googleapis.com/token".into())
}

impl ClientConfig {
    pub fn with_ids(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret,
            auth_url: default_auth_url(),
            token_url: default_token_url(),
        }
    }

    /// Accepts Mermoz's flat file or the JSON Google Cloud console downloads ({"installed": {...}}).
    pub fn parse(raw: &str) -> Result<Self> {
        let v: serde_json::Value = serde_json::from_str(raw)?;
        let inner = v.get("installed").cloned().unwrap_or(v);
        Ok(serde_json::from_value(inner)?)
    }

    /// A client the user brings for one account: the JSON downloaded from Google Cloud,
    /// or its ID and secret pasted. Neither may be blank.
    pub fn from_user(
        json: Option<&str>,
        client_id: Option<&str>,
        client_secret: Option<&str>,
    ) -> Result<Self> {
        let cfg = match (json, client_id, client_secret) {
            (Some(raw), _, _) => Self::parse(raw).map_err(|_| {
                Error::InvalidParams(
                    "that file isn't a Google sign-in client (expected the client_secret_….json from Google Cloud)".into(),
                )
            })?,
            (None, Some(id), Some(secret)) => {
                Self::with_ids(id.trim().into(), secret.trim().into())
            }
            _ => {
                return Err(Error::InvalidParams(
                    "give the client JSON file, or both the client ID and secret".into(),
                ));
            }
        };
        if cfg.client_id.trim().is_empty() || cfg.client_secret.trim().is_empty() {
            return Err(Error::InvalidParams(
                "the client ID and secret can't be empty".into(),
            ));
        }
        Ok(cfg)
    }

    /// Build-time credentials (release builds), then env, then `MERMOZ_HOME/oauth_client.json`.
    pub fn load() -> Result<Self> {
        if let (Some(id), Some(secret)) = (
            option_env!("MERMOZ_GOOGLE_CLIENT_ID"),
            option_env!("MERMOZ_GOOGLE_CLIENT_SECRET"),
        ) {
            return Ok(Self {
                client_id: id.into(),
                client_secret: secret.into(),
                auth_url: default_auth_url(),
                token_url: default_token_url(),
            });
        }
        if let (Ok(id), Ok(secret)) = (
            std::env::var("MERMOZ_GOOGLE_CLIENT_ID"),
            std::env::var("MERMOZ_GOOGLE_CLIENT_SECRET"),
        ) {
            return Ok(Self {
                client_id: id,
                client_secret: secret,
                auth_url: default_auth_url(),
                token_url: default_token_url(),
            });
        }
        let p = paths::oauth_client_path();
        let raw = std::fs::read_to_string(&p).map_err(|_| {
            Error::other(format!(
                "no Google OAuth client configured (expected {})",
                p.display()
            ))
        })?;
        Self::parse(&raw)
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    /// Space-separated scopes actually granted: with granular consent the user may
    /// untick some of those asked for.
    #[serde(default)]
    scope: Option<String>,
}

/// What a consent screen asks for.
#[derive(Debug, Clone)]
pub struct AuthRequest {
    pub scopes: Vec<String>,
    /// Preselects the Google account, so the user approves for the right one.
    pub login_hint: Option<String>,
    /// Asks for a refresh token. Google keeps at most 100 per account and client and
    /// silently drops the oldest beyond that, so one-off grants (the file picker) don't.
    pub offline: bool,
    /// More query parameters, e.g. `trigger_onepick=true` for the Drive file picker.
    pub extra: Vec<(String, String)>,
    /// The sentence the browser tab shows once Mermoz has the answer.
    pub done_message: String,
}

impl AuthRequest {
    pub fn gmail() -> Self {
        Self {
            scopes: vec![SCOPE.into()],
            login_hint: None,
            offline: true,
            extra: vec![],
            done_message: "Mermoz is syncing your mail. You can close this tab.".into(),
        }
    }
}

/// The outcome of a consent: the tokens, what was granted, and (file picker) the
/// files the user chose.
#[derive(Debug, Clone)]
pub struct Granted {
    pub access_token: String,
    pub expires_in: Option<u64>,
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
    pub picked: Vec<String>,
}

pub struct PendingAuth {
    pub auth_url: String,
    listener: tokio::net::TcpListener,
    verifier: String,
    state: String,
    redirect: String,
    cfg: ClientConfig,
    done_message: String,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Opens the loopback listener and builds the Gmail consent URL.
pub async fn start(cfg: ClientConfig) -> Result<PendingAuth> {
    start_request(cfg, AuthRequest::gmail()).await
}

/// Opens the loopback listener and builds the consent URL for `req`.
pub async fn start_request(cfg: ClientConfig, req: AuthRequest) -> Result<PendingAuth> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = b64(&rand::random::<[u8; 32]>());
    let challenge = b64(&sha2::Sha256::digest(verifier.as_bytes()));
    let state = b64(&rand::random::<[u8; 16]>());
    let mut url = url::Url::parse(&cfg.auth_url).map_err(|e| Error::other(e.to_string()))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("client_id", &cfg.client_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("response_type", "code")
            .append_pair("scope", &req.scopes.join(" "))
            .append_pair(
                "access_type",
                if req.offline { "offline" } else { "online" },
            )
            .append_pair("prompt", "consent")
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state);
        if let Some(h) = &req.login_hint {
            q.append_pair("login_hint", h);
        }
        for (k, v) in &req.extra {
            q.append_pair(k, v);
        }
    }
    Ok(PendingAuth {
        auth_url: url.to_string(),
        listener,
        verifier,
        state,
        redirect,
        cfg,
        done_message: req.done_message,
    })
}

fn done_page(message: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>Mermoz</title><body style=\"font-family:-apple-system,system-ui;background:#EAF3FB;color:#0D2436;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"background:#fff;padding:32px 40px;border-radius:16px;box-shadow:0 10px 30px rgba(31,111,181,.1)\"><h2 style=\"margin:0 0 8px\">You're all set</h2><p style=\"margin:0;color:#4B6477\">{message}</p></div>"
    )
}

/// What the browser brought back: an authorization code and any picked files.
struct Redirect {
    code: String,
    picked: Vec<String>,
}

impl PendingAuth {
    /// Waits for Google's redirect, exchanges the code; returns the refresh token.
    pub async fn finish(self, timeout: Duration) -> Result<String> {
        self.finish_grant(timeout)
            .await?
            .refresh_token
            .ok_or_else(|| Error::other("Google returned no refresh token"))
    }

    /// Waits for Google's redirect and exchanges the code. A refusal on the consent
    /// screen is [`Error::Declined`].
    pub async fn finish_grant(self, timeout: Duration) -> Result<Granted> {
        let accept = async {
            loop {
                let (mut sock, _) = self.listener.accept().await?;
                // Requests can arrive in several TCP segments: read until the header block ends.
                let mut buf = Vec::with_capacity(2048);
                let mut chunk = [0u8; 2048];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16384 {
                    let n = sock.read(&mut chunk).await?;
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let req = String::from_utf8_lossy(&buf).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let url = url::Url::parse(&format!("http://localhost{path}"))
                    .map_err(|e| Error::other(e.to_string()))?;
                let get = |k: &str| {
                    url.query_pairs()
                        .find(|(kk, _)| kk == k)
                        .map(|(_, v)| v.to_string())
                };
                if get("code").is_none() && get("error").is_none() {
                    // Favicon or stray request: ignore and keep waiting.
                    let _ = sock
                        .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                        .await;
                    continue;
                }
                let body = if get("error").is_some() {
                    "Nothing was changed. You can close this tab.".to_string()
                } else {
                    done_page(&self.done_message)
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                if let Some(e) = get("error") {
                    // The description tells a cancel from an organization's block.
                    return Err(Error::Declined(match get("error_description") {
                        Some(d) => format!("{e}: {d}"),
                        None => e,
                    }));
                }
                if get("state").as_deref() != Some(self.state.as_str()) {
                    return Err(Error::other("oauth state mismatch"));
                }
                let picked = get("picked_file_ids")
                    .map(|ids| {
                        ids.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                return Ok::<Redirect, Error>(Redirect {
                    code: get("code").unwrap(),
                    picked,
                });
            }
        };
        let redirect = tokio::time::timeout(timeout, accept)
            .await
            .map_err(|_| Error::other("sign-in timed out"))??;
        let http = reqwest::Client::new();
        let resp = http
            .post(&self.cfg.token_url)
            .form(&[
                ("code", redirect.code.as_str()),
                ("client_id", &self.cfg.client_id),
                ("client_secret", &self.cfg.client_secret),
                ("redirect_uri", &self.redirect),
                ("grant_type", "authorization_code"),
                ("code_verifier", &self.verifier),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Upstream {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        let tok: TokenResponse = resp.json().await?;
        Ok(Granted {
            access_token: tok.access_token,
            expires_in: tok.expires_in,
            refresh_token: tok.refresh_token,
            scopes: tok
                .scope
                .unwrap_or_default()
                .split_whitespace()
                .map(String::from)
                .collect(),
            picked: redirect.picked,
        })
    }
}

/// Why Google refused a sign-in, from what it said (the redirect's `error=`, a token
/// endpoint body, or the words on its own error page).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// `admin_policy_enforced`: the Workspace only allows apps its admin trusts.
    AdminPolicy,
    /// "… has not completed the Google verification process".
    Unverified,
    /// `org_internal`: an internal client used with an account outside its organization.
    OrgInternal,
    /// `access_denied` from the Workspace's app access control.
    AppBlocked,
    /// `access_denied` with nothing more: a cancel, or a block Google didn't explain.
    Declined,
}

impl Refusal {
    pub fn classify(text: &str) -> Option<Self> {
        let t = text.to_ascii_lowercase();
        let any = |words: &[&str]| words.iter().any(|w| t.contains(w));
        // Most specific first: Google's pages also say "access_denied" and "blocked".
        if t.contains("admin_policy_enforced") {
            Some(Self::AdminPolicy)
        } else if t.contains("org_internal") {
            Some(Self::OrgInternal)
        } else if any(&["verification process", "not been verified", "unverified"]) {
            Some(Self::Unverified)
        } else if any(&["access_denied", "access blocked"]) {
            if any(&[
                "admin",
                "blocked",
                "organization",
                "organisation",
                "institution",
                "trust",
            ]) {
                Some(Self::AppBlocked)
            } else {
                Some(Self::Declined)
            }
        } else {
            None
        }
    }

    /// The explanation and the fix, in plain words. `custom`: the consent used the
    /// user's own sign-in client rather than Mermoz's.
    pub fn message(self, custom: bool) -> String {
        let trust = if custom {
            "Ask your Google Workspace admin to trust this sign-in client (client ID below)"
        } else {
            "Ask your Google Workspace admin to trust Mermoz (client ID below)"
        };
        let own = "or use your own sign-in client (Settings › Developer)";
        match self {
            Self::AdminPolicy => {
                format!("Your organization only allows apps it trusts. {trust}, {own}.")
            }
            Self::Unverified => format!(
                "Google stopped the sign-in because this app hasn't finished Google's review yet. {trust}, {own}."
            ),
            Self::OrgInternal => format!(
                "This sign-in client only works for accounts of the organization that made it. Sign in with an account from that organization, {own}."
            ),
            Self::AppBlocked => format!(
                "Your Google Workspace admin has blocked this app for your organization. {trust}, {own}."
            ),
            Self::Declined => format!(
                "Google didn't give Mermoz access. If you didn't cancel, your organization may only allow apps it trusts: {}, {own}.",
                trust.replacen("Ask", "ask", 1)
            ),
        }
    }
}

/// Access tokens from a refresh token, cached until shortly before expiry.
pub struct TokenSource {
    cfg: ClientConfig,
    refresh_token: String,
    http: reqwest::Client,
    cache: Mutex<Option<(String, Instant)>>,
}

impl TokenSource {
    pub fn new(cfg: ClientConfig, refresh_token: String) -> Self {
        Self {
            cfg,
            refresh_token,
            http: reqwest::Client::new(),
            cache: Mutex::new(None),
        }
    }

    /// A one-off access token with no refresh token behind it (an online grant).
    pub fn fixed(cfg: ClientConfig, access_token: String, expires_in: Option<u64>) -> Self {
        let ttl = expires_in.unwrap_or(3600).saturating_sub(120);
        Self {
            cfg,
            refresh_token: String::new(),
            http: reqwest::Client::new(),
            cache: Mutex::new(Some((
                access_token,
                Instant::now() + Duration::from_secs(ttl),
            ))),
        }
    }

    pub async fn access_token(&self) -> Result<String> {
        if let Some((t, exp)) = self.cache.lock().unwrap().clone()
            && Instant::now() < exp
        {
            return Ok(t);
        }
        let resp = self
            .http
            .post(&self.cfg.token_url)
            .form(&[
                ("client_id", self.cfg.client_id.as_str()),
                ("client_secret", &self.cfg.client_secret),
                ("refresh_token", &self.refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Upstream {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        let tok: TokenResponse = resp.json().await?;
        let ttl = tok.expires_in.unwrap_or(3600).saturating_sub(120);
        *self.cache.lock().unwrap() = Some((
            tok.access_token.clone(),
            Instant::now() + Duration::from_secs(ttl),
        ));
        Ok(tok.access_token)
    }

    pub fn invalidate(&self) {
        *self.cache.lock().unwrap() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn parses_google_console_download() {
        let raw = r#"{"installed":{"client_id":"cid","client_secret":"s","token_uri":"https://oauth2.googleapis.com/token"}}"#;
        assert_eq!(ClientConfig::parse(raw).unwrap().client_id, "cid");
    }

    #[rstest]
    #[case::admin_policy("admin_policy_enforced", Refusal::AdminPolicy)]
    #[case::admin_policy_page("Error 400: admin_policy_enforced", Refusal::AdminPolicy)]
    #[case::unverified(
        "Access blocked: Mermoz has not completed the Google verification process. Error 403: access_denied",
        Refusal::Unverified
    )]
    #[case::org_internal("org_internal", Refusal::OrgInternal)]
    #[case::org_internal_page("Error 403: org_internal", Refusal::OrgInternal)]
    #[case::app_access_control(
        "access_denied: Your administrator has blocked access to Mermoz",
        Refusal::AppBlocked
    )]
    #[case::bare_access_denied("access_denied", Refusal::Declined)]
    fn google_refusals_are_recognised(#[case] text: &str, #[case] expected: Refusal) {
        assert_eq!(Refusal::classify(text), Some(expected));
    }

    #[rstest]
    fn other_errors_are_not_refusals() {
        assert_eq!(Refusal::classify("invalid_grant"), None);
    }

    #[rstest]
    fn admin_policy_offers_both_fixes() {
        assert_eq!(
            Refusal::AdminPolicy.message(false),
            "Your organization only allows apps it trusts. Ask your Google Workspace admin to trust Mermoz (client ID below), or use your own sign-in client (Settings › Developer)."
        );
    }

    #[rstest]
    fn own_client_refusal_names_that_client() {
        assert!(
            Refusal::AdminPolicy
                .message(true)
                .contains("trust this sign-in client")
        );
    }

    #[rstest]
    fn user_client_from_google_download() {
        let raw = r#"{"installed":{"client_id":"company-cid","client_secret":"s"}}"#;
        assert_eq!(
            ClientConfig::from_user(Some(raw), None, None)
                .unwrap()
                .client_id,
            "company-cid"
        );
    }

    #[rstest]
    fn user_client_from_pasted_ids_is_trimmed() {
        assert_eq!(
            ClientConfig::from_user(None, Some(" company-cid \n"), Some("s"))
                .unwrap()
                .client_id,
            "company-cid"
        );
    }

    #[rstest]
    #[case::not_json(Some("hello"), None, None)]
    #[case::no_secret(None, Some("company-cid"), None)]
    #[case::blank_secret(None, Some("company-cid"), Some("  "))]
    #[case::nothing(None, None, None)]
    fn user_client_must_be_complete(
        #[case] json: Option<&str>,
        #[case] id: Option<&str>,
        #[case] secret: Option<&str>,
    ) {
        assert!(matches!(
            ClientConfig::from_user(json, id, secret),
            Err(Error::InvalidParams(_))
        ));
    }

    #[rstest]
    fn parses_flat_file() {
        assert_eq!(
            ClientConfig::parse(r#"{"client_id":"cid","client_secret":"s"}"#)
                .unwrap()
                .client_secret,
            "s"
        );
    }
}
