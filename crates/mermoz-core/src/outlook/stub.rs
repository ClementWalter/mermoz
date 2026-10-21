//! In-process stub of the Microsoft Graph mail API and Microsoft's token endpoint, with
//! enough behaviour (folders, delta links, moves, categories, send, throttling) to
//! drive an Outlook account end to end. Used by unit tests and the daemon's e2e tests.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct StubFolder {
    pub id: String,
    pub name: String,
    pub parent: String,
    /// Graph's well-known name, when it has one.
    pub well_known: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StubMessage {
    pub id: String,
    pub conversation_id: String,
    pub folder: String,
    pub is_read: bool,
    pub categories: Vec<String>,
    pub flagged: bool,
    pub received_ms: i64,
    pub mime: String,
}

/// One change, as a delta walks them: `from` is the folder the message left (moves
/// and deletes), `None` for a new or changed message.
#[derive(Clone, Debug)]
struct Change {
    seq: u64,
    id: String,
    from: Option<String>,
}

#[derive(Debug, Default)]
pub struct Graph {
    /// Base URL of this stub's Graph routes, for the links it hands out.
    pub base: String,
    pub email: String,
    pub folders: Vec<StubFolder>,
    pub messages: BTreeMap<String, StubMessage>,
    pub categories: Vec<String>,
    changes: Vec<Change>,
    seq: u64,
    /// Messages a delta returns per page.
    pub page_size: usize,
    /// Every delta link handed out is refused as expired (410) while set.
    pub expire_deltas: bool,
    /// The next this many requests are throttled (429, `Retry-After: 0`).
    pub throttle: usize,
    /// Throttled answers given.
    pub throttled: usize,
    /// MIME bytes of each sent message, decoded.
    pub sent: Vec<String>,
    /// `PATCH /me/messages/{id}` bodies, in order.
    pub patches: Vec<(String, Value)>,
    /// `(message, destination)` of each move, in order.
    pub moves: Vec<(String, String)>,
    /// `Prefer` headers seen, in order.
    pub prefer: Vec<String>,
    /// Every `$value` fetch, by message id.
    pub mime_gets: Vec<String>,
    /// Every token request's form.
    pub token_forms: Vec<HashMap<String, String>>,
    /// Bearer tokens seen on Graph calls.
    pub bearers: BTreeSet<String>,
    next_id: u64,
}

pub type Shared = Arc<Mutex<Graph>>;

/// Default folders of a new mailbox: (id, display name, well-known name).
const DEFAULT_FOLDERS: &[(&str, &str, &str)] = &[
    ("f-inbox", "Inbox", "inbox"),
    ("f-sent", "Sent Items", "sentitems"),
    ("f-deleted", "Deleted Items", "deleteditems"),
    ("f-junk", "Junk Email", "junkemail"),
    ("f-drafts", "Drafts", "drafts"),
    ("f-archive", "Archive", "archive"),
    ("f-outbox", "Outbox", "outbox"),
];

impl Graph {
    /// A mailbox with the default folders.
    pub fn new(email: &str) -> Self {
        Self {
            email: email.into(),
            folders: DEFAULT_FOLDERS
                .iter()
                .map(|(id, name, wk)| StubFolder {
                    id: id.to_string(),
                    name: name.to_string(),
                    parent: "root".into(),
                    well_known: Some(wk.to_string()),
                })
                .collect(),
            page_size: 2,
            ..Default::default()
        }
    }

    fn record(&mut self, id: &str, from: Option<String>) {
        self.seq += 1;
        self.changes.push(Change {
            seq: self.seq,
            id: id.into(),
            from,
        });
    }

    pub fn add_folder(&mut self, id: &str, name: &str, parent: &str) {
        self.folders.push(StubFolder {
            id: id.into(),
            name: name.into(),
            parent: parent.into(),
            well_known: None,
        });
    }

    /// Delivers a message into a folder.
    pub fn add_message(
        &mut self,
        id: &str,
        conversation_id: &str,
        folder: &str,
        is_read: bool,
        mime: &str,
        received_ms: i64,
    ) {
        self.messages.insert(
            id.into(),
            StubMessage {
                id: id.into(),
                conversation_id: conversation_id.into(),
                folder: folder.into(),
                is_read,
                categories: vec![],
                flagged: false,
                received_ms,
                mime: mime.into(),
            },
        );
        self.record(id, None);
    }

