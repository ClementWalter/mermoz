//! Gmail sync engine: resumable newest-first import, history-based incremental
//! sync, and the offline write-back queue (labels, read state).

use crate::compose::{self, Outgoing};
use crate::error::{Error, Result};
use crate::gmail::{Gmail, decode_raw};
use crate::mime;
use crate::model::Draft;
use crate::store::{IncomingMessage, Store, SyncMode, ThreadPatch};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

mod push;

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
    ContactsChanged,
    ProvidersChanged,
    /// Background sorting progressed: column suggestions or counts changed.
    SuggestionsChanged,
    /// An account's Drive connection, a file pick or a saved attachment changed.
    DriveChanged {
        account_id: String,
    },
    /// Feed judgments or card words changed.
    FeedChanged,
    /// The answer streaming into a thread's chat so far (whole text, actions block hidden).
    ChatDelta {
        thread_id: String,
        text: String,
    },
    /// An "Ask Mermoz to do" run changed `done` of its `total` threads so far.
    DoProgress {
        id: String,
        done: i64,
        total: i64,
    },
}

pub struct AccountSync {
    pub store: Arc<Store>,
    pub gmail: Arc<Gmail>,
    pub account_id: String,
    pub email: String,
    pub events: broadcast::Sender<Event>,
}

/// How long a send waits for its sent copy before answering; the sync loop brings it
/// otherwise.
const SENT_COPY_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Fetch fan-out per batch; keeps us under Gmail's per-user quota (250 units/s, 5 per get).
const FETCH_CONCURRENCY: usize = 25;

fn sync_limit() -> Option<i64> {
    std::env::var("MERMOZ_SYNC_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
}

/// One message Gmail's own search found, as the fallback lists it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GmailHit {
    pub account_id: String,
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub subject: String,
    pub date: String,
    /// Already stored locally (e.g. a word local search can't see, like a Gmail operator).
    pub in_mermoz: bool,
}

