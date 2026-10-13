//! JSON-RPC method dispatch; see `docs/protocol.md` for the contract.

use crate::service::Service;
use mermoz_core::Error;
use mermoz_core::model::ThreadState;
use mermoz_core::model::{Draft, new_id, now_ms};
use mermoz_core::oauth::ClientConfig;
use mermoz_core::sanitize::{has_remote_images, sanitize_html_with};
use mermoz_core::store::{
    AttachmentSource, ColumnScope, NewAttachment, SortAnswer, SortUndo, SuggestedFilter,
    SuggestionGroup, ThreadFilter, ThreadPatch,
};
use mermoz_core::sync::Event;
use serde_json::{Value, json};
use std::sync::Arc;

pub const APPROVAL_REQUIRED: i64 = -32010;
pub const APP_ONLY: i64 = -32011;

#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::new(-32602, msg)
    }
}

impl From<Error> for RpcError {
    fn from(e: Error) -> Self {
        match e {
            Error::NotFound(m) => RpcError::new(-32004, format!("not found: {m}")),
            Error::InvalidParams(m) => RpcError::new(-32602, m),
            Error::Upstream { status, body } => RpcError {
                code: -32020,
                message: format!("provider error {status}"),
                data: Some(json!({ "status": status, "body": body })),
            },
            other => RpcError::new(-32000, other.to_string()),
        }
    }
}

pub(crate) type R = Result<Value, RpcError>;

/// Who is calling: the app is trusted to send and to change the send gate.
#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub is_app: bool,
}

pub(crate) fn s(p: &Value, k: &str) -> Result<String, RpcError> {
    p.get(k)
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or_else(|| RpcError::invalid(format!("missing string param '{k}'")))
}

/// Puts the message's own images (`cid:` links) in its HTML. Mail stored before
/// Content-IDs were kept learns them from Gmail once, on first view.
async fn inline_images(svc: &Arc<Service>, mid: &str, html: String) -> Result<String, RpcError> {
    use mermoz_core::inline;
    if !inline::has_cid(&html) {
        return Ok(html);
    }
    let st = &svc.store;
    let (out, missing) = inline::resolve(&html, &st.inline_parts(mid)?);
    if missing == 0 {
        return Ok(out);
    }
    let Some((account, provider_id)) = st.message_missing_cids(mid)? else {
        return Ok(out);
    };
    let fetched = match svc.worker(&account) {
        Ok(w) => w.sync.gmail.get_raw(&provider_id).await.ok(),
        Err(_) => None,
    };
    let parsed = fetched
        .and_then(|m| mermoz_core::gmail::decode_raw(m.raw.as_deref()?).ok())
        .and_then(|raw| mermoz_core::mime::parse(&raw));
    let Some(parsed) = parsed else {
        return Ok(out);
    };
    st.set_attachment_cids(mid, &parsed.attachments)?;
    Ok(inline::resolve(&html, &st.inline_parts(mid)?).0)
}

/// A boolean parameter, false when absent.
fn flag(p: &Value, k: &str) -> bool {
    p.get(k).and_then(|v| v.as_bool()).unwrap_or(false)
}

pub(crate) fn opt_s(p: &Value, k: &str) -> Option<String> {
    p.get(k).and_then(|v| v.as_str()).map(|v| v.to_string())
}

pub(crate) fn str_list(p: &Value, k: &str) -> Result<Option<Vec<String>>, RpcError> {
    match p.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => Ok(Some(
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
        )),
        Some(Value::String(v)) => Ok(Some(
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        )),
        _ => Err(RpcError::invalid(format!(
            "'{k}' must be a list of strings"
        ))),
    }
}

/// One attachment spec: a full path, or `{path | from_attachment | url, filename?, inline?}`.
fn attachment_spec(v: &Value) -> Result<NewAttachment, RpcError> {
    let path = |p: &str| -> Result<AttachmentSource, RpcError> {
        let pb = std::path::PathBuf::from(p);
        if !pb.is_absolute() {
            return Err(RpcError::invalid(format!(
                "attachment path '{p}' must be absolute"
            )));
        }
        Ok(AttachmentSource::Path(pb))
    };
    if let Some(p) = v.as_str() {
        return Ok(NewAttachment {
            source: path(p)?,
            filename: None,
            inline: false,
        });
    }
    let source = if let Some(p) = opt_s(v, "path") {
        path(&p)?
    } else if let Some(a) = opt_s(v, "from_attachment") {
        AttachmentSource::Message(a)
    } else if let Some(u) = opt_s(v, "url") {
        AttachmentSource::Link(u)
    } else {
        return Err(RpcError::invalid(
            "an attachment needs 'path', 'from_attachment' or 'url'",
        ));
    };
    Ok(NewAttachment {
        source,
        filename: opt_s(v, "filename"),
        inline: v.get("inline").and_then(|b| b.as_bool()).unwrap_or(false),
    })
}

fn attachment_specs(p: &Value) -> Result<Vec<NewAttachment>, RpcError> {
    match p.get("attachments") {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => a.iter().map(attachment_spec).collect(),
        Some(_) => Err(RpcError::invalid("'attachments' must be a list")),
    }
}

/// `Some(None)` means "clear": the key is present with null.
fn nullable_s(p: &Value, k: &str) -> Option<Option<String>> {
    match p.get(k) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(v.as_str().map(|s| s.to_string())),
    }
}

pub(crate) fn app_only(c: Caller, method: &str) -> Result<(), RpcError> {
    if c.is_app {
        Ok(())
    } else {
        Err(RpcError::new(
            APP_ONLY,
            format!("app_only: {method} can only be done by the user in Mermoz"),
        ))
    }
}

pub(crate) fn to_value<T: serde::Serialize>(v: T) -> R {
    serde_json::to_value(v).map_err(|e| RpcError::new(-32000, e.to_string()))
}

