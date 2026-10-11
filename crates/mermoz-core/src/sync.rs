//! Gmail sync engine: resumable newest-first import, history-based incremental
//! sync, and the offline write-back queue (labels, read state).

use crate::compose::{self, Outgoing};
use crate::error::{Error, Result};
use crate::gmail::{Gmail, decode_raw};
use crate::mime;
use crate::model::Draft;
use crate::store::{IncomingMessage, LABEL_PREFIX, Store, SyncMode, ThreadPatch};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Notifications fanned out to subscribed clients.
#[derive(Debug, Clone)]
pub enum Event {
    ThreadsChanged(Vec<String>),
    AccountsChanged,
    SyncProgress {
        account_id: String,
        synced: i64,
        total: i64,
    },
    DraftsChanged,
}

pub struct AccountSync {
    pub store: Arc<Store>,
    pub gmail: Arc<Gmail>,
    pub account_id: String,
    pub email: String,
    pub events: broadcast::Sender<Event>,
}

/// Fetch fan-out per batch; keeps us under Gmail's per-user quota (250 units/s, 5 per get).
const FETCH_CONCURRENCY: usize = 25;

fn sync_limit() -> Option<i64> {
    std::env::var("MERMOZ_SYNC_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
}

impl AccountSync {
    pub async fn refresh_labels(&self) -> Result<()> {
        let labels = self.gmail.labels().await?;
        let pairs: Vec<(String, String)> = labels.into_iter().map(|l| (l.id, l.name)).collect();
        self.store.set_labels(&self.account_id, &pairs)
    }

    async fn fetch_many(&self, ids: Vec<(String, String)>) -> Vec<IncomingMessage> {
        let mut out = vec![];
        for chunk in ids.chunks(FETCH_CONCURRENCY) {
            let mut set = tokio::task::JoinSet::new();
            for (id, _) in chunk {
                let g = self.gmail.clone();
                let id = id.clone();
                set.spawn(async move { g.get_raw(&id).await });
            }
            while let Some(res) = set.join_next().await {
                match res {
                    Ok(Ok(m)) => match to_incoming(m) {
                        Some(i) => out.push(i),
                        None => debug!("skipping unparsable message"),
                    },
                    Ok(Err(e)) => warn!("fetch failed: {e}"),
                    Err(e) => warn!("fetch task failed: {e}"),
                }
            }
        }
        out
    }

    /// Imports the mailbox newest-first, page by page, resuming from the saved cursor.
    /// Recent mail is usable within seconds while older mail keeps streaming in.
    pub async fn initial_sync(&self) -> Result<()> {
        let profile = self.gmail.profile().await?;
        if self.store.history_id(&self.account_id)?.is_none() {
            // Captured before listing so nothing that arrives meanwhile is missed.
            self.store
                .set_history_id(&self.account_id, &profile.history_id)?;
        }
        let total = profile.messages_total.unwrap_or(0);
        let limit = sync_limit();
        let mut synced = self
            .store
            .accounts()?
            .into_iter()
            .find(|a| a.id == self.account_id)
            .map(|a| a.sync.synced)
            .unwrap_or(0);
        self.store.set_sync(
            &self.account_id,
            "syncing",
            None,
            Some(limit.map(|l| l.min(total)).unwrap_or(total)),
            None,
        )?;
        let mut cursor = self.store.cursor(&self.account_id)?;
        loop {
            let page = self.gmail.list_messages(cursor.as_deref()).await?;
            let todo: Vec<(String, String)> = page
                .messages
                .iter()
                .filter(|r| {
                    !self
                        .store
                        .has_message(&self.account_id, &r.id)
                        .unwrap_or(false)
                })
                .map(|r| (r.id.clone(), r.thread_id.clone()))
                .collect();
            synced += (page.messages.len() - todo.len()) as i64;
            for chunk in todo.chunks(100) {
                let msgs = self.fetch_many(chunk.to_vec()).await;
                let ids = self.store.ingest_many(
                    &self.account_id,
                    &self.email,
                    &msgs,
                    SyncMode::Initial,
                )?;
                synced += chunk.len() as i64;
                self.store
                    .set_sync(&self.account_id, "syncing", Some(synced), None, None)?;
                let _ = self.events.send(Event::ThreadsChanged(dedup(ids)));
                let _ = self.events.send(Event::SyncProgress {
                    account_id: self.account_id.clone(),
                    synced,
                    total,
                });
            }
            cursor = page.next_page_token;
            self.store.set_cursor(&self.account_id, cursor.as_deref())?;
            let reached_limit = limit.is_some_and(|l| synced >= l);
            if cursor.is_none() || reached_limit {
                break;
            }
        }
        self.store.set_cursor(&self.account_id, None)?;
        self.store
            .set_sync(&self.account_id, "idle", Some(synced), None, None)?;
        let _ = self.events.send(Event::AccountsChanged);
        info!(account = %self.email, synced, "initial sync complete");
        Ok(())
    }

    /// Applies everything that changed since the last history id.
    pub async fn incremental(&self) -> Result<()> {
        let Some(start) = self.store.history_id(&self.account_id)? else {
            return Ok(());
        };
        let mut page_token: Option<String> = None;
        let mut added: Vec<(String, String)> = vec![];
        let mut relabeled: BTreeSet<String> = BTreeSet::new();
        let mut latest = start.clone();
        loop {
            let Some(page) = self.gmail.history(&start, page_token.as_deref()).await? else {
                warn!(account = %self.email, "history expired, rescanning");
                let profile = self.gmail.profile().await?;
                self.store
                    .set_history_id(&self.account_id, &profile.history_id)?;
                self.store.set_cursor(&self.account_id, None)?;
                self.store
                    .set_sync(&self.account_id, "initial", None, None, None)?;
                return Ok(());
            };
            for rec in &page.history {
                for m in &rec.messages_added {
                    added.push((m.message.id.clone(), m.message.thread_id.clone()));
                }
                for m in rec.labels_added.iter().chain(rec.labels_removed.iter()) {
                    relabeled.insert(m.message.id.clone());
                }
            }
            if let Some(h) = page.history_id {
                latest = h;
            }
            page_token = page.next_page_token;
            if page_token.is_none() {
                break;
            }
        }
        let mut changed = vec![];
        let new: Vec<(String, String)> = added
            .into_iter()
            .filter(|(id, _)| !self.store.has_message(&self.account_id, id).unwrap_or(true))
            .collect();
        if !new.is_empty() || !relabeled.is_empty() {
            // Labels created on another device must be known before ingesting.
            self.refresh_labels().await?;
        }
        if !new.is_empty() {
            let msgs = self.fetch_many(new).await;
            changed.extend(self.store.ingest_many(
                &self.account_id,
                &self.email,
                &msgs,
                SyncMode::Incremental,
            )?);
        }
        for id in relabeled {
            if !self.store.has_message(&self.account_id, &id)? {
                continue;
            }
            match self.gmail.get_minimal(&id).await {
                Ok(m) => {
                    let inc = IncomingMessage {
                        provider_id: m.id,
                        provider_thread_id: m.thread_id,
                        label_ids: m.label_ids,
                        internal_date: 0,
                        parsed: Default::default(),
                    };
                    changed.push(self.store.ingest(
                        &self.account_id,
                        &self.email,
                        &inc,
                        SyncMode::Incremental,
                    )?);
                }
                Err(Error::Upstream { status: 404, .. }) => {}
                Err(e) => return Err(e),
            }
        }
        self.store.set_history_id(&self.account_id, &latest)?;
        if !changed.is_empty() {
            let _ = self.events.send(Event::ThreadsChanged(dedup(changed)));
        }
        Ok(())
    }

    async fn label_ids(&self, names: &BTreeSet<String>) -> Result<Vec<String>> {
        let mut ids = vec![];
        for n in names {
            if n == "INBOX" || n == "UNREAD" {
                ids.push(n.clone());
                continue;
            }
            let id = match self.store.label_id(&self.account_id, n)? {
                Some(id) => id,
                None => {
                    let l = match self.gmail.create_label(n).await {
                        Ok(l) => l,
                        // Already exists remotely but not in our cache: refresh and look again.
                        Err(Error::Upstream { status: 409, .. }) => {
                            self.refresh_labels().await?;
                            let id = self
                                .store
                                .label_id(&self.account_id, n)?
                                .ok_or_else(|| Error::other(format!("label {n} missing")))?;
                            ids.push(id);
                            continue;
                        }
                        Err(e) => return Err(e),
                    };
                    self.store
                        .set_labels(&self.account_id, &[(l.id.clone(), l.name.clone())])?;
                    l.id
                }
            };
            ids.push(id);
        }
        Ok(ids)
    }

    /// Pushes queued local edits to Gmail; failures stay queued for the next round.
    pub async fn push_ops(&self) -> Result<usize> {
        let ops = self.store.pending_ops(200)?;
        let mut done = 0;
        for op in ops.into_iter().filter(|o| o.account_id == self.account_id) {
            let res = match op.kind.as_str() {
                "labels" => self.push_labels(&op.key).await,
                "read" => {
                    let read = op.payload["read"].as_bool().unwrap_or(true);
                    self.push_read(&op.key, read).await
                }
                other => Err(Error::other(format!("unknown op {other}"))),
            };
            match res {
                Ok(()) => {
                    self.store.op_done(&op)?;
                    done += 1;
                }
                Err(Error::NotFound(_)) => self.store.op_done(&op)?,
                Err(e) => {
                    warn!(op = op.id, "push failed: {e}");
                    self.store.op_failed(&op, &e.to_string())?;
                    if matches!(e, Error::Http(_)) {
                        // Offline: stop this round, everything stays queued.
                        break;
                    }
                }
            }
        }
        Ok(done)
    }

    async fn push_labels(&self, thread_id: &str) -> Result<()> {
        let (_, provider_thread) = self.store.provider_thread_id(thread_id)?;
        let desired = self.store.thread_desired_labels(thread_id)?;
        let current = self
            .store
            .thread_current_labels(&self.account_id, thread_id)?;
        let add: BTreeSet<String> = desired.difference(&current).cloned().collect();
        let remove: BTreeSet<String> = current
            .iter()
            .filter(|n| *n == "INBOX" || n.starts_with(LABEL_PREFIX))
            .filter(|n| !desired.contains(*n))
            .cloned()
            .collect();
        if add.is_empty() && remove.is_empty() {
            return Ok(());
        }
        let add_ids = self.label_ids(&add).await?;
        let remove_ids = self.label_ids(&remove).await?;
        self.gmail
            .modify_thread(&provider_thread, &add_ids, &remove_ids)
            .await?;
        self.store
            .apply_label_delta(thread_id, &add_ids, &remove_ids)
    }

    async fn push_read(&self, thread_id: &str, read: bool) -> Result<()> {
        let (_, provider_thread) = self.store.provider_thread_id(thread_id)?;
        let unread = vec!["UNREAD".to_string()];
        if read {
            self.gmail
                .modify_thread(&provider_thread, &[], &unread)
                .await?;
            self.store.apply_label_delta(thread_id, &[], &unread)
        } else {
            self.gmail
                .modify_thread(&provider_thread, &unread, &[])
                .await?;
            self.store.apply_label_delta(thread_id, &unread, &[])
        }
    }

    /// Sends a draft now; the thread then waits on the other side.
    pub async fn send_draft(&self, draft: &Draft, after_status: Option<&str>) -> Result<()> {
        let (in_reply_to, references, provider_thread) = match &draft.thread_id {
            Some(t) => {
                let (irt, refs) = self.store.reply_headers(t)?;
                let (_, pt) = self.store.provider_thread_id(t)?;
                (irt, refs, Some(pt))
            }
            None => (None, vec![], None),
        };
        let raw = compose::build(&Outgoing {
            from: &self.email,
            to: &draft.to,
            cc: &draft.cc,
            subject: &draft.subject,
            body: &draft.body,
            in_reply_to: in_reply_to.as_deref(),
            references: &references,
        })?;
        self.gmail.send(&raw, provider_thread.as_deref()).await?;
        self.store.set_draft_state(&draft.id, "sent")?;
        if let Some(t) = &draft.thread_id {
            let status = after_status.unwrap_or("waiting").to_string();
            self.store.update_threads(
                std::slice::from_ref(t),
                &ThreadPatch {
                    status: Some(status),
                    ..Default::default()
                },
            )?;
            let _ = self.events.send(Event::ThreadsChanged(vec![t.clone()]));
        }
        let _ = self.events.send(Event::DraftsChanged);
        // Pull the sent copy so the thread shows it immediately.
        if let Err(e) = self.incremental().await {
            debug!("post-send sync failed: {e}");
        }
        Ok(())
    }
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

fn to_incoming(m: crate::gmail::RawMessage) -> Option<IncomingMessage> {
    let raw = decode_raw(m.raw.as_deref()?).ok()?;
    let parsed = mime::parse(&raw)?;
    Some(IncomingMessage {
        provider_id: m.id,
        provider_thread_id: m.thread_id,
        label_ids: m.label_ids,
        internal_date: m.internal_date.and_then(|d| d.parse().ok()).unwrap_or(0),
        parsed,
    })
}
