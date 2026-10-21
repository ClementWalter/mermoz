//! Outlook sync through Microsoft Graph, the counterpart of the Gmail engine:
//!
//! - **Import**, newest first and resumable: the mailbox listing (`/me/messages`,
//!   newest first, its next link saved as the cursor), then one delta walk per
//!   imported folder (Inbox, Sent Items, Archive, the user's folders), which catches
//!   whatever changed meanwhile and leaves a delta link per folder, the Outlook
//!   counterpart of Gmail's history id.
//! - **Incremental**: each folder's delta link brings new mail, moves and read,
//!   category and flag changes. A message that left a folder is looked up once,
//!   unless it already showed up in another imported folder in the same pass; one
//!   gone for good counts as deleted. Changes whose labels the store already shows
//!   (the echo of our own write-backs) fetch nothing.
//! - **Write-back**: label changes become moves and patches (see
//!   [`crate::outlook::labels::plan_thread`]); threads are Outlook conversations.
//! - **Send**: the composed MIME goes to `sendMail` as it is; the copy Outlook files
//!   in Sent Items comes back with the next delta.

use super::{AccountSync, Event, dedup, sync_limit};
use crate::error::{Error, Result};
use crate::outlook::labels::{self, CATEGORY, FolderMap, WELL_KNOWN};
use crate::outlook::{MessageMeta, Outlook};
use crate::store::{IncomingMessage, Op, SyncMode};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Cursor value once the newest-first listing is done and the delta walks remain.
const DELTA_PHASE: &str = "outlook:delta";

/// History id once every imported folder has a delta link.
const DELTA_READY: &str = "outlook:delta";

/// How long the folder map is trusted before it is read again.
const FOLDERS_TTL: Duration = Duration::from_secs(900);

/// Newest Inbox messages checked on each poll while the import still runs.
const RECENT: usize = 25;

/// Messages fetched per batch; the client keeps four requests in flight.
const FETCH_BATCH: usize = 16;

impl AccountSync {
    /// The mailbox's folders, read again when older than [`FOLDERS_TTL`] or `force`d.
    /// User folders and categories become the account's labels.
    pub(super) async fn folder_map(&self, o: &Outlook, force: bool) -> Result<Arc<FolderMap>> {
        if !force
            && let Some((at, map)) = o.folder_cache.lock().unwrap().clone()
            && at.elapsed() < FOLDERS_TTL
        {
            return Ok(map);
        }
        let mut roles = HashMap::new();
        for (name, _) in WELL_KNOWN {
            if let Some(f) = o.well_known(name).await? {
                roles.insert(name.to_string(), f.id);
            }
        }
        let mut all = vec![];
        let mut queue = o.folders(None).await?;
        while let Some(f) = queue.pop() {
            if f.child_folder_count > 0 {
                queue.extend(o.folders(Some(&f.id)).await?);
            }
            all.push(f);
        }
        let map = Arc::new(FolderMap::build(roles, &all));
        let mut names = map.folder_labels();
        match o.categories().await {
            Ok(cats) => names.extend(cats.into_iter().map(|c| (format!("{CATEGORY}{c}"), c))),
            // Categories also arrive with the messages that carry them.
            Err(e) => debug!(account = %self.email, "categories: {e}"),
        }
        self.store.set_labels(&self.account_id, &names)?;
        *o.folder_cache.lock().unwrap() = Some((Instant::now(), map.clone()));
        Ok(map)
    }

