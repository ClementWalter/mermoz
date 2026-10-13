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
    /// Exact bytes of each sent message, in the same order as `sent`.
    pub sent_raw: Vec<Vec<u8>>,
    /// Which endpoint each send used: "json" or "upload".
    pub sent_via: Vec<&'static str>,
    /// Nonce the stub OpenAI puts in the next ID token (set by the test from the authorize URL).
    pub oidc_nonce: String,
    /// Authorization headers seen by the stub Responses endpoint.
    pub responses_auth: Vec<String>,
    /// API keys seen by the stub Anthropic endpoint.
    pub llm_keys: Vec<String>,
    /// Message ids whose next fetch fails with 503, to exercise retries.
    pub fail_once: BTreeSet<String>,
    /// Authorization headers seen by the stub TypeSafe endpoint.
    pub jev_auth: Vec<String>,
    /// Feed judgments the stub Jev made.
    pub jev_feed_calls: usize,
    /// Card words the stub AI wrote.
    pub feed_text_calls: usize,
    /// Jev answers 429 (Retry-After 100 ms) to this many calls first.
    pub jev_rate_limited: usize,
    /// Jev answers 402 (no credits left) while set.
    pub jev_out_of_credits: bool,
    /// Authorization headers seen by the stub SubQ endpoint.
    pub subq_auth: Vec<String>,
    /// SubQ answers 402 (no credits left) while set.
    pub subq_out_of_credits: bool,
    /// (provider path, model) of every System One call, in order.
    pub system_one_models: Vec<(&'static str, String)>,
    /// How long each System One call takes, to exercise slow providers.
    pub system_one_delay_ms: u64,
    /// Folder descriptions of every routing call, in order.
    pub folder_criteria: Vec<Value>,
    /// Status the stub Jev picks; "archive" when unset.
    pub jev_status: Option<String>,
    /// Conversation questions the AI answered.
    pub grouping_asks: usize,
    /// How long the stub AI takes to answer, to exercise slow providers.
    pub llm_delay_ms: u64,
    /// Bodies of the streamed chat requests the stub AI received.
    pub chat_requests: Vec<Value>,
    /// The stub ChatGPT refuses requests carrying tools (web search) with a 400.
    pub reject_tools: bool,
    /// How many of the next stub Claude chat answers stop with `pause_turn`.
    pub pause_turns: usize,
    /// Space-separated scopes the stub token endpoint says were granted.
    pub granted_scope: Option<String>,
    /// Every call to the token endpoint, as (grant_type, client_id).
    pub token_requests: Vec<(String, String)>,
    /// The stub AI's answers to "Ask Mermoz to do", in order; the last one repeats.
    pub do_plans: Vec<Value>,
    /// Bodies of the "Ask Mermoz to do" requests the stub AI received.
    pub do_asks: Vec<Value>,
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
    /// A message Gmail has but sync never saw (no history entry): only Gmail's own
    /// search finds it.
    pub fn add_unseen(&mut self, id: &str, thread_id: &str, raw: &str, internal_date: i64) {
        self.messages.insert(
            id.into(),
            StubMessage {
                id: id.into(),
                thread_id: thread_id.into(),
                labels: ["INBOX".to_string()].into(),
                raw: raw.into(),
                internal_date,
            },
        );
    }

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

async fn token(
    State(s): State<Shared>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Json<Value> {
    let field = |k: &str| form.get(k).cloned().unwrap_or_default();
    s.lock()
        .unwrap()
        .token_requests
        .push((field("grant_type"), field("client_id")));
    let mut v = json!({ "access_token": "stub-access", "expires_in": 3600, "refresh_token": "stub-refresh", "token_type": "Bearer" });
    // Like Google: the scopes this consent granted, which a test sets per flow.
    if let Some(scope) = s.lock().unwrap().granted_scope.clone() {
        v["scope"] = json!(scope);
    }
    Json(v)
}

async fn profile(State(s): State<Shared>) -> Json<Value> {
    let m = s.lock().unwrap();
    Json(
        json!({ "emailAddress": m.email, "messagesTotal": m.messages.len(), "historyId": m.history_id.to_string() }),
    )
}

async fn list(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let m = s.lock().unwrap();
    // `q`, like Gmail's search box; here a plain case-insensitive match on the source.
    let want = q.get("q").map(|w| w.to_lowercase());
    let mut msgs: Vec<&StubMessage> = m
        .messages
        .values()
        .filter(|x| {
            want.as_ref()
                .is_none_or(|w| x.raw.to_lowercase().contains(w))
        })
        .collect();
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
    // Like Gmail's format=metadata: the message's headers, unfolded.
    if q.get("format").map(|f| f.as_str()) == Some("metadata") {
        let head = msg
            .raw
            .split("\r\n\r\n")
            .next()
            .unwrap_or("")
            .replace("\r\n ", " ")
            .replace("\r\n\t", " ");
        let headers: Vec<Value> = head
            .split("\r\n")
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| json!({ "name": k.trim(), "value": v.trim() }))
            .collect();
        v["payload"] = json!({ "headers": headers });
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

/// Stub Jev: files everything under "Receipts" when that folder is offered, to archive (or `jev_status`).
/// "Automated" is likely only for no-reply senders. "Same conversation" is likely,
/// unsure when the later subject says "Maybe", unlikely when it says "Unrelated";
/// other yes/no questions answer 0.96.
async fn jev_systemone(
    state: State<Shared>,
    headers: axum::http::HeaderMap,
    body: Json<Value>,
) -> axum::response::Response {
    system_one("jev", state, headers, body).await
}

/// Stub SubQ: the same API and answers as the stub Jev, on its own path.
async fn subq_systemone(
    state: State<Shared>,
    headers: axum::http::HeaderMap,
    body: Json<Value>,
) -> axum::response::Response {
    system_one("subq", state, headers, body).await
}

async fn system_one(
    which: &'static str,
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let delay = s.lock().unwrap().system_one_delay_ms;
    if delay > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    let status = {
        let mut m = s.lock().unwrap();
        let model = body["model"].as_str().unwrap_or("").to_string();
        m.system_one_models.push((which, model));
        let out_of_credits = if which == "subq" {
            m.subq_auth.push(auth);
            m.subq_out_of_credits
        } else {
            m.jev_auth.push(auth);
            m.jev_out_of_credits
        };
        if out_of_credits {
            return (
                axum::http::StatusCode::PAYMENT_REQUIRED,
                r#"{"error":{"type":"billing_error","message":"credit balance is too low"}}"#,
            )
                .into_response();
        }
        if m.jev_rate_limited > 0 {
            m.jev_rate_limited -= 1;
            return (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", "100")],
                "slow down",
            )
                .into_response();
        }
        m.jev_status.clone().unwrap_or("archive".into())
    };
    let state = body["state"].to_string();
    let later = body["state"]["later"]["subject"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let questions = &body["questions"];
    if let Some(c) = questions.get("project") {
        s.lock()
            .unwrap()
            .folder_criteria
            .push(c["criteria"].clone());
    }
    // Feed calls (they ask for a card kind) get answers shaped by the subject.
    let feed_kind = questions
        .get("kind")
        .map(|_| feed_kind(body["state"]["subject"].as_str().unwrap_or("")));
    s.lock().unwrap().jev_feed_calls += feed_kind.is_some() as usize;
    let mut answers = serde_json::Map::new();
    for (id, q) in questions.as_object().into_iter().flatten() {
        let first_option = || {
            q["criteria"]
                .as_object()
                .and_then(|c| c.keys().find(|k| *k != "none").cloned())
                .unwrap_or_else(|| "none".into())
        };
        let a = match (q["type"].as_str(), id.as_str(), feed_kind) {
            (_, "kind", Some(k)) => json!({ "type": "choice", "choice": k, "confidence": 0.9 }),
            (_, "automated", Some(k)) => {
                let p = if matches!(k, "reading" | "cleanup") {
                    0.96
                } else {
                    0.05
                };
                json!({ "type": "noul", "noul": p })
            }
            (_, "due" | "amount", Some(_)) => {
                json!({ "type": "choice", "choice": first_option(), "confidence": 0.9 })
            }
            (Some("noul"), "automated", _) => {
                json!({ "type": "noul", "noul": if state.contains("noreply") { 0.96 } else { 0.05 } })
            }
            (Some("noul"), "same", _) => {
                let p = if later.contains("Maybe") {
                    0.6
                } else if later.contains("Unrelated") {
                    0.1
                } else {
                    0.96
                };
                json!({ "type": "noul", "noul": p })
            }
            (Some("noul"), ..) => json!({ "type": "noul", "noul": 0.96 }),
            (_, "relation", _) => json!({ "type": "score", "score": 1.5, "confidence": 0.5 }),
            (Some("score"), ..) => json!({ "type": "score", "score": 2.0, "confidence": 0.8 }),
            (_, "project", _) if q["criteria"].get("Receipts").is_some() => {
                json!({ "type": "choice", "choice": "Receipts", "confidence": 0.92 })
            }
            (_, "project", _) => json!({ "type": "choice", "choice": "none", "confidence": 0.9 }),
            (_, "status", _) => json!({ "type": "choice", "choice": status, "confidence": 0.9 }),
            _ => json!({ "type": "choice", "choice": "none", "confidence": 0.8 }),
        };
        answers.insert(id.clone(), a);
    }
    Json(json!({ "answers": answers, "usage": { "input_tokens": 7500, "output_tokens": 900 } }))
        .into_response()
}

/// The card kind the stub Jev gives a demo thread, from words in its subject.
fn feed_kind(subject: &str) -> &'static str {
    const KINDS: &[(&str, &str)] = &[
        ("Carrelage", "decision"),
        ("Facture", "payment"),
        ("Réunion", "event"),
        ("Avenant", "sign"),
        ("Chiffres", "promise"),
        ("intro", "intro"),
        ("Brief", "reading"),
        ("Quartier", "reading"),
        ("digest", "reading"),
        ("reçu", "cleanup"),
        ("Commande", "cleanup"),
        ("statement", "cleanup"),
    ];
    KINDS
        .iter()
        .find(|(w, _)| subject.contains(w))
        .map_or("reply", |(_, k)| k)
}

/// Like Gmail, a POST without a Content-Length is refused with 411.
fn has_length(headers: &axum::http::HeaderMap) -> bool {
    headers.contains_key(axum::http::header::CONTENT_LENGTH)
}

async fn trash_thread(
    State(s): State<Shared>,
    Path(tid): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, axum::http::StatusCode> {
    if !has_length(&headers) {
        return Err(axum::http::StatusCode::LENGTH_REQUIRED);
    }
    s.lock()
        .unwrap()
        .relabel_thread(&tid, &["TRASH"], &["INBOX"]);
    Ok(Json(json!({ "id": tid })))
}

async fn untrash_thread(
    State(s): State<Shared>,
    Path(tid): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, axum::http::StatusCode> {
    if !has_length(&headers) {
        return Err(axum::http::StatusCode::LENGTH_REQUIRED);
    }
    s.lock().unwrap().relabel_thread(&tid, &[], &["TRASH"]);
    Ok(Json(json!({ "id": tid })))
}

/// Like Gmail: the label disappears from every message, the messages stay.
async fn delete_label(State(s): State<Shared>, Path(id): Path<String>) -> Json<Value> {
    let mut m = s.lock().unwrap();
    m.labels.remove(&id);
    for msg in m.messages.values_mut() {
        msg.labels.remove(&id);
    }
    Json(json!({}))
}

async fn send(State(s): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let raw = base64::engine::general_purpose::URL_SAFE
        .decode(body["raw"].as_str().unwrap_or(""))
        .unwrap_or_default();
    let tid = body["threadId"].as_str().map(String::from);
    Json(record_send(&s, raw, tid, "json"))
}

/// Gmail's upload endpoint: `multipart/related` with JSON metadata, then the message.
async fn send_upload(
    State(s): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> (axum::http::StatusCode, Json<Value>) {
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(boundary) = ct.split("boundary=").nth(1) else {
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({})));
    };
    if q.get("uploadType").map(String::as_str) != Some("multipart") {
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({})));
    }
    let sep = format!("--{boundary}");
    let text = body.to_vec();
    let find = |hay: &[u8], needle: &[u8], from: usize| {
        hay[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|i| i + from)
    };
    // Part 1: headers, blank line, JSON metadata.
    let p1 = find(&text, sep.as_bytes(), 0).unwrap();
    let meta_start = find(&text, b"\r\n\r\n", p1).unwrap() + 4;
    let p2 = find(&text, sep.as_bytes(), meta_start).unwrap();
    let meta: Value = serde_json::from_slice(&text[meta_start..p2 - 2]).unwrap_or(json!({}));
    // Part 2: headers, blank line, the raw message up to the closing boundary.
    let raw_start = find(&text, b"\r\n\r\n", p2).unwrap() + 4;
    let end = find(&text, format!("\r\n{sep}--").as_bytes(), raw_start).unwrap();
    let tid = meta["threadId"].as_str().map(String::from);
    (
        axum::http::StatusCode::OK,
        Json(record_send(
            &s,
            text[raw_start..end].to_vec(),
            tid,
            "upload",
        )),
    )
}

fn record_send(s: &Shared, raw_bytes: Vec<u8>, tid: Option<String>, via: &'static str) -> Value {
    let mut m = s.lock().unwrap();
    let raw = String::from_utf8_lossy(&raw_bytes).to_string();
    m.sent.push((raw.clone(), tid.clone()));
    m.sent_raw.push(raw_bytes);
    m.sent_via.push(via);
    let id = format!("sent{}", m.sent.len());
    let thread = tid.clone().unwrap_or_else(|| id.clone());
    m.add_message(&id, &thread, &["SENT"], &raw, 4_000_000_000_000);
    json!({ "id": id, "threadId": thread })
}

/// Stub of Anthropic's Messages API: always returns the same digest.
async fn llm(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    let delay = s.lock().unwrap().llm_delay_ms;
    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    // People clean-up (schema with "groups"): merge everything into one tidy person.
    // Conversation grouping: the later thread continues the earlier one unless it says otherwise.
    if body["output_config"]["format"]["schema"]["required"][0] == "same_conversation" {
        let input = body["messages"][0]["content"].as_str().unwrap_or("");
        let text =
            json!({ "same_conversation": !input.contains("Not the same"), "reason": "stub" })
                .to_string();
        s.lock().unwrap().grouping_asks += 1;
        return Json(
            json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": text }] }),
        );
    }
    if body["output_config"]["format"]["schema"]["required"][0] == "groups" {
        let input: Value =
            serde_json::from_str(body["messages"][0]["content"].as_str().unwrap_or("{}"))
                .unwrap_or_default();
        let ids: Vec<Value> = input["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| e["id"].clone())
            .collect();
        let text = json!({ "groups": [{ "ids": ids, "name": "Tidy Name", "kind": "person" }] })
            .to_string();
        return Json(
            json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": text }] }),
        );
    }
    // "Ask Mermoz to do" (schema starting with "understood"): the plan the test set.
    if body["output_config"]["format"]["schema"]["required"][0] == "understood" {
        let text = {
            let mut m = s.lock().unwrap();
            let n = m.do_asks.len();
            m.do_asks.push(body.clone());
            m.do_plans
                .get(n.min(m.do_plans.len().saturating_sub(1)))
                .cloned()
                .unwrap_or(json!({ "understood": false, "question": "No plan.", "steps": [] }))
                .to_string()
        };
        return Json(
            json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": text }] }),
        );
    }
    // Feed card words (schema starting with "title").
    if body["output_config"]["format"]["schema"]["required"][0] == "title" {
        s.lock().unwrap().feed_text_calls += 1;
        let text = json!({
            "title": "Choose the kitchen tiles",
            "why": "Marie needs your pick by Friday to order on time.",
            "draft": "",
            "options": [
                { "label": "Grès cérame gris", "detail": "42 €/m², 3 weeks", "recommended": true,
                  "reason": "Arrives sooner", "reply": "Bonjour Marie, on part sur le grès cérame gris." },
                { "label": "Terre cuite", "detail": "58 €/m², 6 weeks", "recommended": false,
                  "reason": "", "reply": "Bonjour Marie, on part sur la terre cuite." }
            ],
            "points": [],
            "summary": "Marie asks which floor tiles to order."
        })
        .to_string();
        return Json(
            json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": text }] }),
        );
    }
    let key = headers
        .get("x-api-key")
        .or_else(|| headers.get("authorization"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    s.lock().unwrap().llm_keys.push(key);
    let digest = json!({ "clean_body": "Cleaned", "card": { "name": "Marc Petit", "title": "CFO", "company": "Acme", "phones": ["+33 6 12 34 56 78"], "address": "", "website": "", "socials": [] } });
    Json(
        json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": digest.to_string() }] }),
    )
}

const TEST_KEY: &[u8] = include_bytes!("fixtures/test-oidc-key.pem");

fn oidc_key() -> jsonwebtoken::EncodingKey {
    jsonwebtoken::EncodingKey::from_rsa_pem(TEST_KEY).unwrap()
}

async fn oidc_config(headers: axum::http::HeaderMap) -> Json<Value> {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    Json(
        json!({ "jwks_uri": format!("http://{host}/oa/.well-known/jwks.json"), "revocation_endpoint": format!("http://{host}/oa/oauth/revoke") }),
    )
}

async fn jwks() -> Json<Value> {
    let mut jwk =
        jsonwebtoken::jwk::Jwk::from_encoding_key(&oidc_key(), jsonwebtoken::Algorithm::RS256)
            .unwrap();
    jwk.common.key_id = Some("test".into());
    Json(json!({ "keys": [jwk] }))
}

/// Stub of OpenAI's token endpoint: issues a signed ID token for the registered client.
async fn oa_token(
    State(s): State<Shared>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Json<Value> {
    let nonce = s.lock().unwrap().oidc_nonce.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test".into());
    let claims = json!({ "iss": "https://auth.openai.com", "aud": form.get("client_id").cloned().unwrap_or_default(),
        "sub": "user-123", "email": "me@example.com", "nonce": nonce, "iat": now, "exp": now + 3600 });
    let id_token = jsonwebtoken::encode(&header, &claims, &oidc_key()).unwrap();
    Json(
        json!({ "access_token": "chatgpt-access", "refresh_token": "chatgpt-refresh", "id_token": id_token, "expires_in": 3600,
        "scope": "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct", "token_type": "Bearer" }),
    )
}

async fn oa_models() -> Json<Value> {
    Json(
        json!({ "models": [ { "slug": "gpt-test-mini", "display_name": "GPT Test Mini", "visibility": "list" }, { "slug": "hidden", "display_name": "Hidden", "visibility": "hide" } ] }),
    )
}

/// Stub of the Responses API stream: one completed event carrying the digest JSON.
async fn oa_responses(State(s): State<Shared>, headers: axum::http::HeaderMap) -> String {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    s.lock().unwrap().responses_auth.push(auth);
    let digest = json!({ "clean_body": "Cleaned by ChatGPT", "card": null }).to_string();
    let ev = json!({ "type": "response.completed", "response": { "output": [ { "content": [ { "type": "output_text", "text": digest } ] } ] } });
    format!("event: response.completed\ndata: {ev}\n\n")
}

/// What the stub AI answers in a chat: advice, then an actions block split mid-tag across
/// pieces, with one proposal the daemon must drop (unknown status).
const CHAT_PIECES: &[&str] = &[
    "Marc wants your sign-off. ",
    "I'd reply today.\n\n<act",
    "ions>[{\"kind\":\"status\",\"status\":\"waiting\",\"label\":\"Move to Waiting\"},\
     {\"kind\":\"draft_reply\",\"label\":\"Draft a reply\",\"body\":\"Hi Marc, signed off.\"},\
     {\"kind\":\"status\",\"status\":\"someday\"}]</actions>",
];

/// Anthropic Messages API: streamed requests are chats, the rest structured calls.
async fn messages(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if body["stream"] != true {
        return llm(State(s), headers, Json(body)).await.into_response();
    }
    let searched = body["tools"][0]["name"] == "web_search";
    let pause = {
        let mut m = s.lock().unwrap();
        m.chat_requests.push(body);
        let pause = m.pause_turns > 0;
        m.pause_turns = m.pause_turns.saturating_sub(1);
        pause
    };
    let sse = |ev: Value| {
        format!(
            "event: {}\ndata: {ev}\n\n",
            ev["type"].as_str().unwrap_or("")
        )
    };
    let mut out = sse(json!({ "type": "message_start", "message": {} }));
    if pause {
        // A long research turn the server paused: thinking, one search, a first sentence.
        out += &sse(
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }),
        );
        out += &sse(
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "stub-sig" } }),
        );
        out += &sse(json!({ "type": "content_block_stop", "index": 0 }));
        out += &sse(
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "server_tool_use", "id": "srvtoolu_stub", "name": "web_search", "input": {} } }),
        );
        out += &sse(
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "{\"query\":\"Acme audit\"}" } }),
        );
        out += &sse(json!({ "type": "content_block_stop", "index": 1 }));
        out += &sse(
            json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "text", "text": "" } }),
        );
        out += &sse(
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "text_delta", "text": "Part one. " } }),
        );
        out += &sse(json!({ "type": "content_block_stop", "index": 2 }));
        out += &sse(json!({ "type": "message_delta", "delta": { "stop_reason": "pause_turn" } }));
        out += &sse(json!({ "type": "message_stop" }));
        return ([("content-type", "text/event-stream")], out).into_response();
    }
    if searched {
        out += &sse(
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "server_tool_use", "id": "srvtoolu_stub2", "name": "web_search", "input": {} } }),
        );
        out += &sse(json!({ "type": "content_block_stop", "index": 0 }));
    }
    out += &sse(
        json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
    );
    for p in CHAT_PIECES {
        out += &sse(
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": p } }),
        );
    }
    out += &sse(json!({ "type": "content_block_stop", "index": 1 }));
    out += &sse(json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }));
    out += &sse(json!({ "type": "message_stop" }));
    ([("content-type", "text/event-stream")], out).into_response()
}

