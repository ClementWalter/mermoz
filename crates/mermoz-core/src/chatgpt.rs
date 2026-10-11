//! "Continue with ChatGPT": OpenAI's Sign in with ChatGPT for open-source, local apps.
//! Dynamic client registration + PKCE on a 127.0.0.1 loopback, ID token checked against
//! OpenAI's JWKS, rotating refresh tokens kept in the Keychain, and inference through the
//! Responses API billed to the user's ChatGPT plan (developers.openai.com/siwc).

use crate::error::{Error, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const APP_NAME: &str = "Mermoz";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const RESOURCE: &str = "https://api.openai.com/v1";
const ISSUER: &str = "https://auth.openai.com";

fn auth_base() -> String {
    std::env::var("MERMOZ_CHATGPT_AUTH_BASE").unwrap_or_else(|_| ISSUER.into())
}

pub fn api_base() -> String {
    std::env::var("MERMOZ_OPENAI_API_BASE").unwrap_or_else(|_| RESOURCE.into())
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Opaque, stable identifier of this Mac as an "agent host" (urn:uuid form).
pub fn new_host_id() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// What a signed-in ChatGPT account needs to resume and refresh after restart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Credential {
    pub client_id: String,
    pub subject: String,
    pub email: String,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub scopes: Vec<String>,
}

impl Credential {
    pub fn has_plan_usage(&self) -> bool {
        self.scopes.iter().any(|s| s == "chatgpt.tokens.use.direct")
    }
}

pub struct PendingLogin {
    pub url: String,
    listener: tokio::net::TcpListener,
    redirect: String,
    verifier: String,
    state: String,
    nonce: String,
    client_id: String,
}

/// Opens the loopback listener and builds the authorize URL. `reuse_client` is the issued
/// client id of an account signing in again; first-time sign-ins register dynamically.
pub async fn start(host_id: &str, reuse_client: Option<&str>) -> Result<PendingLogin> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let redirect = format!(
        "http://127.0.0.1:{}/auth/callback",
        listener.local_addr()?.port()
    );
    let verifier = b64(&rand::random::<[u8; 32]>());
    let state = b64(&rand::random::<[u8; 16]>());
    let nonce = b64(&rand::random::<[u8; 16]>());
    let client_id = reuse_client.unwrap_or("dynamic_agent_client").to_string();
    let mut url = url::Url::parse(&format!("{}/api/accounts/authorize", auth_base()))
        .map_err(|e| Error::other(e.to_string()))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("client_id", &client_id);
        if reuse_client.is_none() {
            q.append_pair("agent_name_hint", APP_NAME);
        }
        q.append_pair("ext_agent_host_id", host_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect)
            .append_pair("scope", SCOPES)
            .append_pair("resource", RESOURCE)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge_method", "S256")
            .append_pair(
                "code_challenge",
                &b64(&sha2::Sha256::digest(verifier.as_bytes())),
            );
    }
    Ok(PendingLogin {
        url: url.to_string(),
        listener,
        redirect,
        verifier,
        state,
        nonce,
        client_id,
    })
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Mermoz</title><body style=\"font-family:-apple-system,system-ui;background:#EAF3FB;color:#0D2436;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"background:#fff;padding:32px 40px;border-radius:16px;box-shadow:0 10px 30px rgba(31,111,181,.1)\"><h2 style=\"margin:0 0 8px\">ChatGPT is connected</h2><p style=\"margin:0;color:#4B6477\">You can close this tab and go back to Mermoz.</p></div>";

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdClaims {
    sub: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    nonce: Option<String>,
}

impl PendingLogin {
    /// Waits for the browser to come back, exchanges the code and validates the identity.
    pub async fn finish(self, timeout: Duration) -> Result<Credential> {
        let (code, issued) = tokio::time::timeout(timeout, self.callback())
            .await
            .map_err(|_| {
                Error::other("Sign-in timed out. Try again and finish in your browser.")
            })??;
        let client_id = match (self.client_id.as_str(), issued) {
            ("dynamic_agent_client", Some(c)) => c,
            ("dynamic_agent_client", None) => {
                return Err(Error::other(
                    "ChatGPT did not complete the registration. Try again.",
                ));
            }
            (saved, Some(c)) if c != saved => {
                return Err(Error::other(
                    "ChatGPT returned a different app registration.",
                ));
            }
            (saved, _) => saved.to_string(),
        };
        let http = reqwest::Client::new();
        let resp = http
            .post(format!("{}/api/accounts/oauth/token", auth_base()))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", client_id.as_str()),
                ("code", code.as_str()),
                ("code_verifier", self.verifier.as_str()),
                ("redirect_uri", self.redirect.as_str()),
                ("resource", RESOURCE),
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
        let id_token = tok
            .id_token
            .clone()
            .ok_or_else(|| Error::other("ChatGPT returned no identity"))?;
        let claims = validate_id_token(&id_token, &client_id).await?;
        if claims.nonce.as_deref() != Some(self.nonce.as_str()) {
            return Err(Error::other(
                "ChatGPT sign-in could not be verified (nonce)",
            ));
        }
        let scopes: Vec<String> = tok
            .scope
            .unwrap_or_default()
            .split_whitespace()
            .map(String::from)
            .collect();
        let cred = Credential {
            client_id,
            subject: claims.sub,
            email: claims.email,
            id_token,
            access_token: tok.access_token,
            refresh_token: tok
                .refresh_token
                .ok_or_else(|| Error::other("ChatGPT returned no refresh token"))?,
            expires_at: crate::model::now_ms() + tok.expires_in.unwrap_or(3600) * 1000,
            scopes,
        };
        if !cred.has_plan_usage() {
            return Err(Error::other(
                "ChatGPT plan use wasn't allowed. Sign in again and allow Mermoz to use your plan.",
            ));
        }
        Ok(cred)
    }