pub async fn dispatch(svc: &Arc<Service>, caller: Caller, method: &str, p: &Value) -> R {
    let st = &svc.store;
    match method {
        "hello" => Ok(json!({ "version": env!("CARGO_PKG_VERSION") })),
        "accounts.list" => {
            let mut v = to_value(st.accounts()?)?;
            for a in v.as_array_mut().into_iter().flatten() {
                let id = a["id"].as_str().unwrap_or_default().to_string();
                a["needs_reconnect"] = json!(svc.needs_reconnect(&id));
            }
            Ok(v)
        }
        "accounts.add_gmail" => {
            // The user's own sign-in client, from the Google Cloud download or pasted.
            let (file, id, secret) = (
                opt_s(p, "client_json"),
                opt_s(p, "client_id"),
                opt_s(p, "client_secret"),
            );
            let custom = if file.is_some() || id.is_some() || secret.is_some() {
                Some(ClientConfig::from_user(
                    file.as_deref(),
                    id.as_deref(),
                    secret.as_deref(),
                )?)
            } else {
                None
            };
            Ok(json!({ "auth_url": svc.add_gmail(custom).await? }))
        }
        "accounts.sign_in_status" => {
            let mut v = to_value(svc.gmail_sign_in.lock().unwrap().clone())?;
            // What an admin allow-lists to trust Mermoz, shown with Google's refusals.
            v["app_client_id"] = json!(ClientConfig::load().ok().map(|c| c.client_id));
            Ok(v)
        }
        "accounts.reconnect" => {
            let id = s(p, "account_id")?;
            Ok(json!({ "auth_url": svc.reconnect_gmail(&id).await? }))
        }
        "accounts.remove" => {
            let acc = st.account(&s(p, "id")?)?;
            svc.stop_account(&acc.id);
            st.remove_account(&acc.id)?;
            mermoz_core::tokens::delete(&acc.email)?;
            mermoz_core::tokens::delete_drive_grant(&acc.email)?;
            let _ = svc.events.send(Event::AccountsChanged);
            Ok(json!({}))
        }
        "statuses.list" => to_value(st.statuses()?),
        "labels.list" => to_value(st.user_labels()?),
        "projects.list" => to_value(st.projects()?),
        "projects.update" => {
            let pr = st.update_project(
                &s(p, "id")?,
                opt_s(p, "name").as_deref(),
                opt_s(p, "color").as_deref(),
            )?;
            let _ = svc.events.send(Event::ThreadsChanged(vec![]));
            svc.kick_all();
            to_value(pr)
        }
        "projects.delete" => {
            let n = st.delete_project(&s(p, "id")?)?;
            let _ = svc.events.send(Event::ThreadsChanged(vec![]));
            svc.kick_all();
            Ok(json!({ "threads": n }))
        }
        "projects.create" => {
            to_value(st.create_project(&s(p, "name")?, opt_s(p, "color").as_deref())?)
        }
        "counts" => Ok(st.counts(flag(p, "conversations"))?),
        "threads.list" => {
            let priority = match p.get("priority") {
                Some(Value::Array(a)) => Some(a.iter().filter_map(|v| v.as_i64()).collect()),
                _ => None,
            };
            let f = ThreadFilter {
                suggested: suggested_filter(p)?,
                status: str_list(p, "status")?,
                project: opt_s(p, "project").or_else(|| opt_s(p, "project_id")),
                account_id: opt_s(p, "account_id"),
                label_id: opt_s(p, "label_id"),
                participants: str_list(p, "participant")?,
                order: opt_s(p, "order"),
                reverse: p.get("reverse").and_then(|v| v.as_bool()).unwrap_or(false),
                priority,
                limit: p
                    .get("limit")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(200)
                    .min(2000),
                offset: p.get("offset").and_then(|v| v.as_i64()).unwrap_or(0),
                conversations: flag(p, "conversations"),
                kinds: str_list(p, "kind")?,
            };
            let (threads, total) = st.threads(&f)?;
            Ok(json!({ "threads": threads, "total": total }))
        }
        "threads.get" => {
            let id = st.resolve_thread_id(&s(p, "id")?)?;
            svc.verify_thread_senders(&id);
            if !flag(p, "conversation") {
                return Ok(json!({ "thread": st.thread(&id)?, "messages": st.messages(&id)? }));
            }
            // `latest`: the app opens on the newest messages and asks for the rest
            // only when the reader scrolls back (a 992-thread conversation was 14 MB).
            let latest = p.get("latest").and_then(|v| v.as_u64()).map(|n| n as usize);
            let mut messages = st.conversation_window(&id, latest)?;
            for t in st.conversation_thread_ids(&id)? {
                if t != id {
                    svc.verify_thread_senders(&t);
                }
            }
            let conversation = st.conversation(&id, &messages)?;
            let earlier = latest.map_or(0, |n| messages.len().saturating_sub(n));
            messages.drain(..earlier);
            Ok(json!({ "thread": st.thread(&id)?, "messages": messages,
                "conversation": conversation, "earlier": earlier }))
        }
        "conversations.split" => {
            let id = st.resolve_thread_id(&s(p, "id")?)?;
            let touched = st.split_thread(&id)?;
            let _ = svc.events.send(Event::ThreadsChanged(touched));
            Ok(json!({ "conversation_id": st.thread(&id)?.conversation_id }))
        }
        "conversations.merge" => {
            let ids = str_list(p, "ids")?.ok_or_else(|| RpcError::invalid("missing 'ids'"))?;
            let ids: Vec<String> = ids
                .iter()
                .map(|i| st.resolve_thread_id(i))
                .collect::<Result<_, _>>()?;
            let (conversation, members) = st.merge_threads(&ids)?;
            let _ = svc.events.send(Event::ThreadsChanged(members.clone()));
            Ok(json!({ "conversation_id": conversation, "threads": members.len() }))
        }
        "conversations.progress" => {
            let b = st.grouping_backlog()?;
            let n = |k: &str| b.get(k).copied().unwrap_or(0);
            Ok(json!({ "pending": n("pending") + n("judge"), "waiting_for_ai": n("ai") }))
        }
        "files.list" => {
            let limit = p
                .get("limit")
                .and_then(|v| v.as_i64())
                .unwrap_or(200)
                .clamp(1, 2000);
            to_value(st.shared_files(opt_s(p, "query").as_deref(), limit)?)
        }
        "threads.update" => {
            let ids = str_list(p, "ids")?.ok_or_else(|| RpcError::invalid("missing 'ids'"))?;
            let ids: Vec<String> = ids
                .iter()
                .map(|i| st.resolve_thread_id(i))
                .collect::<Result<_, _>>()?;
            // `project` (one name or null) is shorthand for replacing the whole set.
            let projects = match nullable_s(p, "project") {
                Some(one) => Some(one.into_iter().collect()),
                None => str_list(p, "projects")?,
            };
            let patch = ThreadPatch {
                status: opt_s(p, "status"),
                priority: p.get("priority").and_then(|v| v.as_i64()),
                projects,
                add_projects: str_list(p, "add_projects")?.unwrap_or_default(),
                remove_projects: str_list(p, "remove_projects")?.unwrap_or_default(),
                due: nullable_s(p, "due"),
            };
            let n = st.update_threads(&ids, &patch)?;
            let _ = svc.events.send(Event::ThreadsChanged(ids.clone()));
            for id in &ids {
                if let Ok((acc, _)) = st.provider_thread_id(id) {
                    svc.kick(&acc);
                }
            }
            Ok(json!({ "updated": n }))
        }
        "threads.accept_suggestions" => {
            let ids = str_list(p, "ids")?.ok_or_else(|| RpcError::invalid("missing 'ids'"))?;
            let ids: Vec<String> = ids
                .iter()
                .map(|i| st.resolve_thread_id(i))
                .collect::<Result<_, _>>()?;
            let n = st.accept_suggestions(&ids)?;
            let _ = svc.events.send(Event::ThreadsChanged(ids.clone()));
            svc.kick_all();
            Ok(json!({ "accepted": n }))
        }
        "threads.reject_suggestions" | "threads.reopen_suggestions" => {
            let ids = str_list(p, "ids")?.ok_or_else(|| RpcError::invalid("missing 'ids'"))?;
            let ids: Vec<String> = ids
                .iter()
                .map(|i| st.resolve_thread_id(i))
                .collect::<Result<_, _>>()?;
            let n = if method == "threads.reject_suggestions" {
                st.reject_suggestions(&ids)?
            } else {
                st.reopen_suggestions(&ids)?
            };
            let _ = svc.events.send(Event::ThreadsChanged(ids));
            let _ = svc.events.send(Event::SuggestionsChanged);
            let key = if method == "threads.reject_suggestions" {
                "rejected"
            } else {
                "reopened"
            };
            Ok(json!({ key: n }))
        }
        "threads.mark_read" => {
            let id = st.resolve_thread_id(&s(p, "id")?)?;
            let read = p.get("read").and_then(|v| v.as_bool()).unwrap_or(true);
            st.mark_read(&id, read)?;
            // Reading a conversation reads every thread in it.
            let mut ids = vec![id.clone()];
            if read {
                for t in st.conversation_thread_ids(&id)? {
                    if t != id && st.thread(&t)?.unread {
                        st.mark_read(&t, true)?;
                        ids.push(t);
                    }
                }
            }
            let _ = svc.events.send(Event::ThreadsChanged(ids));
            if let Ok((acc, _)) = st.provider_thread_id(&id) {
                svc.kick(&acc);
            }
            // The thread as reading left it (read, maybe filed), so the app patches it in
            // place instead of fetching the whole conversation again.
            Ok(json!({ "thread": st.thread(&id)? }))
        }
        "messages.source" => {
            // The email exactly as received (every header, the true From and Reply-To),
            // fetched from Gmail only when the reader asks for it.
            let mid = s(p, "id")?;
            let (account, provider_id) = st.message_origin(&mid)?;
            let raw = svc
                .worker(&account)?
                .sync
                .gmail
                .get_raw(&provider_id)
                .await?
                .raw
                .ok_or_else(|| RpcError::invalid("Gmail returned no source for this email"))?;
            let bytes = mermoz_core::gmail::decode_raw(&raw)?;
            const MAX: usize = 2 * 1024 * 1024;
            let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX)]).into_owned();
            Ok(json!({ "source": text, "truncated": bytes.len() > MAX }))
        }
        "messages.html" => {
            let mid = s(p, "id")?;
            let (html, subject) = st.message_html(&mid)?;
            let html = html.unwrap_or_default();
            // A forward is shown whole: its header block is content, not quoted history.
            let full = p.get("full").and_then(|v| v.as_bool()).unwrap_or(false)
                || mermoz_core::quote::is_forward(&subject);
            let (html, has_quoted) = if full {
                (html, false)
            } else {
                mermoz_core::quote::split_html(&html)
            };
            // Remote images only on request: they can report that the mail was opened.
            let images = p.get("images").and_then(|v| v.as_bool()).unwrap_or(false);
            let clean = inline_images(svc, &mid, sanitize_html_with(&html, images)).await?;
            Ok(json!({ "html": clean, "has_quoted": has_quoted,
                "remote_images": has_remote_images(&html) }))
        }
        "search" => {
            let limit = p
                .get("limit")
                .and_then(|v| v.as_i64())
                .unwrap_or(50)
                .clamp(1, 500);
            // `typing`: the app's live search, where the last word may still be growing.
            Ok(json!({ "threads": st.search(&s(p, "query")?, limit, flag(p, "typing"))? }))
        }
        "search.gmail" => {
            // The fallback when local search finds nothing: Gmail's own search, on every
            // connected account.
            let q = s(p, "query")?;
            let limit = p
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(20)
                .clamp(1, 50) as u32;
            let mut hits = vec![];
            for acc in st.accounts()? {
                let Ok(w) = svc.worker(&acc.id) else { continue };
                hits.extend(w.sync.search_gmail(&q, limit).await?);
            }
            Ok(json!({ "results": hits }))
        }
        "search.gmail_pull" => {
            let ids = str_list(p, "ids")?.ok_or_else(|| RpcError::invalid("missing 'ids'"))?;
            let w = svc.worker(&s(p, "account_id")?)?;
            Ok(json!({ "threads": w.sync.pull(&ids).await? }))
        }
        "attachments.get" => {
            let (path, filename, mime, size) = st.attachment(&s(p, "id")?)?;
            Ok(json!({ "path": path, "filename": filename, "mime": mime, "size": size }))
        }
        "contacts.known" => Ok(json!({ "known": st.known_address(&s(p, "email")?)? })),
        "drafts.list" => to_value(st.drafts(opt_s(p, "state").as_deref())?),
        "drafts.get" => to_value(st.draft(&s(p, "id")?)?),
        "drafts.create" => {
            let thread_id = match opt_s(p, "thread_id") {
                Some(t) => Some(st.resolve_thread_id(&t)?),
                None => None,
            };
            let account_id = match (&thread_id, opt_s(p, "account_id")) {
                (_, Some(a)) => st.account(&a)?.id,
                (Some(t), None) => st.provider_thread_id(t)?.0,
                (None, None) => st
                    .accounts()?
                    .first()
                    .map(|a| a.id.clone())
                    .ok_or_else(|| RpcError::invalid("no account configured"))?,
            };
            let me = st.account(&account_id)?.email;
            let (dto, dcc, dsubject) = match &thread_id {
                Some(t) => st.reply_defaults(t, &me)?,
                None => (vec![], vec![], String::new()),
            };
            let body = s(p, "body")?;
            let now = now_ms();
            let d = Draft {
                id: new_id("dft"),
                account_id,
                thread_id,
                to: str_list(p, "to")?.unwrap_or(dto),
                cc: str_list(p, "cc")?.unwrap_or(dcc),
                subject: opt_s(p, "subject").unwrap_or(dsubject),
                body,
                state: "draft".into(),
                origin: if caller.is_app {
                    "app".into()
                } else {
                    "agent".into()
                },
                created: now,
                updated: now,
                attachments: vec![],
            };
            let specs = attachment_specs(p)?;
            let d = st.create_draft(&d)?;
            for a in &specs {
                // All or nothing: a draft missing one of its files would go out incomplete.
                if let Err(e) = st.add_draft_attachment(&d.id, a) {
                    let _ = st.delete_draft(&d.id);
                    return Err(e.into());
                }
            }
            let d = st.draft(&d.id)?;
            let _ = svc.events.send(Event::DraftsChanged);
            to_value(d)
        }
        "drafts.attach" => {
            let id = s(p, "id")?;
            let specs = attachment_specs(p)?;
            let specs = if specs.is_empty() {
                vec![attachment_spec(p)?]
            } else {
                specs
            };
            for a in &specs {
                st.add_draft_attachment(&id, a)?;
            }
            let _ = svc.events.send(Event::DraftsChanged);
            to_value(st.draft(&id)?)
        }
        "drafts.detach" => {
            let id = s(p, "id")?;
            st.remove_draft_attachment(&id, &s(p, "attachment_id")?)?;
            let _ = svc.events.send(Event::DraftsChanged);
            to_value(st.draft(&id)?)
        }
        "drafts.update" => {
            let d = st.update_draft(
                &s(p, "id")?,
                str_list(p, "to")?,
                str_list(p, "cc")?,
                opt_s(p, "subject"),
                opt_s(p, "body"),
            )?;
            let _ = svc.events.send(Event::DraftsChanged);
            to_value(d)
        }
        "drafts.delete" => {
            st.delete_draft(&s(p, "id")?)?;
            let _ = svc.events.send(Event::DraftsChanged);
            Ok(json!({}))
        }
        "drafts.send" => {
            let d = st.draft(&s(p, "id")?)?;
            if d.state == "sent" {
                return Err(RpcError::invalid(format!(
                    "draft {} was already sent",
                    d.id
                )));
            }
            let agents_may_send = st.setting("agents_may_send")?.as_deref() == Some("true");
            if caller.is_app || agents_may_send {
                svc.worker(&d.account_id)?
                    .sync
                    .send_draft(&d, opt_s(p, "after_status").as_deref())
                    .await?;
                return Ok(json!({ "sent": true }));
            }
            let d = st.move_to_review(&d.id)?;
            let number = match &d.thread_id {
                Some(t) => {
                    let _ = svc.events.send(Event::ThreadsChanged(vec![t.clone()]));
                    svc.kick(&d.account_id);
                    Some(st.thread(t)?.number)
                }
                None => None,
            };
            let _ = svc.events.send(Event::DraftsChanged);
            Err(RpcError {
                code: APPROVAL_REQUIRED,
                message: format!(
                    "approval_required: draft {} is waiting for the user to approve it in Mermoz. Tell the user; do not retry.",
                    d.id
                ),
                data: Some(json!({ "draft": d.id, "thread": number })),
            })
        }
        "review.approve" => {
            app_only(caller, method)?;
            let d = st.draft(&s(p, "id")?)?;
            if d.state != "review" {
                return Err(RpcError::invalid(format!(
                    "draft {} is not waiting for review",
                    d.id
                )));
            }
            svc.worker(&d.account_id)?
                .sync
                .send_draft(&d, opt_s(p, "after_status").as_deref())
                .await?;
            Ok(json!({ "sent": true }))
        }
        "review.reject" => {
            app_only(caller, method)?;
            let id = s(p, "id")?;
            let d = st.draft(&id)?;
            st.reject_draft(&id)?;
            if let Some(t) = d.thread_id {
                let _ = svc.events.send(Event::ThreadsChanged(vec![t]));
                svc.kick(&d.account_id);
            }
            let _ = svc.events.send(Event::DraftsChanged);
            Ok(json!({}))
        }
        "providers.list" => {
            let list: Vec<Value> = st
                .providers()?
                .into_iter()
                .map(|pr| {
                    let has_key = pr.auth == "oauth"
                        || mermoz_core::tokens::load_provider_key(&pr.id).is_ok();
                    let mut v = serde_json::to_value(&pr).unwrap_or_default();
                    v["has_key"] = json!(has_key);
                    v
                })
                .collect();
            Ok(json!(list))
        }
        "statuses.reorder" => {
            let keys = str_list(p, "keys")?.ok_or_else(|| RpcError::invalid("missing 'keys'"))?;
            st.reorder_statuses(&keys)?;
            to_value(st.statuses()?)
        }
        "router.get" => Ok(router_state(svc)),
        "router.set" => {
            // The keys pay for every routed thread, and the provider decides where mail
            // is sent: only the app may change them.
            app_only(caller, method)?;
            let provider = opt_provider(p, "provider")?;
            let key_for = opt_provider(p, "key_provider")?
                .or(provider)
                .unwrap_or_else(|| svc.provider());
            let cap = p.get("daily_cap");
            if cap.is_none() && provider.is_none() && p.get("api_key").is_none() {
                return Err(RpcError::invalid(
                    "missing 'api_key' (string, or null to remove), 'provider' or 'daily_cap'",
                ));
            }
            if let Some(cap) = cap {
                let cap = cap
                    .as_i64()
                    .filter(|c| *c >= 0)
                    .ok_or_else(|| RpcError::invalid("'daily_cap' must be a number ≥ 0"))?;
                st.set_setting("jev_daily_cap", &cap.to_string())?;
                svc.resume_all();
            }
            match p.get("api_key") {
                Some(Value::String(k)) if !k.trim().is_empty() => {
                    mermoz_core::jev::save_key(key_for, k)?;
                    svc.resume(key_for);
                    svc.set_error(key_for, None);
                }
                Some(Value::Null) | Some(Value::String(_)) => {
                    let _ = mermoz_core::jev::delete_key(key_for);
                    svc.set_error(key_for, None);
                }
                Some(_) => {
                    return Err(RpcError::invalid(
                        "'api_key' must be a string, or null to remove",
                    ));
                }
                None => {}
            }
            if let Some(provider) = provider {
                st.set_jev_provider(provider)?;
                tracing::info!(provider = provider.id(), "instant sorting provider picked");
            }
            svc.router_kick.notify_one();
            svc.feed_kick.notify_one();
            svc.grouping_kick.notify_one();
            Ok(router_state(svc))
        }
        "router.test" => {
            let provider = opt_provider(p, "provider")?.unwrap_or_else(|| svc.provider());
            let key = opt_s(p, "api_key")
                .or_else(|| mermoz_core::jev::load_key(provider))
                .ok_or_else(|| RpcError::invalid(format!("No {} key to test.", provider.name())))?;
            let t = std::time::Instant::now();
            let q = json!({ "q": { "type": "noul", "instructions": "Is this an email about a meeting?" } });
            mermoz_core::jev::Client::new(provider, key)
                .ask(&json!({ "subject": "Lunch on Friday?" }), &q)
                .await?;
            Ok(json!({ "ok": true, "ms": t.elapsed().as_millis() as i64 }))
        }
        "providers.save" => {
            app_only(caller, method)?;
            let existing =
                opt_s(p, "id").and_then(|id| st.providers().ok()?.into_iter().find(|x| x.id == id));
            let pr = mermoz_core::agent::Provider {
                auth: existing
                    .as_ref()
                    .map(|e| e.auth.clone())
                    .unwrap_or_else(|| "key".into()),
                profile: existing.as_ref().and_then(|e| e.profile.clone()),
                id: opt_s(p, "id").unwrap_or_else(|| new_id("prv")),
                name: s(p, "name")?,
                kind: s(p, "kind")?,
                base_url: opt_s(p, "base_url").filter(|u| !u.trim().is_empty()),
                model: s(p, "model")?,
            };
            st.save_provider(&pr)?;
            if let Some(key) = opt_s(p, "api_key").filter(|k| !k.trim().is_empty()) {
                mermoz_core::tokens::save_provider_key(&pr.id, key.trim())?;
            }
            svc.kick_agents();
            to_value(pr)
        }
        "providers.delete" => {
            app_only(caller, method)?;
            let id = s(p, "id")?;
            if st
                .providers()?
                .iter()
                .any(|x| x.id == id && x.kind == "chatgpt")
            {
                mermoz_core::chatgpt::sign_out(&id).await?;
            }
            if let Some(profile) = st
                .providers()?
                .into_iter()
                .find(|x| x.id == id)
                .and_then(|x| x.profile)
            {
                mermoz_core::ant::logout(&profile).await?;
            }
            st.delete_provider(&id)?;
            mermoz_core::tokens::delete_provider_key(&id)?;
            Ok(json!({}))
        }
        "providers.test" => {
            let id = s(p, "id")?;
            let pr = st
                .providers()?
                .into_iter()
                .find(|x| x.id == id)
                .ok_or_else(|| RpcError::from(Error::NotFound(format!("provider {id}"))))?;
            let key = crate::service::credential(&pr).await?;
            let sample = "Hi Marc,\n\nThanks, see you Thursday.\n\nBest,\nLéa\n--\nLéa Dubois · Head of Ops · Acme\n+33 6 12 34 56 78";
            match mermoz_core::agent::Client::new()
                .digest(
                    &pr,
                    &key,
                    "Léa Dubois <lea@acme.example>",
                    "Thursday",
                    sample,
                )
                .await
            {
                Ok(d) => Ok(json!({ "ok": true, "clean_body": d.clean_body, "card": d.card })),
                Err(e) => Ok(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        "providers.login_anthropic" => {
            app_only(caller, method)?;
            svc.login_anthropic()?;
            Ok(json!({ "started": true }))
        }
        "providers.login_chatgpt" => {
            app_only(caller, method)?;
            Ok(json!({ "auth_url": svc.login_chatgpt().await? }))
        }
        "providers.models" => {
            let id = s(p, "id")?;
            let pr = st
                .providers()?
                .into_iter()
                .find(|x| x.id == id)
                .ok_or_else(|| RpcError::from(Error::NotFound(format!("provider {id}"))))?;
            if pr.kind != "chatgpt" {
                return Ok(json!([]));
            }
            let token = crate::service::credential(&pr).await?;
            let models: Vec<Value> = mermoz_core::chatgpt::models(&token)
                .await?
                .into_iter()
                .map(|(slug, name)| json!({ "id": slug, "name": name }))
                .collect();
            Ok(json!(models))
        }
        "providers.login_status" => {
            Ok(serde_json::to_value(svc.login.lock().unwrap().clone()).unwrap_or_default())
        }
        "accounts.agent.get" => {
            let acc = st.account(&s(p, "account_id")?)?;
            let (done, pending, failed) =
                st.digest_stats(&acc.id, crate::service::backfill_since(st))?;
            Ok(json!({
                "provider_ids": st.account_providers(&acc.id)?,
                "digest_enabled": st.setting(&format!("digest:{}", acc.id))?.as_deref() == Some("true"),
                "done": done, "pending": pending, "failed": failed,
                "error": svc.agent_error(&acc.id),
            }))
        }
        "accounts.agent.set" => {
            app_only(caller, method)?;
            let acc = st.account(&s(p, "account_id")?)?;
            if let Some(ids) = str_list(p, "provider_ids")? {
                st.set_account_providers(&acc.id, &ids)?;
            }
            if let Some(on) = p.get("digest_enabled").and_then(|v| v.as_bool()) {
                st.set_setting(
                    &format!("digest:{}", acc.id),
                    if on { "true" } else { "false" },
                )?;
            }
            svc.kick_agents();
            Ok(json!({}))
        }
        "contacts.list" => {
            let limit = p
                .get("limit")
                .and_then(|v| v.as_i64())
                .unwrap_or(500)
                .clamp(1, 10000);
            to_value(st.people(opt_s(p, "query").as_deref(), limit)?)
        }
        "contacts.get" => {
            let person = st.person(&s(p, "email")?)?;
            let (threads, _) = st.threads(&ThreadFilter {
                participants: Some(person.emails.clone()),
                limit: 50,
                ..Default::default()
            })?;
            Ok(json!({ "contact": person, "threads": threads }))
        }
        "agent.people_clean" => Ok(json!({ "total": svc.clean_people()? })),
        "agent.people_status" => {
            Ok(serde_json::to_value(svc.people_job.lock().unwrap().clone()).unwrap_or_default())
        }
        "agent.clean" => {
            let id = st.resolve_thread_id(&s(p, "id")?)?;
            let (account, _) = st.provider_thread_id(&id)?;
            let pr = st.provider_for_account(&account)?.ok_or_else(|| {
                RpcError::invalid("Allow an AI for this account in Settings › Agents first.")
            })?;
            let key = crate::service::credential(&pr).await?;
            let client = mermoz_core::agent::Client::new();
            let mut cleaned = 0;
            // `missing_only` (the Optimized view) never pays twice for a message.
            let missing_only = p
                .get("missing_only")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // `message_id` limits the clean to the one message the reader is looking at.
            let only = p.get("message_id").and_then(|v| v.as_str());
            for (mid, from_name, from_email, subject, body) in st
                .thread_digest_inputs(&id, missing_only)?
                .into_iter()
                .filter(|(mid, ..)| only.is_none_or(|o| o == mid))
            {
                let from = if from_name.is_empty() {
                    from_email
                } else {
                    format!("{from_name} <{from_email}>")
                };
                let outcome = client.digest(&pr, &key, &from, &subject, &body).await;
                if let Err(e) = &outcome
                    && matches!(e, Error::Http(_) | Error::Upstream { .. })
                {
                    return Err(RpcError::invalid(e.to_string()));
                }
                st.save_digest(&mid, &pr.id, &outcome)?;
                cleaned += outcome.is_ok() as i64;
                // Each message shows up as soon as it is ready.
                let _ = svc.events.send(Event::ThreadsChanged(vec![id.clone()]));
            }
            let _ = svc.events.send(Event::ThreadsChanged(vec![id]));
            let _ = svc.events.send(Event::ContactsChanged);
            Ok(json!({ "cleaned": cleaned }))
        }
        "suggestions.summary" => {
            let statuses =
                str_list(p, "statuses")?.ok_or_else(|| RpcError::invalid("missing 'statuses'"))?;
            let scope = column_scope(p);
            let mut columns = vec![];
            for s in &statuses {
                let mut c = serde_json::to_value(st.column_suggestions(s, &scope)?)
                    .map_err(|e| RpcError::new(-32000, e.to_string()))?;
                // Nothing sorts whole columns any more; zero progress keeps older apps' bars quiet.
                c["classified"] = json!(0);
                c["total"] = json!(0);
                c["sorted"] = json!(false);
                columns.push(c);
            }
            Ok(json!({
                "columns": columns,
                "sorting": false,
                "paused": svc.paused(),
                "provider_name": svc.provider().name(),
                "calls_today": st.background_calls_today()?,
                "daily_cap": st.background_daily_cap()?,
                "resort": null,
            }))
        }
        "suggestions.resort" => {
            // Whole columns are no longer sorted at once; only stopping is left: it ends
            // a pass recorded by an older daemon and clears what it suggested.
            if !p.get("stop").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Err(RpcError::invalid(
                    "Sorting whole columns at once is turned off; pass {\"stop\": true} to clear a previous run.",
                ));
            }
            let cleared = st.clear_machine_suggestions()?;
            tracing::info!(
                cleared,
                "whole-mailbox sort stopped; unreviewed suggestions cleared"
            );
            let _ = svc.events.send(Event::SuggestionsChanged);
            let _ = svc.events.send(Event::ThreadsChanged(vec![]));
            Ok(json!({ "stopped": true, "cleared": cleared }))
        }
        "threads.suggest" => {
            let mut ids = vec![];
            for id in str_list(p, "ids")?.unwrap_or_default() {
                ids.push(st.resolve_thread_id(&id)?);
            }
            to_value(svc.suggest(&ids).await?)
        }
        "learned.list" => Ok(json!({ "learned": st.learned(200)? })),
        "learned.forget" => {
            let item: mermoz_core::store::Learned =
                serde_json::from_value(p.get("item").cloned().unwrap_or(Value::Null))
                    .map_err(|e| RpcError::invalid(format!("bad 'item': {e}")))?;
            let forgotten = st.forget_learned(&item)?;
            let _ = svc.events.send(Event::FeedChanged);
            Ok(json!({ "forgotten": forgotten }))
        }
        "sort.answer" => {
            let id = st.resolve_thread_id(&s(p, "id")?)?;
            let answer = match s(p, "answer")?.as_str() {
                "yes" => SortAnswer::Yes,
                "no" => SortAnswer::No,
                "change" => SortAnswer::Change {
                    status: opt_s(p, "status"),
                    projects: str_list(p, "projects")?.unwrap_or_default(),
                },
                other => {
                    return Err(RpcError::invalid(format!(
                        "answer must be yes, no or change, not '{other}'"
                    )));
                }
            };
            if let SortAnswer::Change {
                status: None,
                projects,
            } = &answer
                && projects.is_empty()
            {
                return Err(RpcError::invalid("change needs a 'status' or 'projects'"));
            }
            let sender = st.thread_sender(&id)?;
            let had_rule = st.sender_rule(&sender)?.is_some();
            let undo = st.answer_sort(&id, &answer)?;
            // A rule that this answer just made is told once, in plain words.
            let learned = match st.sender_rule(&sender)? {
                Some(g) if !had_rule => {
                    let name = st.thread(&id)?.last_from;
                    let mut to: Vec<String> = g.status.into_iter().collect();
                    to.extend(g.projects);
                    Some(format!(
                        "Learned: mail from {name} goes to {}",
                        to.join(" · ")
                    ))
                }
                _ => None,
            };
            let _ = svc.events.send(Event::ThreadsChanged(vec![id.clone()]));
            if let Ok((acc, _)) = st.provider_thread_id(&id) {
                svc.kick(&acc);
            }
            Ok(json!({ "undo": undo, "learned": learned }))
        }
        "sort.undo" => {
            let undo: SortUndo =
                serde_json::from_value(p.get("undo").cloned().unwrap_or(Value::Null))
                    .map_err(|e| RpcError::invalid(format!("bad 'undo': {e}")))?;
            st.undo_sort(&undo)?;
            let _ = svc
                .events
                .send(Event::ThreadsChanged(vec![undo.thread.id.clone()]));
            if let Ok((acc, _)) = st.provider_thread_id(&undo.thread.id) {
                svc.kick(&acc);
            }
            Ok(json!({}))
        }
        "suggestions.clear" => {
            let cleared = st.clear_machine_suggestions()?;
            tracing::info!(cleared, "unreviewed suggestions cleared");
            let _ = svc.events.send(Event::SuggestionsChanged);
            let _ = svc.events.send(Event::ThreadsChanged(vec![]));
            Ok(json!({ "cleared": cleared }))
        }
        "suggestions.accept" => {
            let status = s(p, "status")?;
            let target: Option<SuggestionGroup> = match p.get("target") {
                None | Some(Value::Null) => None,
                Some(t) => Some(
                    serde_json::from_value(t.clone())
                        .map_err(|e| RpcError::invalid(format!("bad 'target': {e}")))?,
                ),
            };
            let before = st.accept_column(&status, &column_scope(p), target.as_ref())?;
            let ids: Vec<String> = before.iter().map(|t| t.id.clone()).collect();
            let _ = svc.events.send(Event::ThreadsChanged(ids));
            let _ = svc.events.send(Event::SuggestionsChanged);
            svc.kick_all();
            Ok(json!({ "accepted": before.len(), "before": before }))
        }
        "threads.restore" => {
            let before: Vec<ThreadState> = serde_json::from_value(
                p.get("threads").cloned().unwrap_or(Value::Null),
            )
            .map_err(|e| RpcError::invalid(format!("'threads' must be thread snapshots: {e}")))?;
            let n = st.restore_threads(&before)?;
            let _ = svc.events.send(Event::ThreadsChanged(
                before.iter().map(|t| t.id.clone()).collect(),
            ));
            let _ = svc.events.send(Event::SuggestionsChanged);
            svc.kick_all();
            Ok(json!({ "restored": n }))
        }
        "chat.get" => {
            let id = st.resolve_thread_id(&s(p, "thread_id")?)?;
            let (account, _) = st.provider_thread_id(&id)?;
            let research = st
                .provider_for_account(&account)?
                .is_some_and(|pr| mermoz_core::chat::can_research(&pr));
            Ok(json!({ "messages": st.chat_messages(&id)?, "research_available": research }))
        }
        "chat.send" => {
            let id = st.resolve_thread_id(&s(p, "thread_id")?)?;
            let research = p.get("research").and_then(|v| v.as_bool()).unwrap_or(true);
            match crate::chat::send(svc, &id, &s(p, "text")?, research).await {
                Ok(m) => to_value(m),
                Err(Error::Upstream {
                    status: 401 | 403, ..
                }) => Err(RpcError::invalid(
                    "Your AI sign-in expired. Sign in again in Settings › Agents.",
                )),
                // A provider failure reads as one plain sentence in the chat panel.
                Err(e @ (Error::Http(_) | Error::Upstream { .. })) => {
                    tracing::warn!(thread = %id, "chat failed: {e}");
                    Err(RpcError::invalid(
                        "Your AI couldn't answer right now. Try again in a moment.",
                    ))
                }
                Err(e) => Err(e.into()),
            }
        }
        "chat.clear" => {
            let id = st.resolve_thread_id(&s(p, "thread_id")?)?;
            Ok(json!({ "deleted": st.clear_chat(&id)? }))
        }
        "feed.list" => Ok(crate::feed::list(svc, &column_scope(p))?),
        "do.status" => Ok(crate::bulk::status(svc)?),
        "do.ask" => {
            let origin = if caller.is_app { "app" } else { "agent" };
            match crate::bulk::ask(svc, &s(p, "text")?, origin).await {
                Ok(v) => Ok(v),
                Err(Error::Upstream {
                    status: 401 | 403, ..
                }) => Err(RpcError::invalid(
                    "Your AI sign-in expired. Sign in again in Settings › Agents.",
                )),
                Err(e @ (Error::Http(_) | Error::Upstream { .. })) => {
                    tracing::warn!("do.ask failed: {e}");
                    Err(RpcError::invalid(
                        "Your AI couldn't answer right now. Try again in a moment.",
                    ))
                }
                Err(e) => Err(e.into()),
            }
        }
        "do.preview" => {
            let origin = if caller.is_app { "app" } else { "agent" };
            let plan = p
                .get("plan")
                .ok_or_else(|| RpcError::invalid("missing 'plan'"))?;
            let text = opt_s(p, "text").unwrap_or_default();
            Ok(crate::bulk::preview_json(svc, &text, plan, origin)?)
        }
        "do.run" => {
            let id = s(p, "id")?;
            let confirm = p.get("confirm").and_then(|v| v.as_i64());
            Ok(crate::bulk::run(svc, &id, confirm).await?)
        }
        "do.undo" => Ok(crate::bulk::undo(svc, &s(p, "id")?)?),
        "do.get" => to_value(st.bulk_op(&s(p, "id")?)?),
        "do.list" => {
            let limit = p.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
            to_value(st.bulk_ops(limit)?)
        }
        "feed.texts" => {
            let ids = str_list(p, "ids")?.ok_or_else(|| RpcError::invalid("missing 'ids'"))?;
            Ok(json!({ "queued": crate::feed::request_texts(svc, &ids)? }))
        }
        "feed.snooze" => {
            let ids = str_list(p, "ids")?.ok_or_else(|| RpcError::invalid("missing 'ids'"))?;
            let ids: Vec<String> = ids
                .iter()
                .map(|i| st.resolve_thread_id(i))
                .collect::<Result<_, _>>()?;
            let until = p
                .get("until")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| RpcError::invalid("missing 'until' (Unix milliseconds)"))?;
            let n = st.snooze(&ids, until, opt_s(p, "label").as_deref())?;
            let _ = svc.events.send(Event::FeedChanged);
            let _ = svc.events.send(Event::ThreadsChanged(ids));
            svc.later_kick.notify_one();
            Ok(json!({ "snoozed": n }))
        }
        "later.options" => Ok(crate::feed::later_options()),
        "later.parse" => Ok(crate::feed::later_parse(&s(p, "text")?)),
        "feed.learn_archive" => {
            let ids = str_list(p, "ids")?.ok_or_else(|| RpcError::invalid("missing 'ids'"))?;
            let ids: Vec<String> = ids
                .iter()
                .map(|i| st.resolve_thread_id(i))
                .collect::<Result<_, _>>()?;
            st.learn_archive(&ids)?;
            let _ = svc.events.send(Event::FeedChanged);
            Ok(json!({ "learned": ids.len() }))
        }
        "feed.unsnooze" => {
            let ids = str_list(p, "ids")?.ok_or_else(|| RpcError::invalid("missing 'ids'"))?;
            let ids: Vec<String> = ids
                .iter()
                .map(|i| st.resolve_thread_id(i))
                .collect::<Result<_, _>>()?;
            let n = st.unsnooze(&ids)?;
            let _ = svc.events.send(Event::FeedChanged);
            Ok(json!({ "unsnoozed": n }))
        }
        "settings.get" => Ok(
            json!({ "agents_may_send": st.setting("agents_may_send")?.as_deref() == Some("true") }),
        ),
        "settings.set" => {
            app_only(caller, method)?;
            let v = p
                .get("agents_may_send")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| RpcError::invalid("missing bool 'agents_may_send'"))?;
            st.set_setting("agents_may_send", if v { "true" } else { "false" })?;
            Ok(json!({}))
        }
        "feedback.submit" => {
            let text = s(p, "text")?;
            if text.trim().is_empty() {
                return Err(RpcError::invalid("feedback text is empty"));
            }
            Ok(
                json!({ "id": st.add_feedback(&text, opt_s(p, "command").as_deref(), opt_s(p, "agent").as_deref())? }),
            )
        }
        "feedback.list" => to_value(st.feedback()?),
        "sync.now" => {
            svc.kick_all();
            Ok(json!({}))
        }
        m if m.starts_with("drive.") => crate::drive::dispatch(svc, caller, m, p).await,
        _ => Err(RpcError::new(-32601, format!("method not found: {method}"))),
    }
}