/// Responses API: chats (no structured format) stream text; the rest go to the digest stub.
async fn oa_responses_any(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if body["text"]["format"].is_object() {
        return oa_responses(State(s), headers).await.into_response();
    }
    let reject = s.lock().unwrap().reject_tools && body.get("tools").is_some();
    s.lock().unwrap().chat_requests.push(body);
    if reject {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "{\"error\":{\"message\":\"tools are not supported\"}}",
        )
            .into_response();
    }
    let mut out = String::new();
    for p in CHAT_PIECES {
        let ev = json!({ "type": "response.output_text.delta", "delta": p });
        out.push_str(&format!(
            "event: response.output_text.delta\ndata: {ev}\n\n"
        ));
    }
    out.push_str(
        "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{}}\n\n",
    );
    ([("content-type", "text/event-stream")], out).into_response()
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
                .route("/v1/messages", post(messages))
                .route("/oa/.well-known/openid-configuration", get(oidc_config))
                .route("/oa/.well-known/jwks.json", get(jwks))
                .route("/oa/api/accounts/oauth/token", post(oa_token))
                .route("/oa/v1/models", get(oa_models))
                .route("/oa/v1/responses", post(oa_responses_any))
                .route("/gmail/profile", get(profile))
                .route("/gmail/messages", get(list))
                .route("/gmail/messages/send", post(send))
                .route("/gmail/upload/messages/send", post(send_upload))
                .route("/gmail/messages/{id}", get(get_message))
                .route("/gmail/history", get(history))
                .route("/gmail/labels", get(labels).post(create_label))
                .route("/gmail/threads/{id}/modify", post(modify_thread))
                .route("/gmail/threads/{id}/trash", post(trash_thread))
                .route("/jev/v1/systemone", post(jev_systemone))
                .route("/subq/v1/systemone", post(subq_systemone))
                .route("/gmail/threads/{id}/untrash", post(untrash_thread))
                .route("/gmail/labels/{id}", axum::routing::delete(delete_label))
                .with_state(mailbox);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap().port()).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    rx.recv().unwrap()
}
