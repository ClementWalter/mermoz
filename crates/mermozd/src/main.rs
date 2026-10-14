//! mermozd: the Mermoz background daemon. Owns sync and the local store and
//! serves the app, CLI and MCP clients over a Unix socket.

mod bulk;
mod chat;
mod drive;
mod feed;
mod grouping;
mod peer;
mod rpc;
mod semantic;
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
        Event::DriveChanged { account_id } => {
            json!({ "kind": "drive_changed", "account_id": account_id })
        }
        Event::FeedChanged => json!({ "kind": "feed_changed" }),
        Event::ChatDelta { thread_id, text } => {
            json!({ "kind": "chat_delta", "thread_id": thread_id, "text": text })
        }
        Event::DoProgress { id, done, total } => {
            json!({ "kind": "do_progress", "id": id, "done": done, "total": total })
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

/// Moves HTML still stored in message rows to its own table, in short writes spaced
/// out so a sync or a click never waits behind it.
async fn move_html(store: Arc<Store>) {
    // The app's first list and counts read the disk alone.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let (t0, mut after, mut batches) = (std::time::Instant::now(), 0i64, 0u64);
    loop {
        let st = store.clone();
        match tokio::task::spawn_blocking(move || st.move_html_batch(after, 25)).await {
            Ok(Ok(Some(last))) => (after, batches) = (last, batches + 1),
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return warn!("moving message HTML: {e}"),
            Err(e) => return warn!("moving message HTML: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
    if batches > 0 {
        info!(
            batches,
            ms = t0.elapsed().as_millis() as u64,
            "message HTML moved out of the rows"
        );
    }
    index_addresses(store).await;
}

/// Lists the addresses of mail stored before the recipient check had its own index,
/// 2,000 messages per short write.
async fn index_addresses(store: Arc<Store>) {
    let (t0, mut after, mut batches) = (std::time::Instant::now(), 0i64, 0u64);
    loop {
        let st = store.clone();
        match tokio::task::spawn_blocking(move || st.index_addresses_batch(after, 2000)).await {
            Ok(Ok(Some(next))) => (after, batches) = (next, batches + 1),
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return warn!("indexing addresses: {e}"),
            Err(e) => return warn!("indexing addresses: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
    if batches > 0 {
        info!(
            batches,
            ms = t0.elapsed().as_millis() as u64,
            "addresses indexed for the recipient check"
        );
    }
    read_bodies(store).await;
}

/// Works out the bubble text of mail stored before it was kept at ingest (or by an
/// older reading pipeline), 200 messages per short write; until then a thread reads
/// it on the fly.
async fn read_bodies(store: Arc<Store>) {
    let (t0, mut after, mut batches) = (std::time::Instant::now(), 0i64, 0u64);
    loop {
        let st = store.clone();
        match tokio::task::spawn_blocking(move || st.read_bodies_batch(after, 200)).await {
            Ok(Ok(Some(last))) => (after, batches) = (last, batches + 1),
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return warn!("reading message bodies: {e}"),
            Err(e) => return warn!("reading message bodies: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
    if batches > 0 {
        info!(
            batches,
            ms = t0.elapsed().as_millis() as u64,
            "message bodies read for the chat view"
        );
    }
}

/// Copies the WAL into the database once a second after writes, so no commit pays for
/// a checkpoint while holding the write lock.
async fn checkpoints(store: Arc<Store>) {
    let second = std::time::Duration::from_secs(1);
    // Not at launch: the first list and counts own the disk.
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + second, second);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let st = store.clone();
        if let Ok(Err(e)) = tokio::task::spawn_blocking(move || st.checkpoint()).await {
            warn!("checkpoint: {e}");
        }
    }
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
    // The feed only reads the store and Jev, so it also runs on demo data.
    feed::start(&svc);
    semantic::start(&svc);
    tokio::spawn(move_html(svc.store.clone()));
    tokio::spawn(checkpoints(svc.store.clone()));
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
    // Metal aborts the process at exit if the search model still holds GPU buffers.
    svc.semantic.unload();
    info!("mermozd stopped");
    Ok(())
}