    async fn callback(&self) -> Result<(String, Option<String>)> {
        loop {
            let (mut sock, _) = self.listener.accept().await?;
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
            let url = url::Url::parse(&format!("http://127.0.0.1{path}"))
                .map_err(|e| Error::other(e.to_string()))?;
            if url.path() != "/auth/callback" {
                let _ = sock
                    .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                    .await;
                continue;
            }
            let get = |k: &str| {
                url.query_pairs()
                    .find(|(kk, _)| kk == k)
                    .map(|(_, v)| v.to_string())
            };
            let ok = get("error").is_none() && get("code").is_some();
            let body = if ok {
                DONE_PAGE
            } else {
                "Sign-in was cancelled. You can close this tab."
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            if get("state").as_deref() != Some(self.state.as_str()) {
                return Err(Error::other(
                    "ChatGPT sign-in could not be verified (state)",
                ));
            }
            if let Some(e) = get("error") {
                return Err(Error::other(if e == "access_denied" {
                    "Sign-in was cancelled.".to_string()
                } else {
                    format!("ChatGPT sign-in failed: {e}")
                }));
            }
            return Ok((get("code").unwrap_or_default(), get("client_id")));
        }
    }
}

/// Verifies signature (JWKS), issuer, audience and expiry of an OpenAI ID token.
async fn validate_id_token(token: &str, client_id: &str) -> Result<IdClaims> {
    let http = reqwest::Client::new();
    let base = auth_base();
    let jwks_uri = match http
        .get(format!("{base}/.well-known/openid-configuration"))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r.json::<Value>().await?["jwks_uri"]
            .as_str()
            .map(String::from),
        _ => None,
    }
    .unwrap_or_else(|| format!("{base}/.well-known/jwks.json"));
    let set: jsonwebtoken::jwk::JwkSet = http.get(&jwks_uri).send().await?.json().await?;
    let header = jsonwebtoken::decode_header(token)
        .map_err(|e| Error::other(format!("bad ID token: {e}")))?;
    let jwk = match &header.kid {
        Some(kid) => set.find(kid),
        None => set.keys.first(),
    }
    .ok_or_else(|| Error::other("ID token key not found"))?;
    let key = jsonwebtoken::DecodingKey::from_jwk(jwk)
        .map_err(|e| Error::other(format!("bad ID token key: {e}")))?;
    let mut v = jsonwebtoken::Validation::new(header.alg);
    v.set_audience(&[client_id]);
    v.set_issuer(&[ISSUER]);
    let data = jsonwebtoken::decode::<IdClaims>(token, &key, &v)
        .map_err(|e| Error::other(format!("ID token rejected: {e}")))?;
    Ok(data.claims)
}

/// Refreshes are serialized: OpenAI rotates refresh tokens, so two in flight would race.
static REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A valid access token, refreshing (and persisting the rotated tokens) near expiry.
pub async fn access_token(provider_id: &str) -> Result<String> {
    let _guard = REFRESH.lock().await;
    let mut c = load(provider_id)?;
    if c.expires_at - crate::model::now_ms() > 120_000 {
        return Ok(c.access_token);
    }
    let resp = reqwest::Client::new()
        .post(format!("{}/api/accounts/oauth/token", auth_base()))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", c.client_id.as_str()),
            ("refresh_token", c.refresh_token.as_str()),
            ("resource", RESOURCE),
        ])
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Upstream {
            status: 401,
            body: format!("ChatGPT sign-in expired ({status}). Sign in again in Settings."),
        });
    }
    let tok: TokenResponse = resp.json().await?;
    c.access_token = tok.access_token;
    if let Some(r) = tok.refresh_token {
        c.refresh_token = r;
    }
    c.expires_at = crate::model::now_ms() + tok.expires_in.unwrap_or(3600) * 1000;
    if let Some(s) = tok.scope {
        c.scopes = s.split_whitespace().map(String::from).collect();
    }
    save(provider_id, &c)?;
    Ok(c.access_token)
}

