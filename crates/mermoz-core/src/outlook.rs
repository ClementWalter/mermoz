//! Thin Microsoft Graph mail client for Outlook accounts (personal and Microsoft 365):
//! only the calls the sync engine, the write-back queue and the send path need.
//!
//! Every request asks for immutable ids (`Prefer: IdType="ImmutableId"`), so a message
//! keeps its id when it moves between folders, and at most [`CONCURRENCY`] requests
//! run at once: Exchange throttles beyond four concurrent requests per mailbox.

use crate::error::{Error, Result};
use crate::oauth::TokenSource;
use base64::Engine;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

pub mod labels;
#[cfg(any(test, feature = "stub-graph"))]
pub mod stub;
#[cfg(test)]
mod tests;

pub fn default_base() -> String {
    std::env::var("MERMOZ_GRAPH_BASE").unwrap_or_else(|_| "https://graph.microsoft.com/v1.0".into())
}

/// Graph's per-mailbox limit on requests in flight.
pub const CONCURRENCY: usize = 4;

/// Message fields the sync reads: enough to place a message and know its state.
pub const SELECT: &str = "id,conversationId,parentFolderId,isRead,categories,flag,receivedDateTime";

/// Messages per page when listing or walking a delta.
const PAGE: usize = 100;

/// The longest wait honoured from a `Retry-After`.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub mail: Option<String>,
    pub user_principal_name: Option<String>,
}

impl Me {
    /// The address mail goes out from: `mail`, else the sign-in name (personal accounts
    /// often have no `mail`).
    pub fn email(&self) -> Option<String> {
        self.mail
            .clone()
            .filter(|m| !m.trim().is_empty())
            .or_else(|| self.user_principal_name.clone())
            .map(|m| m.trim().to_string())
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    pub parent_folder_id: Option<String>,
    #[serde(default)]
    pub child_folder_count: i64,
    #[serde(default)]
    pub total_item_count: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Flag {
    #[serde(default)]
    pub flag_status: String,
}

/// A message's place and state, as listed or reported by a delta.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MessageMeta {
    pub id: String,
    pub conversation_id: Option<String>,
    pub parent_folder_id: Option<String>,
    pub is_read: Option<bool>,
    #[serde(default)]
    pub categories: Vec<String>,
    pub flag: Option<Flag>,
    pub received_date_time: Option<String>,
    /// Set on a delta entry for a message that left the folder (moved or deleted).
    #[serde(rename = "@removed")]
    pub removed: Option<Value>,
}

impl MessageMeta {
    /// A delta may report a change with only some fields; such an entry is read again.
    pub fn is_complete(&self) -> bool {
        self.conversation_id.is_some() && self.parent_folder_id.is_some() && self.is_read.is_some()
    }

    pub fn flagged(&self) -> bool {
        self.flag
            .as_ref()
            .is_some_and(|f| f.flag_status == "flagged")
    }

    /// Received time in milliseconds since the epoch, 0 when unknown.
    pub fn received_ms(&self) -> i64 {
        self.received_date_time
            .as_deref()
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
            .map(|d| d.timestamp_millis())
            .unwrap_or(0)
    }
}

#[derive(Debug, Deserialize)]
pub struct Page<T> {
    #[serde(default = "Vec::new")]
    pub value: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    pub delta_link: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Category {
    display_name: String,
}

/// What a request carries.
enum Body<'a> {
    Empty,
    Json(&'a Value),
    Text(String),
}

/// Seconds from a `Retry-After` header (Graph sends whole seconds), capped.
pub fn retry_after(value: Option<&str>) -> Option<Duration> {
    let secs: u64 = value?.trim().parse().ok()?;
    Some(Duration::from_secs(secs).min(MAX_RETRY_AFTER))
}

pub struct Outlook {
    http: reqwest::Client,
    base: String,
    tokens: Arc<TokenSource>,
    slots: tokio::sync::Semaphore,
}

impl Outlook {
    pub fn new(base: String, tokens: Arc<TokenSource>) -> Self {
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(CONCURRENCY * 2)
            .timeout(Duration::from_secs(60))
            .build()
            .expect("http client");
        Self {
            http,
            base,
            tokens,
            slots: tokio::sync::Semaphore::new(CONCURRENCY),
        }
    }

