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

/// Upload address matching a REST base: Google serves media uploads under `/upload/`.
pub fn upload_base(base: &str) -> String {
    if let Ok(b) = std::env::var("MERMOZ_GMAIL_UPLOAD_BASE") {
        return b;
    }
    match base.split_once("googleapis.com/") {
        Some((host, rest)) => format!("{host}googleapis.com/upload/{rest}"),
        None => format!("{base}/upload"),
    }
}

/// Above this size a send goes through the upload endpoint: the JSON endpoint
/// rejects large bodies, the upload one takes messages up to 35 MB.
fn upload_over() -> usize {
    std::env::var("MERMOZ_GMAIL_UPLOAD_OVER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4 * 1024 * 1024)
}

/// What a request carries.
enum Payload<'a> {
    Empty,
    Json(&'a serde_json::Value),
    Bytes {
        content_type: &'a str,
        data: &'a [u8],
    },
}

/// `multipart/related` body for Gmail's upload endpoint: JSON metadata, then the message.
fn related_body(boundary: &str, meta: &serde_json::Value, raw: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: message/rfc822\r\n\r\n"
    )
    .into_bytes();
    out.extend_from_slice(raw);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
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

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response> {
        let payload = match &body {
            Some(b) => Payload::Json(b),
            None => Payload::Empty,
        };
        self.request(method, &format!("{}{}", self.base, path), query, payload)
            .await
    }

    /// Sends a request with auth, retrying rate limits and transient 5xx with backoff.
    async fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        query: &[(&str, String)],
        payload: Payload<'_>,
    ) -> Result<reqwest::Response> {
        let mut delay = Duration::from_millis(500);
        for attempt in 0..6 {
            let token = self.tokens.access_token().await?;
            let mut req = self
                .http
                .request(method.clone(), url)
                .bearer_auth(token)
                .query(query);
            match &payload {
                Payload::Empty => {}
                Payload::Json(b) => req = req.json(b),
                Payload::Bytes { content_type, data } => {
                    req = req
                        .header(reqwest::header::CONTENT_TYPE, *content_type)
                        .body(data.to_vec())
                }
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

    /// Gmail's own search, as typed in Gmail's search box (`q`), newest first.
    pub async fn search_messages(&self, q: &str, max: u32) -> Result<MessageList> {
        Ok(self
            .call(
                reqwest::Method::GET,
                "/messages",
                &[("q", q.to_string()), ("maxResults", max.to_string())],
                None,
            )
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

    /// Values of the named headers, in message order.
    pub async fn headers(&self, id: &str, names: &[&str]) -> Result<Vec<(String, String)>> {
        let mut q: Vec<(&str, String)> = vec![("format", "metadata".into())];
        q.extend(names.iter().map(|n| ("metadataHeaders", n.to_string())));
        let v: serde_json::Value = self
            .call(reqwest::Method::GET, &format!("/messages/{id}"), &q, None)
            .await?
            .json()
            .await?;
        Ok(v["payload"]["headers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| {
                Some((
                    h["name"].as_str()?.to_string(),
                    h["value"].as_str()?.to_string(),
                ))
            })
            .collect())
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

    /// Sends RFC 5322 bytes; `thread_id` files the message in an existing thread.
    pub async fn send(&self, raw: &[u8], thread_id: Option<&str>) -> Result<MessageRef> {
        if raw.len() > upload_over() {
            return self.send_upload(raw, thread_id).await;
        }
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

    /// Large messages: the upload endpoint takes the raw bytes as they are.
    async fn send_upload(&self, raw: &[u8], thread_id: Option<&str>) -> Result<MessageRef> {
        let meta = match thread_id {
            Some(t) => json!({ "threadId": t }),
            None => json!({}),
        };
        let boundary = format!("mermoz_{}", crate::model::new_id("b"));
        let body = related_body(&boundary, &meta, raw);
        let content_type = format!("multipart/related; boundary={boundary}");
        Ok(self
            .request(
                reqwest::Method::POST,
                &format!("{}/messages/send", upload_base(&self.base)),
                &[("uploadType", "multipart".into())],
                Payload::Bytes {
                    content_type: &content_type,
                    data: &body,
                },
            )
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

    #[rstest]
    #[case::google(
        "https://gmail.googleapis.com/gmail/v1/users/me",
        "https://gmail.googleapis.com/upload/gmail/v1/users/me"
    )]
    #[case::stub("http://127.0.0.1:9/gmail", "http://127.0.0.1:9/gmail/upload")]
    fn upload_base_follows_the_rest_base(#[case] base: &str, #[case] want: &str) {
        assert_eq!(upload_base(base), want);
    }

    #[rstest]
    fn related_body_ends_with_the_closing_boundary() {
        let body = related_body("B", &json!({ "threadId": "t1" }), b"Subject: x\r\n\r\nhi");
        assert!(body.ends_with(b"\r\n--B--\r\n"));
    }
}