/// Models the signed-in account may use, in the server's order: (slug, display name).
pub async fn models(token: &str) -> Result<Vec<(String, String)>> {
    let v: Value = reqwest::Client::new()
        .get(format!("{}/models", api_base()))
        .bearer_auth(token)
        .send()
        .await?
        .json()
        .await?;
    Ok(v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| {
            Some((
                m["slug"].as_str()?.to_string(),
                m["display_name"].as_str().unwrap_or("").to_string(),
            ))
        })
        .collect())
}

/// One Responses API call (store:false, stream:true, as the plan route requires),
/// with structured output; returns the final output text.
pub async fn respond(
    token: &str,
    model: &str,
    instructions: &str,
    user: &str,
    schema: &Value,
) -> Result<String> {
    let body = json!({
        "model": model,
        "instructions": instructions,
        "input": [{ "role": "user", "content": user }],
        "store": false,
        "stream": true,
        "text": { "format": { "type": "json_schema", "name": "digest", "strict": true, "schema": schema } }
    });
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()?
        .post(format!("{}/responses", api_base()))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(Error::Upstream {
            status: status.as_u16(),
            body: text,
        });
    }
    output_from_sse(&text)
}

/// Final text from a Responses event stream: the completed response, else the deltas.
pub fn output_from_sse(stream: &str) -> Result<String> {
    let mut deltas = String::new();
    for line in stream.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(ev) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        match ev["type"].as_str() {
            Some("response.output_text.delta") => {
                deltas.push_str(ev["delta"].as_str().unwrap_or(""))
            }
            Some("response.completed") => {
                let text: String = ev["response"]["output"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|o| o["content"].as_array().cloned().unwrap_or_default())
                    .filter(|c| c["type"] == "output_text")
                    .filter_map(|c| c["text"].as_str().map(String::from))
                    .collect();
                if !text.is_empty() {
                    return Ok(text);
                }
            }
            Some("response.failed") | Some("error") => {
                return Err(Error::other(format!(
                    "ChatGPT request failed: {}",
                    ev["response"]["error"]["message"]
                        .as_str()
                        .or(ev["message"].as_str())
                        .unwrap_or("unknown error")
                )));
            }
            _ => {}
        }
    }
    if deltas.is_empty() {
        Err(Error::other("ChatGPT returned no text"))
    } else {
        Ok(deltas)
    }
}

// ---- credential storage (Keychain, or files under MERMOZ_TOKEN_STORE=file) ----

const SERVICE: &str = "ai.mermoz.chatgpt";

pub fn save(provider_id: &str, c: &Credential) -> Result<()> {
    crate::tokens::secret_set(SERVICE, provider_id, &serde_json::to_string(c)?)
}

pub fn load(provider_id: &str) -> Result<Credential> {
    Ok(serde_json::from_str(&crate::tokens::secret_get(
        SERVICE,
        provider_id,
    )?)?)
}

/// Revokes the renewable session, then forgets it locally.
pub async fn sign_out(provider_id: &str) -> Result<()> {
    let http = reqwest::Client::new();
    let revoke = match http
        .get(format!("{}/.well-known/openid-configuration", auth_base()))
        .send()
        .await
    {
        Ok(r) => r
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["revocation_endpoint"].as_str().map(String::from)),
        Err(_) => None,
    };
    if let (Ok(c), Some(endpoint)) = (load(provider_id), revoke) {
        let _ = http
            .post(endpoint)
            .form(&[
                ("token", c.refresh_token.as_str()),
                ("token_type_hint", "refresh_token"),
                ("client_id", c.client_id.as_str()),
            ])
            .send()
            .await;
    }
    crate::tokens::secret_delete(SERVICE, provider_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn host_id_is_uuid_urn() {
        assert!(new_host_id().starts_with("urn:uuid:"));
    }

    #[rstest]
    fn host_id_is_version_4() {
        assert_eq!(new_host_id().chars().nth(23), Some('4'));
    }

    #[rstest]
    fn completed_event_wins() {
        let s = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"content\":[{\"type\":\"output_text\",\"text\":\"{}\"}]}]}}\n";
        assert_eq!(output_from_sse(s).unwrap(), "{}");
    }

    #[rstest]
    fn deltas_are_a_fallback() {
        let s = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ab\"}\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"c\"}\n";
        assert_eq!(output_from_sse(s).unwrap(), "abc");
    }

    #[rstest]
    fn failed_event_is_an_error() {
        assert!(output_from_sse("data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"quota\"}}}\n").is_err());
    }

    #[rstest]
    fn plan_usage_needs_the_scope() {
        let c = Credential {
            client_id: "c".into(),
            subject: "s".into(),
            email: "e".into(),
            id_token: "i".into(),
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 0,
            scopes: vec!["openid".into()],
        };
        assert!(!c.has_plan_usage());
    }
}