    /// Registers the categories these messages carry as labels, so their names resolve.
    fn note_categories<'a>(&self, metas: impl Iterator<Item = &'a MessageMeta>) -> Result<()> {
        let cats: BTreeSet<&String> = metas.flat_map(|m| &m.categories).collect();
        if cats.is_empty() {
            return Ok(());
        }
        let pairs: Vec<(String, String)> = cats
            .into_iter()
            .map(|c| (format!("{CATEGORY}{c}"), c.clone()))
            .collect();
        self.store.set_labels(&self.account_id, &pairs)
    }

    /// Fetches the sources of these messages, a batch at a time. Transient failures are
    /// retried; a message still missing fails the call, so no cursor or delta link
    /// moves past mail Mermoz doesn't have. Messages deleted meanwhile are skipped.
    async fn outlook_fetch(
        &self,
        o: &Arc<Outlook>,
        metas: &[MessageMeta],
        map: &FolderMap,
    ) -> Result<Vec<IncomingMessage>> {
        let mut out = vec![];
        let mut pending: Vec<&MessageMeta> = metas.iter().collect();
        let mut delay = Duration::from_secs(1);
        for round in 0..4 {
            let mut failed = vec![];
            let mut last_err = None;
            for chunk in pending.chunks(FETCH_BATCH) {
                let mut set = tokio::task::JoinSet::new();
                for (i, m) in chunk.iter().enumerate() {
                    let (o, id) = (o.clone(), m.id.clone());
                    set.spawn(async move { (i, o.mime(&id).await) });
                }
                while let Some(res) = set.join_next().await {
                    let Ok((i, res)) = res else { continue };
                    let meta = chunk[i];
                    match res {
                        Ok(bytes) => match incoming(meta, &bytes, map) {
                            Some(m) => out.push(m),
                            None => debug!(id = %meta.id, "skipping unparsable message"),
                        },
                        // A refused sign-in never comes back by retrying.
                        Err(Error::SignInExpired) => return Err(Error::SignInExpired),
                        Err(Error::Upstream { status: 404, .. }) => {}
                        Err(e) => {
                            failed.push(meta);
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

    /// Fetches and stores messages not stored yet; returns their threads.
    async fn outlook_store_new(
        &self,
        o: &Arc<Outlook>,
        metas: &[MessageMeta],
        map: &FolderMap,
        mode: SyncMode,
    ) -> Result<Vec<String>> {
        if metas.is_empty() {
            return Ok(vec![]);
        }
        self.note_categories(metas.iter())?;
        let msgs = self.outlook_fetch(o, metas, map).await?;
        let ids = self
            .store
            .ingest_many(&self.account_id, &self.email, &msgs, mode)?;
        if mode == SyncMode::Initial {
            self.store
                .settle_imported(&self.account_id, &dedup(ids.clone()))?;
        }
        Ok(ids)
    }

    /// Stores the new labels of known messages whose labels changed; returns their
    /// threads. Changes the store already shows are echoes and change nothing.
    fn outlook_relabel(
        &self,
        metas: &[MessageMeta],
        map: &FolderMap,
        mode: SyncMode,
    ) -> Result<Vec<String>> {
        self.note_categories(metas.iter())?;
        let mut out = vec![];
        let mut echoes = 0usize;
        for m in metas {
            let labels = labels::labels_for(m, map);
            match self.store.message_label_ids(&self.account_id, &m.id)? {
                None => {}
                Some(stored) if stored == labels => echoes += 1,
                Some(_) => out.push(self.store.ingest(
                    &self.account_id,
                    &self.email,
                    &labels_only(m, labels),
                    mode,
                )?),
            }
        }
        if echoes > 0 {
            debug!(account = %self.email, echoes, "label changes already applied");
        }
        Ok(out)
    }

    /// Imports the mailbox newest first, then walks each imported folder's delta once,
    /// resuming from the saved cursor and links.
    pub(super) async fn outlook_initial_sync(&self, o: &Arc<Outlook>) -> Result<()> {
        let map = self.folder_map(o, false).await?;
        let total = map.total();
        let limit = sync_limit();
        let mut synced = self.store.account(&self.account_id)?.sync.synced;
        self.store.set_sync(
            &self.account_id,
            "syncing",
            None,
            Some(limit.map(|l| l.min(total)).unwrap_or(total)),
            None,
        )?;
        let mut cursor = self.store.cursor(&self.account_id)?;
        if cursor.as_deref() != Some(DELTA_PHASE) {
            loop {
                let page = match o.messages_page(cursor.as_deref()).await {
                    Ok(p) => p,
                    // A saved page link Graph no longer honours: list again from the top;
                    // what is stored already is skipped.
                    Err(Error::Upstream { status, .. })
                        if cursor.is_some() && (400..500).contains(&status) && status != 429 =>
                    {
                        warn!(account = %self.email, status, "listing link expired, listing again");
                        cursor = None;
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                let metas: Vec<MessageMeta> = page
                    .value
                    .into_iter()
                    .filter(|m| {
                        m.parent_folder_id
                            .as_deref()
                            .is_some_and(|f| map.is_synced(f))
                    })
                    .collect();
                let todo: Vec<MessageMeta> = metas
                    .iter()
                    .filter(|m| {
                        !self
                            .store
                            .has_message(&self.account_id, &m.id)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();
                synced += (metas.len() - todo.len()) as i64;
                for chunk in todo.chunks(100) {
                    let ids = self
                        .outlook_store_new(o, chunk, &map, SyncMode::Initial)
                        .await?;
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
                cursor = page.next_link;
                self.store.set_cursor(&self.account_id, cursor.as_deref())?;
                if cursor.is_none() || limit.is_some_and(|l| synced >= l) {
                    break;
                }
            }
            self.store.set_cursor(&self.account_id, Some(DELTA_PHASE))?;
        }
        let changed = self
            .delta_pass(o, SyncMode::Initial, limit.is_none())
            .await?;
        let _ = self.events.send(Event::ThreadsChanged(changed));
        self.store.set_history_id(&self.account_id, DELTA_READY)?;
        self.store.set_cursor(&self.account_id, None)?;
        self.store
            .set_sync(&self.account_id, "idle", Some(synced), None, None)?;
        let _ = self.events.send(Event::AccountsChanged);
        info!(account = %self.email, synced, "initial sync complete");
        Ok(())
    }

    /// Applies what changed since each folder's delta link; returns how many threads
    /// changed. While the import runs, only the newest Inbox mail is checked.
    pub(super) async fn outlook_incremental(&self, o: &Arc<Outlook>) -> Result<usize> {
        let changed = if self.store.history_id(&self.account_id)?.is_none() {
            self.recent_inbox(o).await?
        } else {
            self.delta_pass(o, SyncMode::Incremental, true).await?
        };
        let changed = dedup(changed);
        let n = changed.len();
        if n > 0 {
            let _ = self.events.send(Event::ThreadsChanged(changed));
        }
        Ok(n)
    }

    /// New Inbox mail during a long import: the newest few messages, stored if new.
    async fn recent_inbox(&self, o: &Arc<Outlook>) -> Result<Vec<String>> {
        let map = self.folder_map(o, false).await?;
        let Some(inbox) = map.role("inbox") else {
            return Ok(vec![]);
        };
        let new: Vec<MessageMeta> = o
            .recent(inbox, RECENT)
            .await?
            .into_iter()
            .filter(|m| {
                !self
                    .store
                    .has_message(&self.account_id, &m.id)
                    .unwrap_or(true)
            })
            .collect();
        self.outlook_store_new(o, &new, &map, SyncMode::Incremental)
            .await
    }

    /// Walks every imported folder's delta from its saved link (from the start when
    /// none, or when Graph expired it) and applies what it reports. During the import
    /// (`Initial`) each page's link is saved as it goes, so a restart resumes there.
    /// `fetch_new` false leaves mail not stored yet aside (a capped import).
    async fn delta_pass(
        &self,
        o: &Arc<Outlook>,
        mode: SyncMode,
        fetch_new: bool,
    ) -> Result<Vec<String>> {
        let mut map = self.folder_map(o, false).await?;
        let mut links = self.store.delta_links(&self.account_id)?;
        // Folders no longer imported (deleted, moved to Deleted Items) drop their link.
        let synced: Vec<String> = map.synced();
        for gone in links
            .keys()
            .filter(|f| !synced.contains(f))
            .cloned()
            .collect::<Vec<_>>()
        {
            self.store.remove_delta_link(&self.account_id, &gone)?;
            links.remove(&gone);
        }
        let mut changed_threads = vec![];
        let mut seen: HashSet<String> = HashSet::new();
        let mut removed: BTreeSet<String> = BTreeSet::new();
        let mut done_links: Vec<(String, String)> = vec![];
        for folder in synced {
            let mut link = links.get(&folder).cloned();
            let mut restarted = false;
            loop {
                let page = match o.delta(&folder, link.as_deref()).await {
                    Ok(Some(p)) => p,
                    Ok(None) if !restarted => {
                        warn!(account = %self.email, folder, "delta link expired, walking the folder again");
                        restarted = true;
                        link = None;
                        continue;
                    }
                    Ok(None) => return Err(Error::other("outlook: delta link refused twice")),
                    // Deleted since the folder map was read.
                    Err(Error::Upstream { status: 404, .. }) => {
                        self.store.remove_delta_link(&self.account_id, &folder)?;
                        break;
                    }
                    Err(e) => return Err(e),
                };
                let mut present = vec![];
                for m in page.value {
                    if m.removed.is_some() {
                        removed.insert(m.id);
                    } else {
                        seen.insert(m.id.clone());
                        present.push(m);
                    }
                }
                let (ids, refreshed) = self
                    .apply_present(o, &map, present, mode, fetch_new)
                    .await?;
                if let Some(m) = refreshed {
                    map = m;
                }
                changed_threads.extend(ids);
                match (page.delta_link, page.next_link) {
                    (Some(d), _) => {
                        if mode == SyncMode::Initial {
                            self.store.set_delta_link(&self.account_id, &folder, &d)?;
                        } else {
                            done_links.push((folder.clone(), d));
                        }
                        break;
                    }
                    (None, Some(next)) => {
                        if mode == SyncMode::Initial {
                            self.store
                                .set_delta_link(&self.account_id, &folder, &next)?;
                        }
                        link = Some(next);
                    }
                    (None, None) => break,
                }
            }
        }
        // Left a folder and seen in none: moved out of the imported folders, or deleted.
        let mut gone_to = vec![];
        for id in removed.into_iter().filter(|id| !seen.contains(id)) {
            if !self.store.has_message(&self.account_id, &id)? {
                continue;
            }
            match o.message(&id).await {
                Ok(m) => gone_to.push(m),
                Err(Error::Upstream { status: 404, .. }) => {
                    let trashed = BTreeSet::from(["TRASH".to_string()]);
                    let m = MessageMeta {
                        id: id.clone(),
                        ..Default::default()
                    };
                    changed_threads.push(self.store.ingest(
                        &self.account_id,
                        &self.email,
                        &labels_only(&m, trashed),
                        mode,
                    )?);
                }
                Err(e) => return Err(e),
            }
        }
        changed_threads.extend(self.outlook_relabel(&gone_to, &map, mode)?);
        for (folder, link) in done_links {
            self.store
                .set_delta_link(&self.account_id, &folder, &link)?;
        }
        Ok(dedup(changed_threads))
    }

    /// Stores what a delta page reports as present in a folder: new mail fetched,
    /// known mail relabelled. Entries Graph sent with some fields only are read again;
    /// a folder the map doesn't know yet reads the map again (returned when it did).
    async fn apply_present(
        &self,
        o: &Arc<Outlook>,
        map: &Arc<FolderMap>,
        present: Vec<MessageMeta>,
        mode: SyncMode,
        fetch_new: bool,
    ) -> Result<(Vec<String>, Option<Arc<FolderMap>>)> {
        let mut full = Vec::with_capacity(present.len());
        for m in present {
            if m.is_complete() {
                full.push(m);
                continue;
            }
            match o.message(&m.id).await {
                Ok(m) => full.push(m),
                Err(Error::Upstream { status: 404, .. }) => {}
                Err(e) => return Err(e),
            }
        }
        let unknown_folder = full
            .iter()
            .any(|m| m.parent_folder_id.as_deref().is_some_and(|f| !map.knows(f)));
        let refreshed = if unknown_folder {
            Some(self.folder_map(o, true).await?)
        } else {
            None
        };
        let map = refreshed.as_ref().unwrap_or(map);
        let (known, new): (Vec<MessageMeta>, Vec<MessageMeta>) = full.into_iter().partition(|m| {
            self.store
                .has_message(&self.account_id, &m.id)
                .unwrap_or(false)
        });
        let mut ids = self.outlook_relabel(&known, map, mode)?;
        if fetch_new {
            ids.extend(self.outlook_store_new(o, &new, map, mode).await?);
        }
        Ok((ids, refreshed))
    }

    /// Stores these messages by id (search hits, the sent copy); returns their threads.
    pub(super) async fn outlook_pull(
        &self,
        o: &Arc<Outlook>,
        ids: &[String],
    ) -> Result<Vec<String>> {
        let map = self.folder_map(o, false).await?;
        let mut metas = vec![];
        for id in ids {
            match o.message(id).await {
                Ok(m) => metas.push(m),
                Err(Error::Upstream { status: 404, .. }) => {}
                Err(e) => return Err(e),
            }
        }
        let threads = dedup(
            self.outlook_store_new(o, &metas, &map, SyncMode::Incremental)
                .await?,
        );
        let _ = self.events.send(Event::ThreadsChanged(threads.clone()));
        Ok(threads)
    }

    /// One label change (by names) for these threads, as moves and patches per message.
    /// Each thread's op is done or failed on its own.
    pub(super) async fn outlook_push_group(
        &self,
        o: &Arc<Outlook>,
        add: &BTreeSet<String>,
        remove: &BTreeSet<String>,
        ops: &[&Op],
        out: &mut super::push::Outcome,
    ) -> Result<()> {
        let map = match self.folder_map(o, false).await {
            Ok(m) => m,
            Err(e) => {
                for op in ops {
                    self.failed(op, &e, out)?;
                }
                return Ok(());
            }
        };
        for op in ops {
            if out.offline {
                return Ok(());
            }
            let mut messages = vec![];
            for id in self
                .store
                .provider_message_ids(std::slice::from_ref(&op.key))?
                .remove(&op.key)
                .unwrap_or_default()
            {
                if let Some(labels) = self.store.message_label_ids(&self.account_id, &id)? {
                    messages.push((id, labels));
                }
            }
            let edits = labels::plan_thread(&messages, add, remove, &map);
            let mut after = vec![];
            let mut error = None;
            for e in edits {
                match self.apply_edit(o, &e).await {
                    Ok(()) | Err(Error::Upstream { status: 404, .. }) => {
                        after.push((e.id.clone(), e.labels_after.clone()))
                    }
                    Err(err) => {
                        error = Some(err);
                        break;
                    }
                }
            }
            let cats: Vec<(String, String)> = after
                .iter()
                .flat_map(|(_, l)| l.iter())
                .filter_map(|l| l.strip_prefix(CATEGORY).map(|c| (l.clone(), c.to_string())))
                .collect();
            self.store.set_labels(&self.account_id, &cats)?;
            self.store.set_message_labels(&self.account_id, &after)?;
            match error {
                Some(e) => self.failed(op, &e, out)?,
                None => {
                    self.store.op_done(op)?;
                    out.pushed += 1;
                }
            }
        }
        Ok(())
    }

    async fn apply_edit(&self, o: &Outlook, e: &labels::Edit) -> Result<()> {
        if let Some(f) = &e.move_to {
            o.move_to(&e.id, f).await?;
        }
        if let Some(p) = &e.patch {
            o.update(&e.id, p).await?;
        }
        Ok(())
    }

    /// A project renamed here: its folder is renamed, or its category replaced on
    /// every message carrying it.
    pub(super) async fn outlook_label_rename(
        &self,
        o: &Arc<Outlook>,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let (from, to) = (
            payload["from"].as_str().unwrap_or_default(),
            payload["to"].as_str().unwrap_or_default(),
        );
        let id = match self.store.label_id(&self.account_id, to)? {
            Some(id) => Some(id),
            None => self.store.label_id(&self.account_id, from)?,
        };
        let Some(id) = id else { return Ok(()) };
        if let Some(folder) = FolderMap::folder_of_label(&id) {
            let leaf = to.rsplit('/').next().unwrap_or(to);
            match o.rename_folder(folder, leaf).await {
                Ok(()) | Err(Error::Upstream { status: 404, .. }) => {}
                Err(e) => return Err(e),
            }
            self.folder_map(o, true).await?;
            return Ok(());
        }
        let Some(old) = id.strip_prefix(CATEGORY) else {
            return Ok(());
        };
        let new_id = format!("{CATEGORY}{to}");
        self.recategorize(o, &id, |cats| {
            cats.remove(old);
            cats.insert(to.to_string());
        })
        .await?;
        self.store
            .set_labels(&self.account_id, &[(new_id.clone(), to.to_string())])?;
        if new_id != id {
            self.store.remove_label(&self.account_id, &id)?;
        }
        Ok(())
    }

    /// A project deleted here: the mail stays. Its category comes off every message;
    /// its folder's mail moves to Archive, then the emptied folder goes.
    pub(super) async fn outlook_label_delete(
        &self,
        o: &Arc<Outlook>,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let name = payload["name"].as_str().unwrap_or_default();
        let Some(id) = self.store.label_id(&self.account_id, name)? else {
            return Ok(());
        };
        if let Some(folder) = FolderMap::folder_of_label(&id) {
            let map = self.folder_map(o, false).await?;
            let Some(archive) = map.role("archive") else {
                return Err(Error::other(
                    "outlook: no Archive folder to empty the folder into",
                ));
            };
            let mut after = vec![];
            for mid in self.store.messages_with_label(&self.account_id, &id)? {
                match o.move_to(&mid, archive).await {
                    Ok(()) | Err(Error::Upstream { status: 404, .. }) => {}
                    Err(e) => return Err(e),
                }
                let mut labels = self
                    .store
                    .message_label_ids(&self.account_id, &mid)?
                    .unwrap_or_default();
                labels.remove(&id);
                after.push((mid, labels));
            }
            self.store.set_message_labels(&self.account_id, &after)?;
            match o.folder(folder).await {
                Ok(f) if f.total_item_count == 0 && f.child_folder_count == 0 => {
                    match o.delete_folder(folder).await {
                        Ok(()) | Err(Error::Upstream { status: 404, .. }) => {}
                        Err(e) => return Err(e),
                    }
                }
                Ok(_) => info!(account = %self.email, "folder kept: it still holds mail"),
                Err(Error::Upstream { status: 404, .. }) => {}
                Err(e) => return Err(e),
            }
            self.folder_map(o, true).await?;
        } else if let Some(cat) = id.strip_prefix(CATEGORY) {
            let cat = cat.to_string();
            self.recategorize(o, &id, |cats| {
                cats.remove(&cat);
            })
            .await?;
        }
        self.store.remove_label(&self.account_id, &id)
    }

    /// Rewrites the categories of every stored message carrying label `id`.
    async fn recategorize(
        &self,
        o: &Outlook,
        id: &str,
        change: impl Fn(&mut BTreeSet<String>),
    ) -> Result<()> {
        let mut after = vec![];
        for mid in self.store.messages_with_label(&self.account_id, id)? {
            let labels = self
                .store
                .message_label_ids(&self.account_id, &mid)?
                .unwrap_or_default();
            let mut cats: BTreeSet<String> = labels
                .iter()
                .filter_map(|l| l.strip_prefix(CATEGORY).map(String::from))
                .collect();
            change(&mut cats);
            match o
                .update(&mid, &serde_json::json!({ "categories": cats }))
                .await
            {
                Ok(()) | Err(Error::Upstream { status: 404, .. }) => {}
                Err(e) => return Err(e),
            }
            let mut new: BTreeSet<String> = labels
                .into_iter()
                .filter(|l| !l.starts_with(CATEGORY))
                .collect();
            new.extend(cats.iter().map(|c| format!("{CATEGORY}{c}")));
            after.push((mid, new));
        }
        self.store.set_message_labels(&self.account_id, &after)?;
        Ok(())
    }
}

/// A delta entry as a label-only update of a stored message.
fn labels_only(m: &MessageMeta, labels: BTreeSet<String>) -> IncomingMessage {
    IncomingMessage {
        provider_id: m.id.clone(),
        provider_thread_id: m.conversation_id.clone().unwrap_or_else(|| m.id.clone()),
        label_ids: labels.into_iter().collect(),
        internal_date: 0,
        parsed: Default::default(),
    }
}

/// A fetched message ready to store: threaded by its conversation, labelled by its
/// folder, categories, read state and flag.
fn incoming(m: &MessageMeta, raw: &[u8], map: &FolderMap) -> Option<IncomingMessage> {
    let parsed = crate::mime::parse(raw)?;
    Some(IncomingMessage {
        provider_id: m.id.clone(),
        provider_thread_id: m.conversation_id.clone().unwrap_or_else(|| m.id.clone()),
        label_ids: labels::labels_for(m, map).into_iter().collect(),
        internal_date: m.received_ms(),
        parsed,
    })
}

#[cfg(test)]
mod tests;