impl AccountSync {
    /// Gmail's own search, for mail Mermoz doesn't hold (not synced yet, or in folders
    /// it doesn't import): each hit with its sender, subject and date.
    pub async fn search_gmail(&self, q: &str, max: u32) -> Result<Vec<GmailHit>> {
        let found = self.gmail.search_messages(q, max).await?;
        let mut out = vec![];
        for r in found.messages {
            let headers = self
                .gmail
                .headers(&r.id, &["From", "Subject", "Date"])
                .await?;
            let get = |name: &str| {
                headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            out.push(GmailHit {
                account_id: self.account_id.clone(),
                in_mermoz: self.store.has_message(&self.account_id, &r.id)?,
                from: get("From"),
                subject: get("Subject"),
                date: get("Date"),
                id: r.id,
                thread_id: r.thread_id,
            });
        }
        Ok(out)
    }

    /// Stores messages found by [`Self::search_gmail`]; returns their threads.
    pub async fn pull(&self, ids: &[String]) -> Result<Vec<String>> {
        let msgs = self
            .fetch_many(ids.iter().map(|i| (i.clone(), String::new())).collect())
            .await?;
        let threads =
            self.store
                .ingest_many(&self.account_id, &self.email, &msgs, SyncMode::Incremental)?;
        let threads = dedup(threads);
        let _ = self.events.send(Event::ThreadsChanged(threads.clone()));
        Ok(threads)
    }

    pub async fn refresh_labels(&self) -> Result<()> {
        let labels = self.gmail.labels().await?;
        let pairs: Vec<(String, String)> = labels.into_iter().map(|l| (l.id, l.name)).collect();
        self.store.set_labels(&self.account_id, &pairs)
    }

    /// Fetches full messages concurrently. Transient failures are retried; if any
    /// message still can't be fetched the call fails, so callers never advance
    /// their cursor or history id past mail they don't have.
    async fn fetch_many(&self, ids: Vec<(String, String)>) -> Result<Vec<IncomingMessage>> {
        let mut out = vec![];
        let mut pending: Vec<String> = ids.into_iter().map(|(id, _)| id).collect();
        let mut delay = std::time::Duration::from_secs(1);
        for round in 0..4 {
            let mut failed = vec![];
            let mut last_err = None;
            for chunk in pending.chunks(FETCH_CONCURRENCY) {
                let mut set = tokio::task::JoinSet::new();
                for id in chunk {
                    let g = self.gmail.clone();
                    let id = id.clone();
                    set.spawn(async move { (id.clone(), g.get_raw(&id).await) });
                }
                while let Some(res) = set.join_next().await {
                    let Ok((id, res)) = res else { continue };
                    match res {
                        Ok(m) => match to_incoming(m) {
                            Some(i) => out.push(i),
                            None => debug!(id, "skipping unparsable message"),
                        },
                        // Deleted between listing and fetching: nothing to import.
                        Err(Error::Upstream { status: 404, .. }) => {}
                        Err(e) => {
                            failed.push(id);
                            last_err = Some(e);
                        }
                    }
                }
            }
            if failed.is_empty() {
                return Ok(out);
            }
            if round == 3 {
                return Err(last_err.unwrap_or_else(|| Error::other("fetch failed")));
            }
            warn!(count = failed.len(), "retrying failed fetches");
            tokio::time::sleep(delay).await;
            delay *= 2;
            pending = failed;
        }
        Ok(out)
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
                let msgs = self.fetch_many(chunk.to_vec()).await?;
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

    /// Applies everything that changed since the last history id; returns how many
    /// threads changed. Label changes the store already shows (the echo of our own
    /// pushes) are skipped without asking Gmail for the message.
    pub async fn incremental(&self) -> Result<usize> {
        let Some(start) = self.store.history_id(&self.account_id)? else {
            return Ok(0);
        };
        let mut page_token: Option<String> = None;
        let mut added: Vec<(String, String)> = vec![];
        let mut relabeled: BTreeMap<String, LabelEdits> = BTreeMap::new();
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
                return Ok(0);
            };
            for rec in &page.history {
                for m in &rec.messages_added {
                    added.push((m.message.id.clone(), m.message.thread_id.clone()));
                }
                for (m, on) in rec
                    .labels_added
                    .iter()
                    .map(|m| (m, true))
                    .chain(rec.labels_removed.iter().map(|m| (m, false)))
                {
                    relabeled
                        .entry(m.message.id.clone())
                        .or_default()
                        .record(&m.label_ids, on);
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
        let mut relabeled_ids = vec![];
        let mut echoes = 0usize;
        for (id, edits) in relabeled {
            match self.store.message_label_ids(&self.account_id, &id)? {
                None => {}
                Some(stored) if edits.shown_by(&stored) => echoes += 1,
                Some(_) => relabeled_ids.push(id),
            }
        }
        if echoes > 0 {
            debug!(account = %self.email, echoes, "label changes already applied");
        }
        let relabeled = relabeled_ids;
        if !new.is_empty() || !relabeled.is_empty() {
            // Labels created on another device must be known before ingesting.
            self.refresh_labels().await?;
        }
        if !new.is_empty() {
            let msgs = self.fetch_many(new).await?;
            changed.extend(self.store.ingest_many(
                &self.account_id,
                &self.email,
                &msgs,
                SyncMode::Incremental,
            )?);
        }
        for id in relabeled {
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
        let changed = dedup(changed);
        let n = changed.len();
        if n > 0 {
            let _ = self.events.send(Event::ThreadsChanged(changed));
        }
        Ok(n)
    }

    async fn label_ids(&self, names: &BTreeSet<String>) -> Result<Vec<String>> {
        let mut ids = vec![];
        for n in names {
            if push::SYSTEM_LABELS.contains(&n.as_str()) {
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

    /// Verifies senders of messages imported before verification existed, from the
    /// `Authentication-Results` header Gmail kept; returns how many were checked.
    pub async fn verify_senders(&self, items: &[(String, String, String)]) -> Result<usize> {
        let mut n = 0;
        for (id, provider_id, from) in items {
            let headers = self
                .gmail
                .headers(provider_id, &["Authentication-Results"])
                .await?;
            let results: Vec<String> = headers
                .into_iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("authentication-results"))
                .map(|(_, v)| v)
                .collect();
            self.store
                .set_sender_auth(id, crate::auth::verdict(&results, from))?;
            n += 1;
        }
        Ok(n)
    }

    /// Reads the `Reply-To` and kind (bounce, auto-reply) of messages stored before they
    /// were kept: one metadata call each, plus the whole message for a bounce, to read
    /// what it reports. Returns how many were filled.
    pub async fn backfill_headers(&self, items: &[crate::store::MissingHeaders]) -> Result<usize> {
        const NAMES: &[&str] = &[
            "Reply-To",
            "Content-Type",
            "Auto-Submitted",
            "X-Autoreply",
            "X-Autorespond",
        ];
        let mut n = 0;
        for m in items {
            let headers = match self.gmail.headers(&m.provider_id, NAMES).await {
                Ok(h) => h,
                // Gone from Gmail: nothing to learn, and nothing to ask again.
                Err(Error::Upstream { status: 404, .. }) => vec![],
                Err(e) => return Err(e),
            };
            let get = |name: &str| {
                headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.clone())
            };
            let reply_to: Vec<_> = headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("reply-to"))
                .flat_map(|(_, v)| mime::header_addresses(v))
                .collect();
            let kind = crate::dsn::classify(&m.from, &m.subject, get);
            let dsn = if kind == crate::dsn::AutoKind::Bounce {
                match self.gmail.get_raw(&m.provider_id).await {
                    Ok(raw) => raw
                        .raw
                        .as_deref()
                        .and_then(|r| decode_raw(r).ok())
                        .and_then(|bytes| crate::dsn::parse(&bytes)),
                    Err(Error::Upstream { status: 404, .. }) => None,
                    Err(e) => return Err(e),
                }
            } else {
                None
            };
            self.store
                .set_headers(&m.id, &reply_to, kind, dsn.as_ref())?;
            n += 1;
        }
        Ok(n)
    }

    async fn push_label_rename(&self, payload: &serde_json::Value) -> Result<()> {
        let (from, to) = (
            payload["from"].as_str().unwrap_or_default(),
            payload["to"].as_str().unwrap_or_default(),
        );
        // The local cache already carries the new name; the server id is under either.
        let id = match self.store.label_id(&self.account_id, to)? {
            Some(id) => Some(id),
            None => self.store.label_id(&self.account_id, from)?,
        };
        let Some(id) = id else { return Ok(()) };
        match self.gmail.rename_label(&id, to).await {
            Ok(()) | Err(Error::Upstream { status: 404, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn push_label_delete(&self, payload: &serde_json::Value) -> Result<()> {
        let name = payload["name"].as_str().unwrap_or_default();
        let Some(id) = self.store.label_id(&self.account_id, name)? else {
            return Ok(());
        };
        match self.gmail.delete_label(&id).await {
            Ok(()) | Err(Error::Upstream { status: 404, .. }) => {
                self.store.remove_label(&self.account_id, &id)
            }
            Err(e) => Err(e),
        }
    }

    /// Sends a draft now; the thread then waits on the other side.
    pub async fn send_draft(&self, draft: &Draft, after_status: Option<&str>) -> Result<()> {
        // The message answered: the one the draft names, else the thread's reply target.
        let answered = match (&draft.reply_to_message_id, &draft.thread_id) {
            (Some(id), _) => Some(self.store.message(id)?),
            (None, Some(t)) => crate::reply::target(&self.store.messages(t)?).cloned(),
            (None, None) => None,
        };
        let (in_reply_to, references) = match &answered {
            Some(m) => self.store.reply_headers(&m.id)?,
            None => (None, vec![]),
        };
        let provider_thread = match &draft.thread_id {
            Some(t) => Some(self.store.provider_thread_id(t)?.1),
            None => None,
        };
        // A reply pinned to a message quotes it.
        let body = match (&draft.reply_to_message_id, &answered) {
            (Some(_), Some(m)) => compose::with_quote(&draft.body, m),
            _ => draft.body.clone(),
        };
        let mut files = Vec::new();
        let mut links = Vec::new();
        for a in &draft.attachments {
            match (a.kind, &a.path, &a.url) {
                (crate::model::AttachmentKind::File, Some(p), _) => {
                    let bytes = std::fs::read(p).map_err(|_| {
                        Error::InvalidParams(format!(
                            "{} is no longer available. Remove it and attach it again.",
                            a.filename
                        ))
                    })?;
                    files.push((a, bytes));
                }
                (crate::model::AttachmentKind::Link, _, Some(u)) => links.push(compose::Link {
                    title: &a.filename,
                    url: u,
                }),
                _ => {}
            }
        }
        let parts: Vec<compose::Part> = files
            .iter()
            .map(|(a, bytes)| compose::Part {
                filename: &a.filename,
                mime: &a.mime,
                bytes,
                inline: a.inline,
            })
            .collect();
        let raw = compose::build(&Outgoing {
            from: &self.email,
            to: &draft.to,
            cc: &draft.cc,
            subject: &draft.subject,
            body: &body,
            in_reply_to: in_reply_to.as_deref(),
            references: &references,
            parts: &parts,
            links: &links,
        })?;
        let sent = self.gmail.send(&raw, provider_thread.as_deref()).await?;
        self.store.set_draft_state(&draft.id, "sent")?;
        // The sent copy syncs back with its own attachments; these copies are spent.
        if let Err(e) = self.store.purge_draft_files(&draft.id) {
            debug!("could not remove sent draft files: {e}");
        }
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
        // Only the sent copy, briefly: a full history pull can take minutes while a
        // bulk relabel is being pushed, and the sender waits on this call.
        match tokio::time::timeout(SENT_COPY_WAIT, self.pull(std::slice::from_ref(&sent.id))).await
        {
            Ok(Err(e)) => debug!("sent copy not pulled: {e}"),
            Err(_) => debug!("sent copy not pulled in time; the next sync brings it"),
            Ok(Ok(_)) => {}
        }
        Ok(())
    }
}

/// One message's label changes in a history window: the last word on each label.
#[derive(Default)]
struct LabelEdits {
    last: BTreeMap<String, bool>,
    /// A change Gmail sent without its labels: the message must be fetched.
    unknown: bool,
}

impl LabelEdits {
    fn record(&mut self, labels: &[String], on: bool) {
        self.unknown |= labels.is_empty();
        for l in labels {
            self.last.insert(l.clone(), on);
        }
    }

    /// The stored labels already show every change: an echo of our own push, or of a
    /// change made here before Gmail reported it.
    fn shown_by(&self, stored: &BTreeSet<String>) -> bool {
        !self.unknown && self.last.iter().all(|(l, on)| stored.contains(l) == *on)
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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn set(labels: &[&str]) -> BTreeSet<String> {
        labels.iter().map(|l| l.to_string()).collect()
    }

    /// History of one message: (labels, added) in order.
    fn edits(changes: &[(Vec<&str>, bool)]) -> LabelEdits {
        let mut e = LabelEdits::default();
        for (labels, on) in changes {
            let labels: Vec<String> = labels.iter().map(|l| l.to_string()).collect();
            e.record(&labels, *on);
        }
        e
    }

    #[rstest]
    #[case::our_archive_echoed(vec![(vec!["INBOX"], false)], &["UNREAD"], true)]
    #[case::our_label_echoed(vec![(vec!["Label_1"], true)], &["INBOX", "Label_1"], true)]
    #[case::archived_elsewhere(vec![(vec!["INBOX"], false)], &["INBOX"], false)]
    #[case::labelled_elsewhere(vec![(vec!["Label_1"], true)], &["INBOX"], false)]
    #[case::last_change_wins(vec![(vec!["INBOX"], false), (vec!["INBOX"], true)], &["INBOX"], true)]
    #[case::labels_not_sent(vec![(vec![], true)], &["INBOX"], false)]
    #[case::our_trash_echoed(vec![(vec!["TRASH"], true), (vec!["INBOX"], false)], &["TRASH"], true)]
    #[case::our_untrash_echoed(vec![(vec!["TRASH"], false), (vec!["INBOX"], true)], &["INBOX"], true)]
    #[case::trashed_elsewhere(vec![(vec!["TRASH"], true)], &["INBOX"], false)]
    fn a_label_change_already_stored_is_an_echo(
        #[case] changes: Vec<(Vec<&str>, bool)>,
        #[case] stored: &[&str],
        #[case] echo: bool,
    ) {
        assert_eq!(edits(&changes).shown_by(&set(stored)), echo);
    }
}
