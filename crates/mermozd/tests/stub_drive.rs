//! In-process stub of the Google Drive v3 REST API (files, export, folders, resumable
//! uploads, permissions, thumbnails), with Drive's two access levels: everything the
//! user can open (`drive.readonly`) or only the files Mermoz made or the user picked
//! (`drive.file`).

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

pub const FOLDER: &str = "application/vnd.google-apps.folder";
pub const DOC: &str = "application/vnd.google-apps.document";
pub const SHEET: &str = "application/vnd.google-apps.spreadsheet";

#[derive(Clone, Debug, Default)]
pub struct StubFile {
    pub name: String,
    pub mime: String,
    pub modified: String,
    pub owner: String,
    pub writers: Vec<String>,
    pub readers: Vec<String>,
    pub parents: Vec<String>,
    pub content: Vec<u8>,
    pub thumbnail: bool,
}

#[derive(Default, Debug)]
pub struct DriveBox {
    /// The Google account the tokens belong to.
    pub email: String,
    pub files: BTreeMap<String, StubFile>,
    /// Files Mermoz created or the user picked: all `drive.file` reaches.
    pub app_files: BTreeSet<String>,
    /// The token reaches every file the user can open (`drive.readonly` granted).
    pub read_all: bool,
    /// (file id, email, role) of every permission created.
    pub permissions: Vec<(String, String, String)>,
    /// (file id, mime) of every export.
    pub exports: Vec<(String, String)>,
    /// Upload sessions: id -> (name, parents).
    sessions: HashMap<String, (String, Vec<String>)>,
    pub uploads: usize,
    pub port: u16,
    next: u64,
}

pub type SharedDrive = Arc<Mutex<DriveBox>>;

impl DriveBox {
    pub fn add_file(&mut self, id: &str, f: StubFile) {
        self.files.insert(id.into(), f);
    }

    fn visible(&self, id: &str) -> bool {
        self.read_all || self.app_files.contains(id)
    }

    fn new_id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{:020}", self.next)
    }

    fn meta(&self, id: &str, f: &StubFile) -> Value {
        let mut v = json!({
            "id": id, "name": f.name, "mimeType": f.mime, "modifiedTime": f.modified,
            "webViewLink": format!("https://drive.example/{id}"),
            "iconLink": "https://drive.example/icon.png",
            "owners": [{ "displayName": f.owner.split('@').next().unwrap_or(""), "emailAddress": f.owner }],
        });
        if f.thumbnail {
            v["thumbnailLink"] = json!(format!("http://127.0.0.1:{}/drive/thumb/{id}", self.port));
        }
        v
    }

    /// Folders by name under a parent, as Mermoz looks them up.
    pub fn folders_named(&self, name: &str) -> Vec<(String, StubFile)> {
        self.files
            .iter()
            .filter(|(_, f)| f.mime == FOLDER && f.name == name)
            .map(|(id, f)| (id.clone(), f.clone()))
            .collect()
    }
}

/// Values in single quotes of a Drive query, unescaped, in order.
fn quoted(q: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur: Option<String> = None;
    let mut chars = q.chars();
    while let Some(c) = chars.next() {
        match (&mut cur, c) {
            (Some(s), '\\') => {
                if let Some(n) = chars.next() {
                    s.push(n);
                }
            }
            (Some(_), '\'') => out.push(cur.take().unwrap()),
            (Some(s), c) => s.push(c),
            (None, '\'') => cur = Some(String::new()),
            (None, _) => {}
        }
    }
    out
}

async fn about(State(s): State<SharedDrive>) -> Json<Value> {
    Json(json!({ "user": { "emailAddress": s.lock().unwrap().email } }))
}

async fn get_file(
    State(s): State<SharedDrive>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let m = s.lock().unwrap();
    let Some(f) = m.files.get(&id).filter(|_| m.visible(&id)) else {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    };
    if q.get("alt").map(String::as_str) == Some("media") {
        return f.content.clone().into_response();
    }
    Json(m.meta(&id, f)).into_response()
}

async fn export(
    State(s): State<SharedDrive>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let mut m = s.lock().unwrap();
    let Some(f) = m.files.get(&id).filter(|_| m.visible(&id)).cloned() else {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    };
    m.exports
        .push((id, q.get("mimeType").cloned().unwrap_or_default()));
    f.content.into_response()
}