    /// A move made elsewhere (webmail, phone), or by a `move` call.
    pub fn move_message(&mut self, id: &str, folder: &str) {
        let Some(m) = self.messages.get_mut(id) else {
            return;
        };
        let from = std::mem::replace(&mut m.folder, folder.into());
        self.record(id, Some(from));
    }

    /// A change of read state, categories or flag made elsewhere, or by a `PATCH`.
    pub fn edit(&mut self, id: &str, f: impl FnOnce(&mut StubMessage)) {
        let Some(m) = self.messages.get_mut(id) else {
            return;
        };
        f(m);
        self.record(id, None);
    }

    /// A message deleted for good.
    pub fn delete(&mut self, id: &str) {
        if let Some(m) = self.messages.remove(id) {
            self.record(id, Some(m.folder));
        }
    }

    fn folder_id(&self, key: &str) -> Option<String> {
        self.folders
            .iter()
            .find(|f| f.id == key || f.well_known.as_deref() == Some(key))
            .map(|f| f.id.clone())
    }

    fn folder_json(&self, f: &StubFolder) -> Value {
        json!({
            "id": f.id,
            "displayName": f.name,
            "parentFolderId": f.parent,
            "childFolderCount": self.folders.iter().filter(|c| c.parent == f.id).count(),
            "totalItemCount": self.messages.values().filter(|m| m.folder == f.id).count(),
        })
    }

    fn meta(&self, m: &StubMessage) -> Value {
        let received = chrono::DateTime::from_timestamp_millis(m.received_ms)
            .unwrap_or_default()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        json!({
            "id": m.id,
            "conversationId": m.conversation_id,
            "parentFolderId": m.folder,
            "isRead": m.is_read,
            "categories": m.categories,
            "flag": { "flagStatus": if m.flagged { "flagged" } else { "notFlagged" } },
            "receivedDateTime": received,
        })
    }

    /// The folder's delta since `since`: the messages in it now that changed (all of
    /// them from the start), and `@removed` for those that left it.
    fn delta_items(&self, folder: &str, since: Option<u64>, upto: u64) -> Vec<Value> {
        let Some(since) = since else {
            let mut ms: Vec<&StubMessage> = self
                .messages
                .values()
                .filter(|m| m.folder == folder)
                .collect();
            ms.sort_by_key(|m| std::cmp::Reverse(m.received_ms));
            return ms.into_iter().map(|m| self.meta(m)).collect();
        };
        let mut seen = BTreeSet::new();
        let mut out = vec![];
        for c in self
            .changes
            .iter()
            .filter(|c| c.seq > since && c.seq <= upto)
        {
            if !seen.insert(c.id.clone()) {
                continue;
            }
            match self.messages.get(&c.id) {
                Some(m) if m.folder == folder => out.push(self.meta(m)),
                _ => {
                    let left = self.changes.iter().any(|x| {
                        x.id == c.id
                            && x.seq > since
                            && x.seq <= upto
                            && x.from.as_deref() == Some(folder)
                    });
                    if left {
                        out.push(json!({ "id": c.id, "@removed": { "reason": "deleted" } }));
                    }
                }
            }
        }
        out
    }

    /// Sends a message: a copy lands in Sent Items, in the conversation of the message
    /// it answers (by `In-Reply-To`), as Exchange threads replies.
    fn send(&mut self, mime: String) {
        let header = |name: &str| {
            mime.split("\r\n\r\n")
                .next()
                .unwrap_or_default()
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case(name)
                        .then(|| v.trim().to_string())
                })
        };
        let parent = header("In-Reply-To").and_then(|irt| {
            self.messages
                .values()
                .find(|m| m.mime.contains(&format!("Message-ID: {irt}")))
                .map(|m| m.conversation_id.clone())
        });
        self.next_id += 1;
        let id = format!("sent-{}", self.next_id);
        let conv = parent.unwrap_or_else(|| format!("conv-{id}"));
        let now = chrono::Utc::now().timestamp_millis();
        self.add_message(&id, &conv, "f-sent", true, &mime, now);
        self.sent.push(mime);
    }
}

fn throttled(g: &mut Graph) -> Option<Response> {
    if g.throttle == 0 {
        return None;
    }
    g.throttle -= 1;
    g.throttled += 1;
    Some(
        (
            StatusCode::TOO_MANY_REQUESTS,
            [("Retry-After", "0")],
            "throttled",
        )
            .into_response(),
    )
}

