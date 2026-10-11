//! In-process stub of the Gmail REST API and Google's token endpoint, with
//! enough behavior (labels, history, send) to drive mermozd end to end.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct StubMessage {
    pub id: String,
    pub thread_id: String,
    pub labels: BTreeSet<String>,
    pub raw: String,
    pub internal_date: i64,
}

#[derive(Default, Debug)]
pub struct Mailbox {
    pub email: String,
    pub messages: BTreeMap<String, StubMessage>,
    pub labels: BTreeMap<String, String>, // id -> name
    pub history_id: u64,
    pub history: Vec<(u64, Value)>,
    pub sent: Vec<(String, Option<String>)>, // decoded raw, threadId
    /// API keys seen by the stub Anthropic endpoint.
    pub llm_keys: Vec<String>,
    /// Message ids whose next fetch fails with 503, to exercise retries.
    pub fail_once: BTreeSet<String>,
    next_label: u64,
}

pub type Shared = Arc<Mutex<Mailbox>>;

impl Mailbox {
    pub fn add_message(
        &mut self,
        id: &str,
        thread_id: &str,
        labels: &[&str],
        raw: &str,
        internal_date: i64,
    ) {
        self.history_id += 1;
        self.messages.insert(
            id.into(),
            StubMessage {
                id: id.into(),
                thread_id: thread_id.into(),
                labels: labels.iter().map(|s| s.to_string()).collect(),
                raw: raw.into(),
                internal_date,
            },
        );
        let h = self.history_id;
        self.history.push((h, json!({ "id": h.to_string(), "messagesAdded": [{ "message": { "id": id, "threadId": thread_id } }] })));
    }

    /// Simulates a change made on another device (phone, webmail).
    pub fn relabel_thread(&mut self, thread_id: &str, add: &[&str], remove: &[&str]) {
        let ids: Vec<String> = self
            .messages
            .values()
            .filter(|m| m.thread_id == thread_id)
            .map(|m| m.id.clone())
            .collect();
        for id in &ids {
            let m = self.messages.get_mut(id).unwrap();
            for a in add {
                m.labels.insert(a.to_string());
            }
            for r in remove {
                m.labels.remove(*r);
            }
        }
        self.history_id += 1;
        let h = self.history_id;
        let refs: Vec<Value> = ids
            .iter()
            .map(|id| json!({ "message": { "id": id, "threadId": thread_id } }))
            .collect();
        self.history
            .push((h, json!({ "id": h.to_string(), "labelsAdded": refs })));
    }

    pub fn label_id_for(&mut self, name: &str) -> String {
        if let Some((id, _)) = self.labels.iter().find(|(_, n)| *n == name) {
            return id.clone();
        }
        self.next_label += 1;
        let id = format!("Label_{}", self.next_label);
        self.labels.insert(id.clone(), name.into());
        id
    }

    pub fn thread_label_names(&self, thread_id: &str) -> BTreeSet<String> {
        self.messages
            .values()
            .filter(|m| m.thread_id == thread_id)
            .flat_map(|m| {
                m.labels
                    .iter()
                    .map(|l| self.labels.get(l).cloned().unwrap_or(l.clone()))
            })
            .collect()
    }
}

fn b64url(s: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes())
}

async fn token() -> Json<Value> {
    Json(
        json!({ "access_token": "stub-access", "expires_in": 3600, "refresh_token": "stub-refresh", "token_type": "Bearer" }),
    )
}

async fn profile(State(s): State<Shared>) -> Json<Value> {
    let m = s.lock().unwrap();
    Json(
        json!({ "emailAddress": m.email, "messagesTotal": m.messages.len(), "historyId": m.history_id.to_string() }),
    )
}

async fn list(State(s): State<Shared>) -> Json<Value> {
    let m = s.lock().unwrap();
    let mut msgs: Vec<&StubMessage> = m.messages.values().collect();
    msgs.sort_by_key(|x| std::cmp::Reverse(x.internal_date));
    let refs: Vec<Value> = msgs
        .iter()
        .map(|x| json!({ "id": x.id, "threadId": x.thread_id }))
        .collect();
    Json(json!({ "messages": refs }))
}