async fn list(
    State(s): State<SharedDrive>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let m = s.lock().unwrap();
    let query = q.get("q").cloned().unwrap_or_default();
    let words = quoted(&query);
    let size: usize = q
        .get("pageSize")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let mut hits: Vec<(&String, &StubFile)> = m
        .files
        .iter()
        .filter(|(id, _)| m.visible(id))
        .filter(|(_, f)| {
            if query.contains(" in parents") {
                // name = 'x' and mimeType = 'folder' and 'parent' in parents
                let parent = &words[2];
                let in_parent =
                    f.parents.contains(parent) || (parent == "root" && f.parents.is_empty());
                f.name == words[0] && f.mime == FOLDER && in_parent
            } else if query.contains(" in owners") {
                f.mime != FOLDER
                    && words
                        .iter()
                        .any(|e| f.owner == *e || f.writers.contains(e) || f.readers.contains(e))
            } else if query.contains("contains") {
                let t = words[0].to_lowercase();
                f.mime != FOLDER
                    && (f.name.to_lowercase().contains(&t)
                        || String::from_utf8_lossy(&f.content)
                            .to_lowercase()
                            .contains(&t))
            } else {
                f.mime != FOLDER
            }
        })
        .collect();
    hits.sort_by(|a, b| b.1.modified.cmp(&a.1.modified));
    let files: Vec<Value> = hits
        .into_iter()
        .take(size)
        .map(|(id, f)| m.meta(id, f))
        .collect();
    Json(json!({ "files": files }))
}

async fn create(State(s): State<SharedDrive>, Json(body): Json<Value>) -> Response {
    let mut m = s.lock().unwrap();
    let parents: Vec<String> = body["parents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str())
        .filter(|p| *p != "root")
        .map(String::from)
        .collect();
    // Drive refuses a parent it doesn't know (deleted, or out of this token's reach).
    if let Some(p) = parents
        .iter()
        .find(|p| !(m.files.contains_key(*p) && m.visible(p)))
    {
        return (StatusCode::NOT_FOUND, format!("File not found: {p}")).into_response();
    }
    let id = m.new_id("fold");
    let email = m.email.clone();
    m.add_file(
        &id,
        StubFile {
            name: body["name"].as_str().unwrap_or("").into(),
            mime: body["mimeType"].as_str().unwrap_or("").into(),
            modified: "2026-10-12T09:00:00Z".into(),
            owner: email,
            parents,
            ..Default::default()
        },
    );
    m.app_files.insert(id.clone());
    Json(json!({ "id": id })).into_response()
}

/// Opens a resumable upload session, as Drive does: the URI comes back in Location.
async fn start_upload(State(s): State<SharedDrive>, Json(body): Json<Value>) -> Response {
    let mut m = s.lock().unwrap();
    let parents: Vec<String> = body["parents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().map(String::from))
        .collect();
    // Drive refuses a parent it doesn't know (or this token can't reach).
    if let Some(p) = parents
        .iter()
        .find(|p| *p != "root" && !(m.files.contains_key(*p) && m.visible(p)))
    {
        return (StatusCode::NOT_FOUND, format!("File not found: {p}")).into_response();
    }
    let sid = m.new_id("sess");
    m.sessions.insert(
        sid.clone(),
        (body["name"].as_str().unwrap_or("").into(), parents),
    );
    let loc = format!("http://127.0.0.1:{}/upload/session/{sid}", m.port);
    (StatusCode::OK, [("location", loc)]).into_response()
}

async fn finish_upload(
    State(s): State<SharedDrive>,
    Path(sid): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut m = s.lock().unwrap();
    let Some((name, parents)) = m.sessions.remove(&sid) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let id = m.new_id("file");
    let email = m.email.clone();
    let f = StubFile {
        name,
        mime: headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .into(),
        modified: "2026-10-12T09:30:00Z".into(),
        owner: email,
        parents,
        content: body.to_vec(),
        thumbnail: false,
        ..Default::default()
    };
    let meta = m.meta(&id, &f);
    m.add_file(&id, f);
    m.app_files.insert(id);
    m.uploads += 1;
    Json(meta).into_response()
}

/// Only files reachable through `drive.file` may have their sharing changed.
async fn permit(
    State(s): State<SharedDrive>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut m = s.lock().unwrap();
    if !m.app_files.contains(&id) {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    }
    let email = body["emailAddress"].as_str().unwrap_or("").to_string();
    let role = body["role"].as_str().unwrap_or("").to_string();
    m.permissions.push((id, email, role));
    Json(json!({ "id": "perm" })).into_response()
}

async fn thumb(Path(_id): Path<String>) -> Response {
    ([("content-type", "image/png")], b"\x89PNG-stub".to_vec()).into_response()
}

/// Serves the stub on an ephemeral port in a background runtime.
pub fn start(state: SharedDrive) -> u16 {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            state.lock().unwrap().port = port;
            let app = Router::new()
                .route("/drive/about", get(about))
                .route("/drive/files", get(list).post(create))
                .route("/drive/files/{id}", get(get_file))
                .route("/drive/files/{id}/export", get(export))
                .route("/drive/files/{id}/permissions", post(permit))
                .route("/drive/thumb/{id}", get(thumb))
                .route("/upload/files", post(start_upload))
                .route("/upload/session/{sid}", put(finish_upload))
                .with_state(state);
            tx.send(port).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    rx.recv().unwrap()
}
