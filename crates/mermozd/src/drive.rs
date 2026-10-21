//! Google Drive on the daemon side: connecting an account's Drive (a separate consent
//! from Gmail), previews of the files a thread links to, the files shared with its
//! people, saving attachments into Drive, picking files for a reply and sharing them.
//! See the `drive.*` methods in `docs/protocol.md`.

use crate::rpc::{Caller, R, RpcError, app_only, opt_s, s, str_list, to_value};
use crate::service::Service;
use base64::Engine;
use mermoz_core::chat::LinkedDoc;
use mermoz_core::drive::{self, Drive, DriveLink, FileMeta, SCOPE_FILE, SCOPE_READ};
use mermoz_core::drive_cache::{CachedFile, DEFAULT_CAP_BYTES, DriveCache, Saved};
use mermoz_core::model::{new_id, now_ms};
use mermoz_core::oauth::{self, AuthRequest, ClientConfig, Granted, TokenSource};
use mermoz_core::sync::Event;
use mermoz_core::tokens::{self, DriveGrant};
use mermoz_core::{Error, Result, paths};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

/// The account has no Drive connection: the app shows "Connect Google Drive".
pub const DRIVE_NOT_CONNECTED: i64 = -32030;
/// Drive is connected but this needs more: `data.reason` is "read_all" (the user
/// unticked "see all your files"), "save" (unticked "files you use with Mermoz") or
/// "pick" (the file must be chosen in Google's file picker first).
pub const DRIVE_NEEDS_ACCESS: i64 = -32031;

/// File details are asked again after this long, so renames and edits show up.
const META_TTL_MS: i64 = 10 * 60 * 1000;
/// How long "Chat about this" waits for linked documents before answering without them.
const CHAT_DOCS_WAIT: Duration = Duration::from_secs(8);
/// Browser consent flows give up after this long.
const CONSENT_WAIT: Duration = Duration::from_secs(600);

/// The last "Connect Google Drive" attempt of an account, as Settings shows it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Attempt {
    /// "waiting" (browser open), "connected", "declined" or "error".
    pub state: String,
    pub error: Option<String>,
}

/// A file pick in Google's picker, polled by the app until it is done.
#[derive(Debug, Clone, Serialize)]
pub struct Pick {
    pub account_id: String,
    /// "waiting", "done", "declined" or "error".
    pub state: String,
    pub files: Vec<Value>,
    pub error: Option<String>,
}

pub struct Conn {
    pub grant: DriveGrant,
    pub drive: Drive,
}

#[derive(Default)]
pub struct Hub {
    cache: Mutex<Option<Arc<DriveCache>>>,
    conns: Mutex<HashMap<String, Arc<Conn>>>,
    attempts: Mutex<HashMap<String, Attempt>>,
    /// The consent in flight per account; a retry aborts it.
    tasks: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    picks: Mutex<HashMap<String, Pick>>,
}

fn not_connected(account_id: &str) -> RpcError {
    RpcError {
        code: DRIVE_NOT_CONNECTED,
        message: "Connect Google Drive for this account first.".into(),
        data: Some(json!({ "account_id": account_id })),
    }
}

fn needs_access(account_id: &str, reason: &str, message: &str) -> RpcError {
    RpcError {
        code: DRIVE_NEEDS_ACCESS,
        message: message.into(),
        data: Some(json!({ "account_id": account_id, "reason": reason })),
    }
}

/// A refresh token Google no longer honours (access removed in the Google account).
fn revoked(e: &Error) -> bool {
    match e {
        Error::SignInExpired => true,
        Error::Upstream { status, body } => mermoz_core::oauth::refused_grant(*status, body),
        _ => false,
    }
}

fn no_access(e: &Error) -> bool {
    matches!(
        e,
        Error::Upstream {
            status: 403 | 404,
            ..
        }
    )
}

impl Hub {
    fn cache(&self, svc: &Service) -> Result<Arc<DriveCache>> {
        let mut c = self.cache.lock().unwrap();
        if let Some(c) = c.as_ref() {
            return Ok(c.clone());
        }
        let cap = svc
            .store
            .setting("drive_cache_mb")?
            .and_then(|v| v.parse::<i64>().ok())
            .map(|mb| mb * 1024 * 1024)
            .unwrap_or(DEFAULT_CAP_BYTES);
        let opened = Arc::new(DriveCache::open(&paths::drive_cache_path(), cap)?);
        *c = Some(opened.clone());
        Ok(opened)
    }