async fn get_message(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    let mut m = s.lock().unwrap();
    if m.fail_once.remove(&id) {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
    let msg = m
        .messages
        .get(&id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;
    let mut v = json!({ "id": msg.id, "threadId": msg.thread_id, "labelIds": msg.labels, "internalDate": msg.internal_date.to_string() });
    if q.get("format").map(|f| f.as_str()) == Some("raw") {
        v["raw"] = json!(b64url(&msg.raw));
    }
    Ok(Json(v))
}

async fn history(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let m = s.lock().unwrap();
    let start: u64 = q
        .get("startHistoryId")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let recs: Vec<Value> = m
        .history
        .iter()
        .filter(|(h, _)| *h > start)
        .map(|(_, r)| r.clone())
        .collect();
    Json(json!({ "history": recs, "historyId": m.history_id.to_string() }))
}

async fn labels(State(s): State<Shared>) -> Json<Value> {
    let m = s.lock().unwrap();
    let mut out: Vec<Value> = ["INBOX", "UNREAD", "SENT"]
        .iter()
        .map(|l| json!({ "id": l, "name": l }))
        .collect();
    out.extend(
        m.labels
            .iter()
            .map(|(id, n)| json!({ "id": id, "name": n })),
    );
    Json(json!({ "labels": out }))
}

async fn create_label(State(s): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let mut m = s.lock().unwrap();
    let name = body["name"].as_str().unwrap_or("").to_string();
    let id = m.label_id_for(&name);
    Json(json!({ "id": id, "name": name }))
}

async fn modify_thread(
    State(s): State<Shared>,
    Path(tid): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut m = s.lock().unwrap();
    let add: Vec<String> = body["addLabelIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let remove: Vec<String> = body["removeLabelIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let mut refs = vec![];
    for msg in m.messages.values_mut().filter(|x| x.thread_id == tid) {
        for a in &add {
            msg.labels.insert(a.clone());
        }
        for r in &remove {
            msg.labels.remove(r);
        }
        refs.push(json!({ "message": { "id": msg.id, "threadId": tid } }));
    }
    // Like Gmail, our own modifications echo back through history.
    m.history_id += 1;
    let h = m.history_id;
    m.history
        .push((h, json!({ "id": h.to_string(), "labelsAdded": refs })));
    Json(json!({ "id": tid }))
}

async fn send(State(s): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let mut m = s.lock().unwrap();
    let raw = base64::engine::general_purpose::URL_SAFE
        .decode(body["raw"].as_str().unwrap_or(""))
        .unwrap_or_default();
    let raw = String::from_utf8_lossy(&raw).to_string();
    let tid = body["threadId"].as_str().map(String::from);
    m.sent.push((raw.clone(), tid.clone()));
    let id = format!("sent{}", m.sent.len());
    let thread = tid.clone().unwrap_or_else(|| id.clone());
    m.add_message(&id, &thread, &["SENT"], &raw, 4_000_000_000_000);
    Json(json!({ "id": id, "threadId": thread }))
}

/// Stub of Anthropic's Messages API: always returns the same digest.
async fn llm(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    Json(_body): Json<Value>,
) -> Json<Value> {
    let key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    s.lock().unwrap().llm_keys.push(key);
    let digest = json!({ "clean_body": "Cleaned", "card": { "name": "Marc Petit", "title": "CFO", "company": "Acme", "phones": ["+33 6 12 34 56 78"], "address": "", "website": "", "socials": [] } });
    Json(
        json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": digest.to_string() }] }),
    )
}

/// Serves the stub on an ephemeral port in a background runtime.
pub fn start(mailbox: Shared) -> u16 {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let app = Router::new()
                .route("/token", post(token))
                .route("/v1/messages", post(llm))
                .route("/gmail/profile", get(profile))
                .route("/gmail/messages", get(list))
                .route("/gmail/messages/send", post(send))
                .route("/gmail/messages/{id}", get(get_message))
                .route("/gmail/history", get(history))
                .route("/gmail/labels", get(labels).post(create_label))
                .route("/gmail/threads/{id}/modify", post(modify_thread))
                .with_state(mailbox);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap().port()).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    rx.recv().unwrap()
}
