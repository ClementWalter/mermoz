//! Thin Gmail REST client: only the calls the sync engine and send path need.

use crate::error::{Error, Result};
use crate::oauth::TokenSource;
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

pub fn default_base() -> String {
    std::env::var("MERMOZ_GMAIL_BASE")
        .unwrap_or_else(|_| "https://gmail.googleapis.com/gmail/v1/users/me".into())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub email_address: String,
    pub messages_total: Option<i64>,
    pub history_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRef {
    pub id: String,
    pub thread_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageList {
    #[serde(default)]
    pub messages: Vec<MessageRef>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawMessage {
    pub id: String,
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    pub internal_date: Option<String>,
    pub raw: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessage {
    pub message: MessageRef,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    #[serde(default)]
    pub messages_added: Vec<HistoryMessage>,
    #[serde(default)]
    pub labels_added: Vec<HistoryMessage>,
    #[serde(default)]
    pub labels_removed: Vec<HistoryMessage>,
    #[serde(default)]
    pub messages_deleted: Vec<HistoryMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    pub next_page_token: Option<String>,
    pub history_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct LabelList {
    #[serde(default)]
    labels: Vec<Label>,
}

pub struct Gmail {
    http: reqwest::Client,
    base: String,
    tokens: Arc<TokenSource>,
}

impl Gmail {
    pub fn new(base: String, tokens: Arc<TokenSource>) -> Self {
        // HTTP/1.1 pool: Google's HTTP/2 DATA framing under concurrent raw fetches
        // trips h2's anti-DoS guard ("too_many_data_frames") and drops connections.
        let http = reqwest::Client::builder()
            .http1_only()
            .pool_max_idle_per_host(32)
            .timeout(Duration::from_secs(60))
            .build()
            .expect("http client");
        Self { http, base, tokens }
    }

    /// Sends a request with auth, retrying rate limits and transient 5xx with backoff.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response> {
        let mut delay = Duration::from_millis(500);
        for attempt in 0..6 {
            let token = self.tokens.access_token().await?;
            let mut req = self
                .http
                .request(method.clone(), format!("{}{}", self.base, path))
                .bearer_auth(token)
                .query(query);
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            if resp.status().is_success() {
                return Ok(resp);
            }
            if status == 401 && attempt == 0 {
                self.tokens.invalidate();
                continue;
            }
            if (status == 429 || status == 403 || status >= 500) && attempt < 5 {
                let text = resp.text().await.unwrap_or_default();
                // 403 is only retried for rate-limit reasons, not permission errors.
                if status == 403 && !text.contains("ateLimit") {
                    return Err(Error::Upstream { status, body: text });
                }
                tokio::time::sleep(delay).await;
                delay *= 2;
                continue;
            }
            return Err(Error::Upstream {
                status,
                body: resp.text().await.unwrap_or_default(),
            });
        }
        Err(Error::other("gmail: retries exhausted"))
    }

    pub async fn profile(&self) -> Result<Profile> {
        Ok(self
            .call(reqwest::Method::GET, "/profile", &[], None)
            .await?
            .json()
            .await?)
    }

    pub async fn list_messages(&self, page_token: Option<&str>) -> Result<MessageList> {
        let mut q = vec![("maxResults", "500".to_string())];
        if let Some(t) = page_token {
            q.push(("pageToken", t.to_string()));
        }
        Ok(self
            .call(reqwest::Method::GET, "/messages", &q, None)
            .await?
            .json()
            .await?)
    }

    pub async fn get_raw(&self, id: &str) -> Result<RawMessage> {
        Ok(self
            .call(
                reqwest::Method::GET,
                &format!("/messages/{id}"),
                &[("format", "raw".into())],
                None,
            )
            .await?
            .json()
            .await?)
    }

    /// Labels only: cheap refresh for label-change history events.
    pub async fn get_minimal(&self, id: &str) -> Result<RawMessage> {
        Ok(self
            .call(
                reqwest::Method::GET,
                &format!("/messages/{id}"),
                &[("format", "minimal".into())],
                None,
            )
            .await?
            .json()
            .await?)
    }

    /// `Ok(None)` means the start id is too old and a full resync is needed.
    pub async fn history(
        &self,
        start: &str,
        page_token: Option<&str>,
    ) -> Result<Option<HistoryPage>> {
        let mut q = vec![
            ("startHistoryId", start.to_string()),
            ("maxResults", "500".to_string()),
        ];
        if let Some(t) = page_token {
            q.push(("pageToken", t.to_string()));
        }
        match self.call(reqwest::Method::GET, "/history", &q, None).await {
            Ok(r) => Ok(Some(r.json().await?)),
            Err(Error::Upstream { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn labels(&self) -> Result<Vec<Label>> {
        let l: LabelList = self
            .call(reqwest::Method::GET, "/labels", &[], None)
            .await?
            .json()
            .await?;
        Ok(l.labels)
    }

    pub async fn create_label(&self, name: &str) -> Result<Label> {
        let body = json!({ "name": name, "labelListVisibility": "labelShow", "messageListVisibility": "show" });
        Ok(self
            .call(reqwest::Method::POST, "/labels", &[], Some(body))
            .await?
            .json()
            .await?)
    }

    pub async fn rename_label(&self, id: &str, name: &str) -> Result<()> {
        self.call(
            reqwest::Method::PATCH,
            &format!("/labels/{id}"),
            &[],
            Some(json!({ "name": name })),
        )
        .await?;
        Ok(())
    }

    /// Deletes a label; Gmail removes it from every message, the messages stay.
    pub async fn delete_label(&self, id: &str) -> Result<()> {
        self.call(reqwest::Method::DELETE, &format!("/labels/{id}"), &[], None)
            .await?;
        Ok(())
    }

    /// Moves a thread to Trash; Gmail purges it after 30 days.
    pub async fn trash_thread(&self, thread_id: &str) -> Result<()> {
        self.call(
            reqwest::Method::POST,
            &format!("/threads/{thread_id}/trash"),
            &[],
            // Gmail answers 411 to a POST without a body.
            Some(json!({})),
        )
        .await?;
        Ok(())
    }

    pub async fn untrash_thread(&self, thread_id: &str) -> Result<()> {
        self.call(
            reqwest::Method::POST,
            &format!("/threads/{thread_id}/untrash"),
            &[],
            // Gmail answers 411 to a POST without a body.
            Some(json!({})),
        )
        .await?;
        Ok(())
    }

    pub async fn modify_thread(
        &self,
        thread_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let body = json!({ "addLabelIds": add, "removeLabelIds": remove });
        self.call(
            reqwest::Method::POST,
            &format!("/threads/{thread_id}/modify"),
            &[],
            Some(body),
        )
        .await?;
        Ok(())
    }

    pub async fn send(&self, raw: &[u8], thread_id: Option<&str>) -> Result<MessageRef> {
        let mut body = json!({ "raw": base64::engine::general_purpose::URL_SAFE.encode(raw) });
        if let Some(t) = thread_id {
            body["threadId"] = json!(t);
        }
        Ok(self
            .call(reqwest::Method::POST, "/messages/send", &[], Some(body))
            .await?
            .json()
            .await?)
    }
}

/// Gmail returns raw messages as base64url, with or without padding.
pub fn decode_raw(raw: &str) -> Result<Vec<u8>> {
    let trimmed = raw.trim_end_matches('=');
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed)
        .map_err(|e| Error::other(format!("bad base64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::unpadded("aGk")]
    #[case::padded("aGk=")]
    fn decode_raw_accepts_padding_variants(#[case] input: &str) {
        assert_eq!(decode_raw(input).unwrap(), b"hi");
    }
}