/// Common checks: bearer token, `Prefer` header, throttling.
fn enter(g: &mut Graph, headers: &HeaderMap) -> Option<Response> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let Some(token) = auth.strip_prefix("Bearer ") else {
        return Some(StatusCode::UNAUTHORIZED.into_response());
    };
    g.bearers.insert(token.to_string());
    if let Some(p) = headers.get("prefer").and_then(|v| v.to_str().ok()) {
        g.prefer.push(p.to_string());
    }
    throttled(g)
}

async fn token(
    State(s): State<Shared>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Json<Value> {
    let mut g = s.lock().unwrap();
    g.token_forms.push(form);
    let n = g.token_forms.len();
    Json(json!({
        "access_token": format!("graph-access-{n}"),
        "expires_in": 3600,
        "refresh_token": format!("graph-refresh-{n}"),
        "token_type": "Bearer",
        "scope": crate::oauth::MICROSOFT_SCOPES,
    }))
}

async fn me(State(s): State<Shared>, headers: HeaderMap) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    Json(json!({ "mail": null, "userPrincipalName": g.email })).into_response()
}

async fn top_folders(State(s): State<Shared>, headers: HeaderMap) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    let v: Vec<Value> = g
        .folders
        .iter()
        .filter(|f| f.parent == "root")
        .map(|f| g.folder_json(f))
        .collect();
    Json(json!({ "value": v })).into_response()
}