/// `suggested`: true for any pending suggestion, or `{"status", "projects"}` for one destination.
fn suggested_filter(p: &Value) -> Result<Option<SuggestedFilter>, RpcError> {
    match p.get("suggested") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(None),
        Some(Value::Bool(true)) => Ok(Some(SuggestedFilter::Any)),
        Some(g) => {
            let mut g = g.clone();
            // A destination is named by status and projects; the count is irrelevant here.
            if let Some(o) = g.as_object_mut() {
                o.entry("projects").or_insert(json!([]));
            }
            serde_json::from_value::<SuggestionGroup>(g)
                .map(|g| Some(SuggestedFilter::To(g)))
                .map_err(|e| RpcError::invalid(format!("bad 'suggested': {e}")))
        }
    }
}

/// The board or list a column belongs to: an optional project and account.
fn column_scope(p: &Value) -> ColumnScope {
    ColumnScope {
        project: opt_s(p, "project"),
        account_id: opt_s(p, "account_id"),
    }
}

/// A System One provider id (`typesafe`, `subq`) under `key`, when given.
fn opt_provider(p: &Value, key: &str) -> Result<Option<mermoz_core::jev::Provider>, RpcError> {
    opt_s(p, key)
        .map(|id| {
            mermoz_core::jev::Provider::parse(&id).ok_or_else(|| {
                RpcError::invalid(format!("unknown '{key}' {id:?}: use typesafe or subq"))
            })
        })
        .transpose()
}

/// Instant sorting as Settings shows it: the picked provider at the top level, and
/// each provider's key, last error, hold and calls today.
fn router_state(svc: &Service) -> Value {
    use mermoz_core::jev::{self, Provider};
    let active = svc.provider();
    let providers: Vec<Value> = Provider::ALL
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id(),
                "name": p.name(),
                "model": p.model(),
                "configured": jev::load_key(p).is_some(),
                "error": svc.last_error(p),
                "paused": svc.paused_on(p),
                "calls_today": svc.store.provider_calls_today(p).unwrap_or(0),
            })
        })
        .collect();
    json!({
        "provider": active.id(),
        "provider_name": active.name(),
        "configured": jev::load_key(active).is_some(),
        "error": svc.last_error(active),
        "model": active.model(),
        "daily_cap": svc.store.background_daily_cap().unwrap_or(jev::DEFAULT_DAILY_CAP),
        "calls_today": svc.store.background_calls_today().unwrap_or(0),
        "paused": svc.paused(),
        "providers": providers,
        "offer": svc.offer(),
    })
}
