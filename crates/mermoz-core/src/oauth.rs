//! Google OAuth for installed apps: loopback redirect + PKCE, as Google
//! recommends for desktop clients. No server involved.

use crate::error::{Error, Result};
use crate::paths;
use base64::Engine;
use serde::Deserialize;
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
}

pub struct PendingAuth {
    pub auth_url: String,
    listener: tokio::net::TcpListener,
    verifier: String,
    state: String,
    redirect: String,
    cfg: ClientConfig,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Opens the loopback listener and builds the consent URL.
pub async fn start(cfg: ClientConfig) -> Result<PendingAuth> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = b64(&rand::random::<[u8; 32]>());
    let challenge = b64(&sha2::Sha256::digest(verifier.as_bytes()));
    let state = b64(&rand::random::<[u8; 16]>());
    let mut url = url::Url::parse(&cfg.auth_url).map_err(|e| Error::other(e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);
    Ok(PendingAuth {
        auth_url: url.to_string(),
        listener,
        verifier,
        state,
        redirect,
        cfg,
    })
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Mermoz</title><body style=\"font-family:-apple-system,system-ui;background:#EAF3FB;color:#0D2436;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"background:#fff;padding:32px 40px;border-radius:16px;box-shadow:0 10px 30px rgba(31,111,181,.1)\"><h2 style=\"margin:0 0 8px\">You're all set</h2><p style=\"margin:0;color:#4B6477\">Mermoz is syncing your mail. You can close this tab.</p></div>";

impl PendingAuth {
    /// Waits for Google's redirect, exchanges the code; returns the refresh token.
    pub async fn finish(self, timeout: Duration) -> Result<String> {
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
                    "Sign-in was cancelled. You can close this tab."
                } else {
                    DONE_PAGE
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                if let Some(e) = get("error") {
                    return Err(Error::other(format!("authorization denied: {e}")));
                }
                if get("state").as_deref() != Some(self.state.as_str()) {
                    return Err(Error::other("oauth state mismatch"));
                }
                return Ok::<String, Error>(get("code").unwrap());
            }
        };
        let code = tokio::time::timeout(timeout, accept)
            .await
            .map_err(|_| Error::other("sign-in timed out"))??;
        let http = reqwest::Client::new();
        let resp = http
            .post(&self.cfg.token_url)
            .form(&[
                ("code", code.as_str()),
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
        tok.refresh_token
            .ok_or_else(|| Error::other("Google returned no refresh token"))
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
    fn parses_flat_file() {
        assert_eq!(
            ClientConfig::parse(r#"{"client_id":"cid","client_secret":"s"}"#)
                .unwrap()
                .client_secret,
            "s"
        );
    }
}