    /// The account's Drive connection, `None` when Drive was never connected.
    fn conn(&self, svc: &Service, account_id: &str) -> Result<Option<Arc<Conn>>> {
        if let Some(c) = self.conns.lock().unwrap().get(account_id) {
            return Ok(Some(c.clone()));
        }
        let email = svc.store.account(account_id)?.email;
        let grant = match tokens::load_drive_grant(&email) {
            Ok(g) => g,
            Err(Error::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let ts = Arc::new(TokenSource::new(
            grant.client(),
            grant.refresh_token.clone(),
        ));
        let conn = Arc::new(Conn {
            grant,
            drive: Drive::new(ts),
        });
        self.conns
            .lock()
            .unwrap()
            .insert(account_id.to_string(), conn.clone());
        Ok(Some(conn))
    }

    fn require(&self, svc: &Service, account_id: &str) -> std::result::Result<Arc<Conn>, RpcError> {
        self.conn(svc, account_id)?
            .ok_or_else(|| not_connected(account_id))
    }

    fn forget(&self, svc: &Service, account_id: &str) {
        self.conns.lock().unwrap().remove(account_id);
        if let Ok(c) = self.cache(svc) {
            let _ = c.forget_account(account_id);
        }
    }
}

fn changed(svc: &Service, account_id: &str) {
    let _ = svc.events.send(Event::DriveChanged {
        account_id: account_id.to_string(),
    });
}

/// A file as the app shows it.
pub fn file_json(meta: &FileMeta) -> Value {
    let owner = meta
        .owners
        .first()
        .map(|o| json!({ "name": o.name, "email": o.email }));
    json!({
        "id": meta.id,
        "name": meta.name,
        "mime": meta.mime_type,
        "kind": drive::kind_label(&meta.mime_type),
        "modified": drive::rfc3339_ms(meta.modified_time.as_deref()),
        "owner": owner,
        "web_link": meta.web_view_link,
        "icon_link": meta.icon_link,
        "has_thumbnail": meta.thumbnail_link.is_some(),
    })
}

/// Details of one file: from the cache while fresh, else from Drive. Access is "ok",
/// "no_access" (Drive refused or does not know it) or "error" (try again later).
async fn resolve(
    cache: &DriveCache,
    conn: &Conn,
    account_id: &str,
    id: &str,
) -> (String, Option<FileMeta>) {
    let cached = cache.file(account_id, id).ok().flatten();
    if let Some(c) = &cached
        && now_ms() - c.fetched < META_TTL_MS
    {
        return (c.access.clone(), c.meta.clone());
    }
    match conn.drive.get(id).await {
        Ok(meta) => {
            let _ = cache.put_file(
                account_id,
                id,
                &CachedFile {
                    access: "ok".into(),
                    meta: Some(meta.clone()),
                    fetched: now_ms(),
                },
            );
            ("ok".into(), Some(meta))
        }
        Err(e) if no_access(&e) => {
            let _ = cache.put_file(
                account_id,
                id,
                &CachedFile {
                    access: "no_access".into(),
                    meta: None,
                    fetched: now_ms(),
                },
            );
            ("no_access".into(), None)
        }
        Err(e) => {
            warn!(file = %id, "drive file details: {e}");
            ("error".into(), cached.and_then(|c| c.meta))
        }
    }
}

/// Remembers files Drive just listed, so previews and content reuse them.
fn remember(cache: &DriveCache, account_id: &str, files: &[FileMeta]) {
    for f in files {
        let _ = cache.put_file(
            account_id,
            &f.id,
            &CachedFile {
                access: "ok".into(),
                meta: Some(f.clone()),
                fetched: now_ms(),
            },
        );
    }
}

/// Drive links in a thread's conversation, once each, with the message they first appear in.
fn thread_links(svc: &Service, thread_id: &str) -> Result<Vec<(String, DriveLink)>> {
    let mut out: Vec<(String, DriveLink)> = vec![];
    for m in svc.store.conversation_messages(thread_id)? {
        let html = svc.store.message_html(&m.id)?.0.unwrap_or_default();
        for text in [m.body_text.as_str(), html.as_str()] {
            for l in drive::find_links(text) {
                if !out.iter().any(|(_, x)| x.file_id == l.file_id) {
                    out.push((m.id.clone(), l));
                }
            }
        }
    }
    Ok(out)
}

/// The linked files of a thread with their details (looked up in parallel).
async fn linked(
    svc: &Arc<Service>,
    thread_id: &str,
) -> Result<(String, Vec<Value>, Vec<FileMeta>)> {
    let account_id = svc.store.provider_thread_id(thread_id)?.0;
    let links = thread_links(svc, thread_id)?;
    let conn = svc.drive.conn(svc, &account_id)?;
    let Some(conn) = conn else {
        let out = links
            .iter()
            .map(|(mid, l)| {
                json!({ "file_id": l.file_id, "url": l.url, "link_kind": l.kind, "message_id": mid,
                        "access": "not_connected", "file": null })
            })
            .collect();
        return Ok((account_id, out, vec![]));
    };
    let cache = svc.drive.cache(svc)?;
    let mut set = tokio::task::JoinSet::new();
    for (i, (_, l)) in links.iter().enumerate() {
        let (cache, conn, acc, id) = (
            cache.clone(),
            conn.clone(),
            account_id.clone(),
            l.file_id.clone(),
        );
        set.spawn(async move { (i, resolve(&cache, &conn, &acc, &id).await) });
    }
    let mut resolved: Vec<Option<(String, Option<FileMeta>)>> = vec![None; links.len()];
    while let Some(Ok((i, r))) = set.join_next().await {
        resolved[i] = Some(r);
    }
    let mut out = vec![];
    let mut metas = vec![];
    for ((mid, l), r) in links.iter().zip(resolved) {
        let (access, meta) = r.unwrap_or(("error".into(), None));
        out.push(json!({
            "file_id": l.file_id, "url": l.url, "link_kind": l.kind, "message_id": mid,
            "access": access, "file": meta.as_ref().map(file_json),
        }));
        if let Some(m) = meta {
            metas.push(m);
        }
    }
    Ok((account_id, out, metas))
}

/// Document text for the AI, cached by version.
async fn text_of(
    cache: &DriveCache,
    conn: &Conn,
    account_id: &str,
    meta: &FileMeta,
) -> Result<Option<String>> {
    let modified = drive::rfc3339_ms(meta.modified_time.as_deref());
    if let Some((data, _)) = cache.content(account_id, &meta.id, "text", modified)? {
        return Ok(Some(String::from_utf8_lossy(&data).into_owned()));
    }
    let text = conn.drive.text(meta).await?;
    if let Some(t) = &text {
        cache.put_content(
            account_id,
            &meta.id,
            "text",
            modified,
            "text/plain",
            t.as_bytes(),
        )?;
    }
    Ok(text)
}

/// The text of the documents a thread links to, for "Chat about this". Best effort:
/// without Drive, or past [`CHAT_DOCS_WAIT`], the AI answers from the mail alone.
pub async fn documents_for_chat(svc: &Arc<Service>, thread_id: &str) -> Vec<LinkedDoc> {
    let work = async {
        let (account_id, _, metas) = linked(svc, thread_id).await.ok()?;
        let conn = svc.drive.conn(svc, &account_id).ok()??;
        let cache = svc.drive.cache(svc).ok()?;
        let mut docs = vec![];
        for m in metas {
            match text_of(&cache, &conn, &account_id, &m).await {
                Ok(Some(text)) => docs.push(LinkedDoc {
                    title: m.name.clone(),
                    kind: drive::kind_label(&m.mime_type).to_string(),
                    text,
                }),
                Ok(None) => {}
                Err(e) => warn!(file = %m.id, "linked document text: {e}"),
            }
        }
        Some(docs)
    };
    tokio::time::timeout(CHAT_DOCS_WAIT, work)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Opens the browser consent for Drive; the grant is stored once the user approves.
async fn connect(svc: &Arc<Service>, account_id: &str) -> Result<String> {
    let acc = svc.store.account(account_id)?;
    // The app's own client (Google names its project on the consent screen), or the
    // one the user chose for this account; never a client merely borrowed for Gmail.
    let (cfg, _) = tokens::consent_client(&acc.email)?;
    let pending = oauth::start_request(
        cfg.clone(),
        AuthRequest {
            scopes: drive::SCOPES.iter().map(|s| s.to_string()).collect(),
            login_hint: Some(acc.email.clone()),
            offline: true,
            extra: vec![],
            done_message: "Google Drive is connected to Mermoz. You can close this tab.".into(),
        },
    )
    .await?;
    let url = pending.auth_url.clone();
    set_attempt(svc, &acc.id, "waiting", None);
    let (svc2, acc2) = (svc.clone(), acc.clone());
    let handle = tokio::spawn(async move {
        let outcome = match pending.finish_grant(CONSENT_WAIT).await {
            Ok(g) => store_grant(&svc2, &acc2.id, &acc2.email, &cfg, g).await,
            Err(e) => Err(e),
        };
        match outcome {
            Ok(()) => {
                info!(account = %acc2.email, "drive connected");
                set_attempt(&svc2, &acc2.id, "connected", None)
            }
            Err(Error::Declined(_)) => set_attempt(&svc2, &acc2.id, "declined", None),
            Err(e) => {
                warn!(account = %acc2.email, "drive connect failed: {e}");
                set_attempt(&svc2, &acc2.id, "error", Some(e.to_string()))
            }
        }
        changed(&svc2, &acc2.id);
    });
    if let Some(old) = svc
        .drive
        .tasks
        .lock()
        .unwrap()
        .insert(acc.id.clone(), handle)
    {
        old.abort();
    }
    Ok(url)
}

fn set_attempt(svc: &Service, account_id: &str, state: &str, error: Option<String>) {
    svc.drive.attempts.lock().unwrap().insert(
        account_id.to_string(),
        Attempt {
            state: state.into(),
            error,
        },
    );
}

/// Checks a fresh grant belongs to the account and covers Drive, then stores it.
async fn store_grant(
    svc: &Arc<Service>,
    account_id: &str,
    email: &str,
    cfg: &ClientConfig,
    g: Granted,
) -> Result<()> {
    let scopes: Vec<String> = g
        .scopes
        .iter()
        .filter(|s| drive::SCOPES.contains(&s.as_str()))
        .cloned()
        .collect();
    // Every Drive box unticked on Google's screen is a "no".
    if scopes.is_empty() {
        return Err(Error::Declined("no Drive access granted".into()));
    }
    let refresh = g
        .refresh_token
        .clone()
        .ok_or_else(|| Error::other("Google returned no refresh token"))?;
    let probe = Drive::new(Arc::new(TokenSource::fixed(
        cfg.clone(),
        g.access_token.clone(),
        g.expires_in,
    )));
    let who = probe.about_email().await?;
    if !who.eq_ignore_ascii_case(email) {
        return Err(Error::other(format!(
            "You chose {who} in Google. Connect Drive with {email}."
        )));
    }
    tokens::save_drive_grant(
        email,
        &DriveGrant {
            refresh_token: refresh,
            client_id: cfg.client_id.clone(),
            client_secret: cfg.client_secret.clone(),
            token_url: cfg.token_url.clone(),
            scopes,
        },
    )?;
    svc.drive.conns.lock().unwrap().remove(account_id);
    Ok(())
}

/// Opens Google's file picker. Picked files become usable by Mermoz (and shareable)
/// even with only the "files you use with Mermoz" permission. Without a Drive
/// connection the pick also connects Drive with that permission alone.
async fn pick(svc: &Arc<Service>, account_id: &str, file_ids: Vec<String>) -> Result<Value> {
    let acc = svc.store.account(account_id)?;
    let (cfg, _) = tokens::consent_client(&acc.email)?;
    // Picked files are opened to the picking client only, so a Drive connection from
    // another client can't read them: the pick then stores a fresh grant instead.
    let existing = svc
        .drive
        .conn(svc, &acc.id)?
        .filter(|c| c.grant.client_id == cfg.client_id);
    let mut extra = vec![
        ("trigger_onepick".to_string(), "true".to_string()),
        ("allow_multiple".to_string(), "true".to_string()),
    ];
    if !file_ids.is_empty() {
        extra.push(("file_ids".into(), file_ids.join(",")));
    }
    let pending = oauth::start_request(
        cfg.clone(),
        AuthRequest {
            // Google's picker for desktop apps accepts this one scope only.
            scopes: vec![SCOPE_FILE.to_string()],
            login_hint: Some(acc.email.clone()),
            offline: existing.is_none(),
            extra,
            done_message: "Your files are ready in Mermoz. You can close this tab.".into(),
        },
    )
    .await?;
    let pick_id = new_id("pick");
    svc.drive.picks.lock().unwrap().insert(
        pick_id.clone(),
        Pick {
            account_id: acc.id.clone(),
            state: "waiting".into(),
            files: vec![],
            error: None,
        },
    );
    let url = pending.auth_url.clone();
    let (svc2, pid) = (svc.clone(), pick_id.clone());
    tokio::spawn(async move {
        let outcome: Result<Vec<Value>> = async {
            let g = pending.finish_grant(CONSENT_WAIT).await?;
            if existing.is_none() {
                store_grant(&svc2, &acc.id, &acc.email, &cfg, g.clone()).await?;
            }
            let token = Arc::new(TokenSource::fixed(
                cfg.clone(),
                g.access_token.clone(),
                g.expires_in,
            ));
            let probe = Drive::new(token);
            let cache = svc2.drive.cache(&svc2)?;
            let mut files = vec![];
            for id in &g.picked {
                let meta = probe.get(id).await?;
                remember(&cache, &acc.id, std::slice::from_ref(&meta));
                files.push(file_json(&meta));
            }
            Ok(files)
        }
        .await;
        let mut picks = svc2.drive.picks.lock().unwrap();
        if let Some(p) = picks.get_mut(&pid) {
            match outcome {
                Ok(files) => {
                    p.state = "done".into();
                    p.files = files;
                }
                Err(Error::Declined(_)) => p.state = "declined".into(),
                Err(e) => {
                    warn!("drive pick failed: {e}");
                    p.state = "error".into();
                    p.error = Some(e.to_string());
                }
            }
        }
        drop(picks);
        changed(&svc2, &acc.id);
    });
    Ok(json!({ "pick_id": pick_id, "auth_url": url }))
}

/// Finds or creates each folder of `path` under My Drive; returns the last one's id.
async fn ensure_folders(
    cache: &DriveCache,
    conn: &Conn,
    account_id: &str,
    path: &[String],
) -> Result<String> {
    let mut parent = "root".to_string();
    for i in 0..path.len() {
        let key = path[..=i].join("/");
        if let Some(id) = cache.folder(account_id, &key)? {
            parent = id;
            continue;
        }
        let id = match conn.drive.find_folder(&path[i], &parent).await? {
            Some(id) => id,
            None => conn.drive.create_folder(&path[i], &parent).await?,
        };
        cache.put_folder(account_id, &key, &id)?;
        parent = id;
    }
    Ok(parent)
}

async fn save_attachment(
    svc: &Arc<Service>,
    attachment_id: &str,
) -> std::result::Result<Value, RpcError> {
    let st = &svc.store;
    let (account_id, thread_id) = st.attachment_thread(attachment_id)?;
    let conn = svc.drive.require(svc, &account_id)?;
    if !conn.grant.has(SCOPE_FILE) {
        return Err(needs_access(
            &account_id,
            "save",
            "Mermoz needs permission to add files to your Google Drive. Connect Google Drive again and keep that box ticked.",
        ));
    }
    let cache = svc.drive.cache(svc)?;
    if let Some(saved) = cache.saved(&account_id, attachment_id)? {
        return to_value(saved);
    }
    let (path, filename, mime, _) = st.attachment(attachment_id)?;
    let bytes = tokio::fs::read(&path).await.map_err(Error::from)?;
    let folders = drive::folder_path(&st.thread(&thread_id)?.projects);
    let mut retried = false;
    let meta = loop {
        let attempt = async {
            let parent = ensure_folders(&cache, &conn, &account_id, &folders).await?;
            conn.drive
                .upload(&filename, &mime, bytes.clone(), &parent)
                .await
        }
        .await;
        match attempt {
            Ok(m) => break m,
            // A folder Mermoz remembered was deleted in Drive: find or make it again.
            Err(Error::Upstream { status: 404, .. }) if !retried => {
                retried = true;
                cache.forget_folders(&account_id)?;
            }
            Err(e) if revoked(&e) => {
                svc.drive.forget(svc, &account_id);
                return Err(not_connected(&account_id));
            }
            Err(e) => return Err(e.into()),
        }
    };
    let saved = Saved {
        attachment_id: attachment_id.to_string(),
        file_id: meta.id.clone(),
        name: meta.name.clone(),
        web_link: meta.web_view_link.clone(),
        folder: folders.join("/"),
        created: now_ms(),
    };
    cache.put_saved(&account_id, &saved)?;
    remember(&cache, &account_id, std::slice::from_ref(&meta));
    info!(folder = %saved.folder, "attachment saved to drive");
    changed(svc, &account_id);
    to_value(saved)
}

/// Downloads kept this long, so a draft saved later can still copy them.
const DOWNLOAD_TTL: Duration = Duration::from_secs(24 * 3600);

/// A download the user can't have, with what went wrong in `data.reason`
/// ("too_large", "no_file_form", "no_access" or "failed").
fn download_error(reason: &str, message: String) -> RpcError {
    RpcError {
        code: -32602,
        message,
        data: Some(json!({ "reason": reason })),
    }
}

/// Removes downloads older than [`DOWNLOAD_TTL`]: drafts have copied theirs by then.
fn prune_downloads(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > DOWNLOAD_TTL);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// A Drive file's bytes saved on this Mac, to attach as a regular file: stored files
/// as they are, Google files exported (PDF unless `format` says otherwise).
async fn download(
    svc: &Arc<Service>,
    account_id: &str,
    file_id: &str,
    format: Option<&str>,
) -> std::result::Result<Value, RpcError> {
    use mermoz_core::compose::{MAX_ATTACHMENT_BYTES, human_size};
    let conn = svc.drive.require(svc, account_id)?;
    let cache = svc.drive.cache(svc)?;
    let (access, meta) = resolve(&cache, &conn, account_id, file_id).await;
    let Some(meta) = meta else {
        return Err(download_error(
            if access == "error" {
                "failed"
            } else {
                "no_access"
            },
            if access == "error" {
                "Google Drive is unavailable right now. Try again, or keep it as a link.".into()
            } else {
                "Mermoz can't open this file in Google Drive. Keep it as a link.".into()
            },
        ));
    };
    let form = drive::as_file(&meta, format).map_err(|m| download_error("no_file_form", m))?;
    let too_large = |size: Option<u64>| {
        let how_big = size
            .map(|s| format!("is {}, ", human_size(s)))
            .unwrap_or_else(|| "is ".into());
        download_error(
            "too_large",
            format!(
                "“{}” {how_big}more than the {} Gmail can send in one email. Keep it as a link.",
                form.filename,
                human_size(MAX_ATTACHMENT_BYTES)
            ),
        )
    };
    let declared = meta.size.as_deref().and_then(|s| s.parse::<u64>().ok());
    if declared.is_some_and(|s| s > MAX_ATTACHMENT_BYTES) {
        return Err(too_large(declared));
    }
    let bytes = match conn
        .drive
        .download(&meta.id, form.export, MAX_ATTACHMENT_BYTES)
        .await
    {
        Ok(b) => b,
        Err(e) if revoked(&e) => {
            svc.drive.forget(svc, account_id);
            return Err(not_connected(account_id));
        }
        // Google refuses to export documents past its own size limit.
        Err(Error::Upstream { body, .. }) if body.contains("exportSizeLimitExceeded") => {
            return Err(too_large(None));
        }
        Err(e) => {
            warn!(file = %file_id, "drive download: {e}");
            return Err(download_error(
                "failed",
                format!(
                    "Couldn't download “{}” from Google Drive. Try again, or keep it as a link.",
                    form.filename
                ),
            ));
        }
    };
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(too_large(None));
    }
    let root = paths::blobs_dir().join("drive-downloads");
    prune_downloads(&root);
    let dir = root.join(new_id("dl"));
    std::fs::create_dir_all(&dir).map_err(Error::from)?;
    let path = dir.join(mermoz_core::store::safe_filename(&form.filename));
    tokio::fs::write(&path, &bytes).await.map_err(Error::from)?;
    info!(file = %file_id, bytes = bytes.len(), "drive file downloaded to attach");
    Ok(json!({
        "file_id": meta.id,
        "path": path.to_string_lossy(),
        "filename": form.filename,
        "mime": form.mime,
        "size": bytes.len(),
    }))
}

async fn share(
    svc: &Arc<Service>,
    account_id: &str,
    file_id: &str,
    emails: &[String],
    role: &str,
) -> std::result::Result<Value, RpcError> {
    if role != "reader" && role != "commenter" {
        return Err(RpcError::invalid(
            "'role' must be \"reader\" or \"commenter\"",
        ));
    }
    let conn = svc.drive.require(svc, account_id)?;
    let (mut shared, mut failed) = (vec![], vec![]);
    for email in emails {
        match conn.drive.share(file_id, email, role).await {
            Ok(()) => shared.push(email.clone()),
            // Mermoz may change sharing only on files the user picked or Mermoz made.
            Err(e) if no_access(&e) && shared.is_empty() => {
                return Err(needs_access(
                    account_id,
                    "pick",
                    "Choose this file in Google Drive first, so Mermoz may share it.",
                ));
            }
            Err(e) => {
                warn!(file = %file_id, "share failed: {e}");
                failed.push(
                    json!({ "email": email, "reason": "Google didn't accept this address." }),
                );
            }
        }
    }
    info!(file = %file_id, people = shared.len(), role, "shared a drive file");
    Ok(json!({ "shared": shared, "failed": failed }))
}

fn status(svc: &Arc<Service>, account_id: &str) -> std::result::Result<Value, RpcError> {
    let acc = svc.store.account(account_id)?;
    let conn = svc.drive.conn(svc, &acc.id)?;
    let attempt = svc.drive.attempts.lock().unwrap().get(&acc.id).cloned();
    Ok(json!({
        "account_id": acc.id,
        "email": acc.email,
        "connected": conn.is_some(),
        "can_read_all": conn.as_ref().is_some_and(|c| c.grant.has(SCOPE_READ)),
        "can_save": conn.as_ref().is_some_and(|c| c.grant.has(SCOPE_FILE)),
        "attempt": attempt,
    }))
}

/// Participants' addresses of a thread's conversation, without the user.
fn other_people(svc: &Service, thread_id: &str, me: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = vec![];
    for m in svc.store.conversation_messages(thread_id)? {
        for a in std::iter::once(&m.from).chain(&m.to).chain(&m.cc) {
            let e = a.email.to_lowercase();
            if !e.is_empty() && e != me.to_lowercase() && !out.contains(&e) {
                out.push(e);
            }
        }
    }
    Ok(out)
}

fn limit(p: &Value, default: i64) -> usize {
    p.get("limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(default)
        .clamp(1, 100) as usize
}

async fn list_files(
    svc: &Arc<Service>,
    account_id: &str,
    q: Option<String>,
    order: &str,
    n: usize,
) -> std::result::Result<Vec<FileMeta>, RpcError> {
    let conn = svc.drive.require(svc, account_id)?;
    if !conn.grant.has(SCOPE_READ) {
        return Err(needs_access(
            account_id,
            "read_all",
            "Mermoz can only see the files you choose. Connect Google Drive again to search all your files.",
        ));
    }
    match conn.drive.list(q.as_deref(), order, n).await {
        Ok(files) => {
            let cache = svc.drive.cache(svc)?;
            remember(&cache, account_id, &files);
            Ok(files)
        }
        Err(e) if revoked(&e) => {
            svc.drive.forget(svc, account_id);
            Err(not_connected(account_id))
        }
        Err(e) => Err(e.into()),
    }
}

fn account_of(svc: &Service, p: &Value) -> std::result::Result<String, RpcError> {
    Ok(svc.store.account(&s(p, "account_id")?)?.id)
}

pub async fn dispatch(svc: &Arc<Service>, caller: Caller, method: &str, p: &Value) -> R {
    let st = &svc.store;
    match method {
        "drive.status" => match opt_s(p, "account_id") {
            Some(a) => status(svc, &a),
            None => Ok(Value::Array(
                st.accounts()?
                    .iter()
                    .map(|a| status(svc, &a.id))
                    .collect::<std::result::Result<_, _>>()?,
            )),
        },
        "drive.connect" => {
            // Opens a consent screen in the user's browser: only the app may.
            app_only(caller, method)?;
            Ok(json!({ "auth_url": connect(svc, &account_of(svc, p)?).await? }))
        }
        "drive.disconnect" => {
            app_only(caller, method)?;
            let account_id = account_of(svc, p)?;
            let email = st.account(&account_id)?.email;
            // Only the local token is dropped. Google's revoke endpoint would withdraw
            // the whole app's access, Gmail included; the user removes Drive alone in
            // their Google account settings if they want it gone there too.
            tokens::delete_drive_grant(&email)?;
            svc.drive.forget(svc, &account_id);
            svc.drive.attempts.lock().unwrap().remove(&account_id);
            changed(svc, &account_id);
            Ok(json!({}))
        }
        "drive.links" => {
            let id = st.resolve_thread_id(&s(p, "thread_id")?)?;
            let (account_id, links, _) = linked(svc, &id).await?;
            let connected = svc.drive.conn(svc, &account_id)?.is_some();
            Ok(json!({ "account_id": account_id, "connected": connected, "links": links }))
        }
        "drive.files_for_thread" => {
            let id = st.resolve_thread_id(&s(p, "thread_id")?)?;
            let (account_id, links, _) = linked(svc, &id).await?;
            let conn = svc.drive.conn(svc, &account_id)?;
            let can_read_all = conn.as_ref().is_some_and(|c| c.grant.has(SCOPE_READ));
            let mut shared = vec![];
            if can_read_all {
                let me = st.account(&account_id)?.email;
                let people = other_people(svc, &id, &me)?;
                if let Some(q) = drive::people_query(&people) {
                    match list_files(svc, &account_id, Some(q), "modifiedTime desc", limit(p, 20))
                        .await
                    {
                        Ok(files) => shared = files,
                        Err(e) => {
                            warn!(thread = %id, "files shared with participants: {}", e.message)
                        }
                    }
                }
            }
            let linked_ids: Vec<&str> =
                links.iter().filter_map(|l| l["file_id"].as_str()).collect();
            let shared: Vec<Value> = shared
                .iter()
                .filter(|f| !linked_ids.contains(&f.id.as_str()))
                .map(file_json)
                .collect();
            Ok(json!({
                "account_id": account_id, "connected": conn.is_some(), "can_read_all": can_read_all,
                "linked": links, "shared": shared, "saved": saved_in_thread(svc, &account_id, &id)?,
            }))
        }
        "drive.files_for_people" => {
            let account_id = account_of(svc, p)?;
            let emails =
                str_list(p, "emails")?.ok_or_else(|| RpcError::invalid("missing 'emails'"))?;
            let q = drive::people_query(&emails)
                .ok_or_else(|| RpcError::invalid("'emails' holds no address"))?;
            let files =
                list_files(svc, &account_id, Some(q), "modifiedTime desc", limit(p, 20)).await?;
            Ok(json!({ "files": files.iter().map(file_json).collect::<Vec<_>>() }))
        }
        "drive.search" => {
            let account_id = account_of(svc, p)?;
            let query = opt_s(p, "query").unwrap_or_default();
            // No words: the files the user opened most recently.
            let (q, order) = if query.trim().is_empty() {
                (
                    format!(
                        "trashed = false and mimeType != {}",
                        drive::quote(drive::FOLDER_MIME)
                    ),
                    "viewedByMeTime desc",
                )
            } else {
                (drive::search_query(&query), "modifiedTime desc")
            };
            let files = list_files(svc, &account_id, Some(q), order, limit(p, 20)).await?;
            Ok(json!({ "files": files.iter().map(file_json).collect::<Vec<_>>() }))
        }
        "drive.content" => {
            let account_id = account_of(svc, p)?;
            let conn = svc.drive.require(svc, &account_id)?;
            let cache = svc.drive.cache(svc)?;
            let file_id = s(p, "file_id")?;
            let (access, meta) = resolve(&cache, &conn, &account_id, &file_id).await;
            let Some(meta) = meta else {
                return Err(if access == "error" {
                    RpcError::invalid("Google Drive is unavailable right now.")
                } else {
                    needs_access(&account_id, "pick", "Mermoz can't open this file.")
                });
            };
            let text = text_of(&cache, &conn, &account_id, &meta).await?;
            Ok(json!({ "file": file_json(&meta), "text": text }))
        }
        "drive.thumbnail" => {
            let account_id = account_of(svc, p)?;
            let conn = svc.drive.require(svc, &account_id)?;
            let cache = svc.drive.cache(svc)?;
            let file_id = s(p, "file_id")?;
            let (_, meta) = resolve(&cache, &conn, &account_id, &file_id).await;
            let Some(meta) = meta else {
                return Ok(Value::Null);
            };
            let Some(link) = meta.thumbnail_link.as_deref() else {
                return Ok(Value::Null);
            };
            let modified = drive::rfc3339_ms(meta.modified_time.as_deref());
            let (data, mime) = match cache.content(&account_id, &file_id, "thumb", modified)? {
                Some(hit) => hit,
                None => match conn.drive.thumbnail(link).await {
                    Ok((data, mime)) => {
                        cache.put_content(
                            &account_id,
                            &file_id,
                            "thumb",
                            modified,
                            &mime,
                            &data,
                        )?;
                        (data, mime)
                    }
                    // A preview is a nicety: the card shows the file's icon instead.
                    Err(e) => {
                        warn!(file = %file_id, "thumbnail: {e}");
                        return Ok(Value::Null);
                    }
                },
            };
            Ok(
                json!({ "mime": mime, "data": base64::engine::general_purpose::STANDARD.encode(data) }),
            )
        }
        "drive.pick" => {
            app_only(caller, method)?;
            let account_id = account_of(svc, p)?;
            let ids = str_list(p, "file_ids")?.unwrap_or_default();
            Ok(pick(svc, &account_id, ids).await?)
        }
        "drive.pick_result" => {
            let id = s(p, "pick_id")?;
            let pick = svc.drive.picks.lock().unwrap().get(&id).cloned();
            to_value(pick.ok_or_else(|| Error::NotFound(format!("pick {id}")))?)
        }
        "drive.save_attachment" => save_attachment(svc, &s(p, "attachment_id")?).await,
        "drive.download" => {
            let account_id = account_of(svc, p)?;
            let format = opt_s(p, "format");
            download(svc, &account_id, &s(p, "file_id")?, format.as_deref()).await
        }
        "drive.saved" => {
            let id = st.resolve_thread_id(&s(p, "thread_id")?)?;
            let account_id = st.provider_thread_id(&id)?.0;
            Ok(json!(saved_in_thread(svc, &account_id, &id)?))
        }
        "drive.share" => {
            // Changing who can open a file needs the user's explicit OK in the app.
            app_only(caller, method)?;
            let account_id = account_of(svc, p)?;
            let emails =
                str_list(p, "emails")?.ok_or_else(|| RpcError::invalid("missing 'emails'"))?;
            let role = opt_s(p, "role").unwrap_or_else(|| "reader".into());
            share(svc, &account_id, &s(p, "file_id")?, &emails, &role).await
        }
        _ => Err(RpcError {
            code: -32601,
            message: format!("method not found: {method}"),
            data: None,
        }),
    }
}

fn saved_in_thread(svc: &Arc<Service>, account_id: &str, thread_id: &str) -> Result<Vec<Saved>> {
    let cache = svc.drive.cache(svc)?;
    let mut out = vec![];
    for m in svc.store.conversation_messages(thread_id)? {
        for a in &m.attachments {
            if let Some(s) = cache.saved(account_id, &a.id)? {
                out.push(s);
            }
        }
    }
    Ok(out)
}
