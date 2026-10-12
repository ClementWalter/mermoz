//! mermozd: the Mermoz background daemon. Owns sync and the local store and
//! serves the app, CLI and MCP clients over a Unix socket.

mod chat;
mod grouping;
mod peer;
mod rpc;
mod service;

use mermoz_core::paths;
use mermoz_core::store::Store;
use mermoz_core::sync::Event;
use serde_json::{Value, json};
use service::Service;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tracing::{info, warn};

fn event_json(e: &Event) -> Value {
    let params = match e {
        Event::ThreadsChanged(ids) => json!({ "kind": "threads_changed", "ids": ids }),
        Event::AccountsChanged => json!({ "kind": "accounts_changed" }),
        Event::SyncProgress {
            account_id,
            synced,
            total,
        } => {
            json!({ "kind": "sync_progress", "account_id": account_id, "synced": synced, "total": total })
        }
        Event::DraftsChanged => json!({ "kind": "drafts_changed" }),
        Event::ContactsChanged => json!({ "kind": "contacts_changed" }),
        Event::ProvidersChanged => json!({ "kind": "providers_changed" }),
        Event::SuggestionsChanged => json!({ "kind": "suggestions_changed" }),
        Event::ChatDelta { thread_id, text } => {
            json!({ "kind": "chat_delta", "thread_id": thread_id, "text": text })
        }
    };
    json!({ "jsonrpc": "2.0", "method": "event", "params": params })
}

fn caller_of(stream: &UnixStream) -> rpc::Caller {
    let is_app = stream
        .peer_cred()
        .ok()
        .and_then(|c| c.pid())
        .and_then(peer::exe_path)
        .map(|p| peer::is_app(&p))
        .unwrap_or(false);
    rpc::Caller { is_app }
}

async fn handle(svc: Arc<Service>, stream: UnixStream) {
    let caller = caller_of(&stream);
    let (rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if wr.write_all(line.as_bytes()).await.is_err() || wr.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(rd).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("parse error: {e}") } }).to_string());
                continue;
            }
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = req
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let params = req.get("params").cloned().unwrap_or(json!({}));
        if method == "subscribe" {
            let mut events = svc.events.subscribe();
            let tx2 = tx.clone();
            tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(e) => {
                            if tx2.send(event_json(&e).to_string()).is_err() {
                                break;
                            }
                        }
                        // A slow client missed events: tell it to refresh everything.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let _ = tx2.send(event_json(&Event::AccountsChanged).to_string());
                        }
                        Err(_) => break,
                    }
                }
            });
            let _ = tx.send(json!({ "jsonrpc": "2.0", "id": id, "result": {} }).to_string());
            continue;
        }
        // Requests run concurrently, each answering when done (clients match answers by
        // id): a slow AI call must not hold back the quick ones queued behind it. The
        // inner task turns a panic into an error answer instead of a closed connection.
        let (svc, tx) = (svc.clone(), tx.clone());
        tokio::spawn(async move {
            let inner = {
                let (svc, method) = (svc.clone(), method.clone());
                tokio::spawn(async move { rpc::dispatch(&svc, caller, &method, &params).await })
            };
            let outcome = inner.await.unwrap_or_else(|e| {
                tracing::error!(%method, "request panicked: {e}");
                Err(rpc::RpcError {
                    code: -32603,
                    message: "Something went wrong on this item. It has been logged.".into(),
                    data: None,
                })
            });
            let resp = match outcome {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(e) => {
                    let mut err = json!({ "code": e.code, "message": e.message });
                    if let Some(d) = e.data {
                        err["data"] = d;
                    }
                    json!({ "jsonrpc": "2.0", "id": id, "error": err })
                }
            };
            let _ = tx.send(resp.to_string());
        });
    }
    drop(tx);
    let _ = writer.await;
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let sock = paths::socket_path();
    std::fs::create_dir_all(paths::home())?;
    if sock.exists() {
        if UnixStream::connect(&sock).await.is_ok() {
            info!("mermozd already running at {}", sock.display());
            return Ok(());
        }
        std::fs::remove_file(&sock)?;
    }
    let store = Arc::new(Store::open(&paths::db_path(), &paths::blobs_dir())?);
    let svc = Service::new(store);
    let listener = UnixListener::bind(&sock)?;
    // Only this user may talk to the daemon.
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    info!("mermozd listening on {}", sock.display());
    // MERMOZ_NO_SYNC serves the local store only (demo data, offline UI work).
    if std::env::var_os("MERMOZ_NO_SYNC").is_none() {
        svc.start_all();
        svc.start_router();
    }
    let shutdown = async {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    };
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => { tokio::spawn(handle(svc.clone(), stream)); }
                Err(e) => warn!("accept failed: {e}"),
            },
            _ = &mut shutdown => break,
        }
    }
    let _ = std::fs::remove_file(&sock);
    info!("mermozd stopped");
    Ok(())
}