    /// `path` under the base (`/me/...`), or an absolute link Graph handed out.
    fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{path}", self.base)
        }
    }

    /// Sends a request with auth, retrying throttling (honouring `Retry-After`) and
    /// transient failures with backoff.
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Body<'_>,
        prefer: &str,
    ) -> Result<reqwest::Response> {
        let url = self.url(path);
        let mut delay = Duration::from_millis(500);
        let _slot = self.slots.acquire().await.expect("semaphore open");
        for attempt in 0..6 {
            let token = self.tokens.access_token().await?;
            let mut req = self
                .http
                .request(method.clone(), &url)
                .bearer_auth(token)
                .header("Prefer", prefer);
            match &body {
                Body::Empty => {}
                Body::Json(v) => req = req.json(v),
                Body::Text(t) => {
                    req = req
                        .header(reqwest::header::CONTENT_TYPE, "text/plain")
                        .body(t.clone())
                }
            }
            let resp = match req.send().await {
                Ok(r) => r,
                // A failed connect never reached Microsoft, so even a send is safe to repeat.
                Err(e) if e.is_connect() && attempt < 5 => {
                    tracing::warn!("outlook: connect failed, retrying: {e}");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let status = resp.status().as_u16();
            if resp.status().is_success() {
                return Ok(resp);
            }
            if status == 401 && attempt == 0 {
                self.tokens.invalidate();
                continue;
            }
            if matches!(status, 429 | 500 | 502 | 503 | 504) && attempt < 5 {
                let wait = retry_after(
                    resp.headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok()),
                )
                .unwrap_or(delay);
                tracing::debug!(status, ?wait, "outlook: throttled or unavailable, retrying");
                tokio::time::sleep(wait).await;
                delay *= 2;
                continue;
            }
            return Err(Error::Upstream {
                status,
                body: resp.text().await.unwrap_or_default(),
            });
        }
        Err(Error::other("outlook: retries exhausted"))
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(self
            .request(reqwest::Method::GET, path, Body::Empty, IMMUTABLE)
            .await?
            .json()
            .await?)
    }

    pub async fn me(&self) -> Result<Me> {
        self.get_json("/me?$select=mail,userPrincipalName").await
    }

    /// A well-known folder (`inbox`, `sentitems`…); `None` when the mailbox has none.
    pub async fn well_known(&self, name: &str) -> Result<Option<Folder>> {
        match self.get_json(&format!("/me/mailFolders/{name}")).await {
            Ok(f) => Ok(Some(f)),
            Err(Error::Upstream { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Top-level folders (`parent` none) or the children of `parent`, every page.
    pub async fn folders(&self, parent: Option<&str>) -> Result<Vec<Folder>> {
        let mut link = Some(match parent {
            None => "/me/mailFolders?$top=250".to_string(),
            Some(p) => format!("/me/mailFolders/{p}/childFolders?$top=250"),
        });
        let mut out = vec![];
        while let Some(l) = link {
            let page: Page<Folder> = self.get_json(&l).await?;
            out.extend(page.value);
            link = page.next_link;
        }
        Ok(out)
    }

    /// One page of the mailbox, newest first; `link` resumes a listing.
    pub async fn messages_page(&self, link: Option<&str>) -> Result<Page<MessageMeta>> {
        match link {
            Some(l) => self.get_json(l).await,
            None => {
                self.get_json(&format!(
                    "/me/messages?$select={SELECT}&$orderby=receivedDateTime%20desc&$top={PAGE}"
                ))
                .await
            }
        }
    }

    /// The newest `n` messages of one folder.
    pub async fn recent(&self, folder: &str, n: usize) -> Result<Vec<MessageMeta>> {
        let page: Page<MessageMeta> = self
            .get_json(&format!(
                "/me/mailFolders/{folder}/messages?$select={SELECT}&$orderby=receivedDateTime%20desc&$top={n}"
            ))
            .await?;
        Ok(page.value)
    }

    /// One page of a folder's changes: from the start (`link` none), or from a saved
    /// next or delta link. `Ok(None)` means the link expired and the folder must be
    /// walked again from the start.
    pub async fn delta(
        &self,
        folder: &str,
        link: Option<&str>,
    ) -> Result<Option<Page<MessageMeta>>> {
        let path = match link {
            Some(l) => l.to_string(),
            None => format!("/me/mailFolders/{folder}/messages/delta?$select={SELECT}"),
        };
        let prefer = format!("{IMMUTABLE}, odata.maxpagesize={PAGE}");
        match self
            .request(reqwest::Method::GET, &path, Body::Empty, &prefer)
            .await
        {
            Ok(r) => Ok(Some(r.json().await?)),
            Err(Error::Upstream { status: 410, .. }) => Ok(None),
            // Graph answers an expired or unknown sync state this way too.
            Err(Error::Upstream { status: 400, body })
                if body.contains("syncStateNotFound") || body.contains("resyncRequired") =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub async fn message(&self, id: &str) -> Result<MessageMeta> {
        self.get_json(&format!("/me/messages/{id}?$select={SELECT}"))
            .await
    }

    /// The message exactly as received (MIME).
    pub async fn mime(&self, id: &str) -> Result<Vec<u8>> {
        Ok(self
            .request(
                reqwest::Method::GET,
                &format!("/me/messages/{id}/$value"),
                Body::Empty,
                IMMUTABLE,
            )
            .await?
            .bytes()
            .await?
            .to_vec())
    }

    /// Changes read state, categories or flag.
    pub async fn update(&self, id: &str, patch: &Value) -> Result<()> {
        self.request(
            reqwest::Method::PATCH,
            &format!("/me/messages/{id}"),
            Body::Json(patch),
            IMMUTABLE,
        )
        .await?;
        Ok(())
    }

    /// Moves a message to another folder; with immutable ids it keeps its id.
    pub async fn move_to(&self, id: &str, folder: &str) -> Result<()> {
        self.request(
            reqwest::Method::POST,
            &format!("/me/messages/{id}/move"),
            Body::Json(&json!({ "destinationId": folder })),
            IMMUTABLE,
        )
        .await?;
        Ok(())
    }

    /// Sends RFC 5322 bytes as they are; Outlook files a copy in Sent Items.
    pub async fn send_mime(&self, raw: &[u8]) -> Result<()> {
        let body = base64::engine::general_purpose::STANDARD.encode(raw);
        self.request(
            reqwest::Method::POST,
            "/me/sendMail",
            Body::Text(body),
            IMMUTABLE,
        )
        .await?;
        Ok(())
    }

    /// Names of the mailbox's categories.
    pub async fn categories(&self) -> Result<Vec<String>> {
        let page: Page<Category> = self.get_json("/me/outlook/masterCategories").await?;
        Ok(page.value.into_iter().map(|c| c.display_name).collect())
    }

    pub async fn folder(&self, id: &str) -> Result<Folder> {
        self.get_json(&format!("/me/mailFolders/{id}")).await
    }

    pub async fn rename_folder(&self, id: &str, name: &str) -> Result<()> {
        self.request(
            reqwest::Method::PATCH,
            &format!("/me/mailFolders/{id}"),
            Body::Json(&json!({ "displayName": name })),
            IMMUTABLE,
        )
        .await?;
        Ok(())
    }

    /// Deletes a folder; Outlook moves whatever is still in it to Deleted Items.
    pub async fn delete_folder(&self, id: &str) -> Result<()> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/me/mailFolders/{id}"),
            Body::Empty,
            IMMUTABLE,
        )
        .await?;
        Ok(())
    }
}

const IMMUTABLE: &str = "IdType=\"ImmutableId\"";