async fn folder(State(s): State<Shared>, Path(key): Path<String>, headers: HeaderMap) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    match g.folder_id(&key) {
        Some(id) => {
            let f = g.folders.iter().find(|f| f.id == id).unwrap().clone();
            Json(g.folder_json(&f)).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn patch_folder(
    State(s): State<Shared>,
    Path(key): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    let Some(f) = g.folders.iter_mut().find(|f| f.id == key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if let Some(n) = body["displayName"].as_str() {
        f.name = n.into();
    }
    Json(json!({})).into_response()
}

async fn delete_folder(
    State(s): State<Shared>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    let ids: Vec<String> = g
        .messages
        .values()
        .filter(|m| m.folder == key)
        .map(|m| m.id.clone())
        .collect();
    for id in ids {
        g.move_message(&id, "f-deleted");
    }
    g.folders.retain(|f| f.id != key);
    StatusCode::NO_CONTENT.into_response()
}

async fn child_folders(
    State(s): State<Shared>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    let Some(id) = g.folder_id(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let v: Vec<Value> = g
        .folders
        .iter()
        .filter(|f| f.parent == id)
        .map(|f| g.folder_json(f))
        .collect();
    Json(json!({ "value": v })).into_response()
}

fn num(q: &HashMap<String, String>, k: &str) -> Option<usize> {
    q.get(k).and_then(|v| v.parse().ok())
}

/// Newest first, every folder (`/me/messages`) or one folder.
fn listing(g: &Graph, folder: Option<&str>, q: &HashMap<String, String>, path: &str) -> Value {
    let mut ms: Vec<&StubMessage> = g
        .messages
        .values()
        .filter(|m| folder.is_none_or(|f| m.folder == f))
        .collect();
    ms.sort_by_key(|m| std::cmp::Reverse(m.received_ms));
    let top = num(q, "$top").unwrap_or(10);
    let skip = num(q, "$skip").unwrap_or(0);
    let page: Vec<Value> = ms.iter().skip(skip).take(top).map(|m| g.meta(m)).collect();
    let mut v = json!({ "value": page });
    if skip + top < ms.len() {
        v["@odata.nextLink"] = json!(format!(
            "{}{path}?$select=id&$top={top}&$skip={}",
            g.base,
            skip + top
        ));
    }
    v
}

async fn messages(
    State(s): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    Json(listing(&g, None, &q, "/me/messages")).into_response()
}

async fn folder_messages(
    State(s): State<Shared>,
    Path(key): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    let Some(id) = g.folder_id(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = format!("/me/mailFolders/{id}/messages");
    Json(listing(&g, Some(&id), &q, &path)).into_response()
}

async fn delta(
    State(s): State<Shared>,
    Path(key): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    let Some(folder) = g.folder_id(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let since = q.get("since").and_then(|v| v.parse::<u64>().ok());
    if since.is_some() && g.expire_deltas {
        return (StatusCode::GONE, "syncStateNotFound").into_response();
    }
    let upto = q.get("upto").and_then(|v| v.parse().ok()).unwrap_or(g.seq);
    let skip = num(&q, "skip").unwrap_or(0);
    let items = g.delta_items(&folder, since, upto);
    let page: Vec<Value> = items.iter().skip(skip).take(g.page_size).cloned().collect();
    let link = |extra: String| format!("{}/me/mailFolders/{folder}/messages/delta?{extra}", g.base);
    let since_q = since.map(|s| format!("since={s}&")).unwrap_or_default();
    let mut v = json!({ "value": page });
    if skip + g.page_size < items.len() {
        v["@odata.nextLink"] = json!(link(format!(
            "{since_q}upto={upto}&skip={}",
            skip + g.page_size
        )));
    } else {
        v["@odata.deltaLink"] = json!(link(format!("since={upto}")));
    }
    Json(v).into_response()
}

async fn message(State(s): State<Shared>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    match g.messages.get(&id) {
        Some(m) => Json(g.meta(m)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn mime_of(g: &mut Graph, id: &str) -> Response {
    g.mime_gets.push(id.to_string());
    match g.messages.get(id) {
        Some(m) => m.mime.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn message_value(
    State(s): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    mime_of(&mut g, &id)
}

async fn patch_message(
    State(s): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    if !g.messages.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    g.patches.push((id.clone(), body.clone()));
    g.edit(&id, |m| {
        if let Some(r) = body["isRead"].as_bool() {
            m.is_read = r;
        }
        if let Some(c) = body["categories"].as_array() {
            m.categories = c
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
        }
        if let Some(f) = body["flag"]["flagStatus"].as_str() {
            m.flagged = f == "flagged";
        }
    });
    let m = g.messages[&id].clone();
    Json(g.meta(&m)).into_response()
}

async fn move_message(
    State(s): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    let dest = body["destinationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let Some(dest) = g.folder_id(&dest) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !g.messages.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    g.moves.push((id.clone(), dest.clone()));
    g.move_message(&id, &dest);
    let m = g.messages[&id].clone();
    (StatusCode::CREATED, Json(g.meta(&m))).into_response()
}

async fn send_mail(State(s): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    let is_text = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/plain"));
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(body.trim()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !is_text {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    g.send(String::from_utf8_lossy(&bytes).into_owned());
    StatusCode::ACCEPTED.into_response()
}

async fn master_categories(State(s): State<Shared>, headers: HeaderMap) -> Response {
    let mut g = s.lock().unwrap();
    if let Some(r) = enter(&mut g, &headers) {
        return r;
    }
    let v: Vec<Value> = g
        .categories
        .iter()
        .map(|c| json!({ "id": c, "displayName": c, "color": "preset0" }))
        .collect();
    Json(json!({ "value": v })).into_response()
}

/// The routes, under `/graph` (Graph) and `/ms` (sign-in), sharing `graph`.
pub fn router(graph: Shared) -> Router {
    Router::new()
        .route("/ms/token", post(token))
        .route("/graph/me", get(me))
        .route("/graph/me/mailFolders", get(top_folders))
        .route(
            "/graph/me/mailFolders/{id}",
            get(folder).patch(patch_folder).delete(delete_folder),
        )
        .route(
            "/graph/me/mailFolders/{id}/childFolders",
            get(child_folders),
        )
        .route("/graph/me/mailFolders/{id}/messages", get(folder_messages))
        .route("/graph/me/mailFolders/{id}/messages/delta", get(delta))
        .route("/graph/me/messages", get(messages))
        .route("/graph/me/messages/{id}", get(message).patch(patch_message))
        .route("/graph/me/messages/{id}/$value", get(message_value))
        .route("/graph/me/messages/{id}/move", post(move_message))
        .route("/graph/me/sendMail", post(send_mail))
        .route("/graph/me/outlook/masterCategories", get(master_categories))
        .with_state(graph)
}

/// Serves the stub on a free local port, on its own thread; returns the port. The
/// mailbox learns its base URL (`http://127.0.0.1:<port>/graph`).
pub fn start(graph: Shared) -> u16 {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            graph.lock().unwrap().base = format!("http://127.0.0.1:{port}/graph");
            tx.send(port).unwrap();
            axum::serve(listener, router(graph)).await.unwrap();
        });
    });
    rx.recv().unwrap()
}
