//! JSON-RPC method dispatch; see `docs/protocol.md` for the contract.

use crate::service::Service;
use mermoz_core::Error;
use mermoz_core::model::{Draft, new_id, now_ms};
use mermoz_core::sanitize::sanitize_html;
use mermoz_core::store::{ThreadFilter, ThreadPatch};
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

type R = Result<Value, RpcError>;

/// Who is calling: the app is trusted to send and to change the send gate.
#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub is_app: bool,
}

fn s(p: &Value, k: &str) -> Result<String, RpcError> {
    p.get(k)
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or_else(|| RpcError::invalid(format!("missing string param '{k}'")))
}

fn opt_s(p: &Value, k: &str) -> Option<String> {
    p.get(k).and_then(|v| v.as_str()).map(|v| v.to_string())
}

fn str_list(p: &Value, k: &str) -> Result<Option<Vec<String>>, RpcError> {
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

/// `Some(None)` means "clear": the key is present with null.
fn nullable_s(p: &Value, k: &str) -> Option<Option<String>> {
    match p.get(k) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(v.as_str().map(|s| s.to_string())),
    }
}

fn app_only(c: Caller, method: &str) -> Result<(), RpcError> {
    if c.is_app {
        Ok(())
    } else {
        Err(RpcError::new(
            APP_ONLY,
            format!("app_only: {method} can only be done by the user in Mermoz"),
        ))
    }
}

fn to_value<T: serde::Serialize>(v: T) -> R {
    serde_json::to_value(v).map_err(|e| RpcError::new(-32000, e.to_string()))
}

pub async fn dispatch(svc: &Arc<Service>, caller: Caller, method: &str, p: &Value) -> R {
    let st = &svc.store;
    match method {
        "hello" => Ok(json!({ "version": env!("CARGO_PKG_VERSION") })),
        "accounts.list" => to_value(st.accounts()?),
        "accounts.add_gmail" => Ok(json!({ "auth_url": svc.add_gmail().await? })),
        "accounts.remove" => {
            let acc = st.account(&s(p, "id")?)?;
            svc.stop_account(&acc.id);
            st.remove_account(&acc.id)?;
            mermoz_core::tokens::delete(&acc.email)?;
            let _ = svc.events.send(Event::AccountsChanged);
            Ok(json!({}))
        }
        "statuses.list" => to_value(st.statuses()?),
        "projects.list" => to_value(st.projects()?),
        "projects.create" => {
            to_value(st.create_project(&s(p, "name")?, opt_s(p, "color").as_deref())?)
        }
        "counts" => Ok(st.counts()?),
        "threads.list" => {
            let priority = match p.get("priority") {
                Some(Value::Array(a)) => Some(a.iter().filter_map(|v| v.as_i64()).collect()),
                _ => None,
            };
            let f = ThreadFilter {
                status: str_list(p, "status")?,
                project_id: opt_s(p, "project_id"),
                account_id: opt_s(p, "account_id"),
                priority,
                limit: p
                    .get("limit")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(200)
                    .min(2000),
                offset: p.get("offset").and_then(|v| v.as_i64()).unwrap_or(0),
            };
            let (threads, total) = st.threads(&f)?;
            Ok(json!({ "threads": threads, "total": total }))
        }
        "threads.get" => {
            let id = st.resolve_thread_id(&s(p, "id")?)?;
            Ok(json!({ "thread": st.thread(&id)?, "messages": st.messages(&id)? }))
        }
        "threads.update" => {
            let ids = str_list(p, "ids")?.ok_or_else(|| RpcError::invalid("missing 'ids'"))?;
            let ids: Vec<String> = ids
                .iter()
                .map(|i| st.resolve_thread_id(i))
                .collect::<Result<_, _>>()?;
            let patch = ThreadPatch {
                status: opt_s(p, "status"),
                priority: p.get("priority").and_then(|v| v.as_i64()),
                project_id: nullable_s(p, "project_id"),
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
        "threads.mark_read" => {
            let id = st.resolve_thread_id(&s(p, "id")?)?;
            let read = p.get("read").and_then(|v| v.as_bool()).unwrap_or(true);
            st.mark_read(&id, read)?;
            let _ = svc.events.send(Event::ThreadsChanged(vec![id.clone()]));
            if let Ok((acc, _)) = st.provider_thread_id(&id) {
                svc.kick(&acc);
            }
            Ok(json!({}))
        }
        "messages.html" => {
            let html = st.message_html(&s(p, "id")?)?.unwrap_or_default();
            let full = p.get("full").and_then(|v| v.as_bool()).unwrap_or(false);
            let (html, has_quoted) = if full {
                (html, false)
            } else {
                mermoz_core::quote::split_html(&html)
            };
            Ok(json!({ "html": sanitize_html(&html), "has_quoted": has_quoted }))
        }
        "search" => {
            let limit = p
                .get("limit")
                .and_then(|v| v.as_i64())
                .unwrap_or(50)
                .clamp(1, 500);
            Ok(json!({ "threads": st.search(&s(p, "query")?, limit)? }))
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
            };
            let d = st.create_draft(&d)?;
            let _ = svc.events.send(Event::DraftsChanged);
            to_value(d)
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
        _ => Err(RpcError::new(-32601, format!("method not found: {method}"))),
    }
}
