//! Search by meaning in the daemon: fetches the model on first use, keeps every
//! thread's vector current in the background (newest mail first), and fuses the
//! nearest threads with full-text hits by Reciprocal Rank Fusion.
//!
//! Until the model and the first vectors are ready, search is full-text only. The
//! user can turn it off (`smart_search` = "off"): the model is unloaded and nothing
//! more is indexed. `MERMOZ_EMBEDDER=stub` swaps in a word-hashing stand-in (tests,
//! demo data); `MERMOZ_EMBEDDER=off` never starts it.

pub mod download;
pub mod llama;

use crate::service::Service;
use mermoz_core::model::ThreadSummary;
use mermoz_core::semantic::{
    Embedder, Lane, StubEmbedder, VectorIndex, VectorStore, quantize, query_text, rrf,
};
use mermoz_core::store::Store;
use mermoz_core::sync::Event;
use mermoz_core::{Error, Result, paths};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tracing::{info, warn};

/// Setting that turns search by meaning off ("off") or on (anything else).
pub const SETTING: &str = "smart_search";

/// Threads embedded per batch: about half a second of background work.
const BATCH: usize = 16;

/// Nearest threads fused with full-text hits.
const NEIGHBOURS: usize = 20;

/// Cosine under which a thread is unrelated to the query. On a real mailbox, sought
/// threads scored 0.31–0.75 (median 0.49) and a nonsense word's best match 0.26–0.37.
const FLOOR: f32 = 0.30;

/// The model (~1 GB while in use) is freed after this long without a search or
/// indexing; it loads again in about a third of a second.
const IDLE: Duration = Duration::from_secs(120);

/// Share of the progress line the download takes; indexing takes the rest.
const DOWNLOAD_SHARE: f64 = 0.3;

/// What the settings line shows.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Status {
    pub enabled: bool,
    /// "off", "starting", "downloading", "indexing", "ready" or "error".
    pub phase: &'static str,
    /// 0–100 over download and indexing together.
    pub progress: u8,
    pub indexed: usize,
    pub total: usize,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            enabled: true,
            phase: "starting",
            progress: 0,
            indexed: 0,
            total: 0,
        }
    }
}

/// Which model runs, from `MERMOZ_EMBEDDER`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Real,
    Stub,
    Off,
}

fn kind() -> Kind {
    match std::env::var("MERMOZ_EMBEDDER").as_deref() {
        Ok("stub") => Kind::Stub,
        Ok("off") => Kind::Off,
        _ => Kind::Real,
    }
}

pub struct Semantic {
    status: Mutex<Status>,
    index: RwLock<VectorIndex>,
    model: RwLock<Option<Arc<dyn Embedder>>>,
    /// The model file once fetched and verified; the model loads from it on demand.
    path: Mutex<Option<std::path::PathBuf>>,
    /// One load at a time.
    loading: tokio::sync::Mutex<()>,
    /// Last search or indexing batch: the model is freed after [`IDLE`] without one.
    last_used: Mutex<Instant>,
    /// Wakes the indexer (setting changed).
    pub kick: Notify,
}

impl Default for Semantic {
    fn default() -> Self {
        Self {
            status: Default::default(),
            index: Default::default(),
            model: Default::default(),
            path: Default::default(),
            loading: Default::default(),
            last_used: Mutex::new(Instant::now()),
            kick: Default::default(),
        }
    }
}

/// Whether a search goes to the vectors too: field filters (`from:`) and fragments
/// under three letters are full-text only.
pub fn wants_vectors(q: &str) -> bool {
    !q.split_whitespace()
        .any(|w| w.starts_with("from:") || w.starts_with("to:") || w.starts_with("subject:"))
        && q.chars().filter(|c| c.is_alphanumeric()).count() >= 3
}

impl Semantic {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn set(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap());
    }

    /// Frees the model (GPU buffers included); the vectors stay in memory.
    pub fn unload(&self) {
        if self.model.write().unwrap().take().is_some() {
            info!("search model unloaded");
        }
    }

    /// The loaded model, marking it in use.
    fn model(&self) -> Option<Arc<dyn Embedder>> {
        let m = self.model.read().unwrap().clone();
        if m.is_some() {
            *self.last_used.lock().unwrap() = Instant::now();
        }
        m
    }

    /// The model, loading it from the fetched file if it was freed.
    async fn ensure_model(&self) -> Result<Arc<dyn Embedder>> {
        if let Some(m) = self.model() {
            return Ok(m);
        }
        let _one = self.loading.lock().await;
        if let Some(m) = self.model() {
            return Ok(m);
        }
        let m: Arc<dyn Embedder> = if kind() == Kind::Stub {
            Arc::new(StubEmbedder)
        } else {
            let path = self
                .path
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| Error::other("the search model isn't fetched yet"))?;
            let t = Instant::now();
            let m = tokio::task::spawn_blocking(move || llama::LlamaEmbedder::load(&path))
                .await
                .map_err(|e| Error::other(e.to_string()))??;
            info!(ms = t.elapsed().as_millis() as u64, "search model loaded");
            Arc::new(m)
        };
        *self.model.write().unwrap() = Some(m.clone());
        *self.last_used.lock().unwrap() = Instant::now();
        Ok(m)
    }

    /// Frees the model once nothing used it for [`IDLE`].
    fn unload_if_idle(&self) {
        if kind() == Kind::Real && self.last_used.lock().unwrap().elapsed() >= IDLE {
            self.unload();
        }
    }

    /// The threads nearest to `q`, closest first; None while off or not ready. A freed
    /// model is loaded first when `wait`, else in the background for the next search
    /// (a keystroke never waits on it).
    pub async fn nearest(self: &Arc<Self>, q: &str, k: usize, wait: bool) -> Option<Vec<String>> {
        if !self.status().enabled || self.index.read().unwrap().is_empty() {
            return None;
        }
        let model = match self.model() {
            Some(m) => m,
            None if wait => self
                .ensure_model()
                .await
                .map_err(|e| info!("search model not loaded: {e}"))
                .ok()?,
            None => {
                let me = self.clone();
                tokio::spawn(async move {
                    if let Err(e) = me.ensure_model().await {
                        info!("search model not loaded: {e}");
                    }
                });
                return None;
            }
        };
        let text = query_text(q);
        let v = tokio::task::spawn_blocking(move || model.embed(&[text], Lane::Query))
            .await
            .ok()?
            .map_err(|e| warn!("search by meaning: {e}"))
            .ok()?
            .pop()?;
        let near = self.index.read().unwrap().nearest(&quantize(&v), k, FLOOR);
        Some(near.into_iter().map(|(id, _)| id).collect())
    }

    /// Full-text hits fused with the threads nearest in meaning, best first.
    pub async fn hybrid(
        self: &Arc<Self>,
        store: &Store,
        q: &str,
        fts: Vec<ThreadSummary>,
        limit: usize,
    ) -> Result<Vec<ThreadSummary>> {
        if !wants_vectors(q) {
            return Ok(fts);
        }
        let Some(near) = self.nearest(q, NEIGHBOURS, false).await else {
            return Ok(fts);
        };
        let fts_ids: Vec<String> = fts.iter().map(|t| t.id.clone()).collect();
        let order: Vec<String> = rrf(&[fts_ids, near]).into_iter().take(limit).collect();
        let known: HashSet<&str> = fts.iter().map(|t| t.id.as_str()).collect();
        let missing: Vec<String> = order
            .iter()
            .filter(|id| !known.contains(id.as_str()))
            .cloned()
            .collect();
        let mut by_id: std::collections::HashMap<String, ThreadSummary> =
            fts.into_iter().map(|t| (t.id.clone(), t)).collect();
        for t in store.threads_by_ids(&missing)? {
            by_id.insert(t.id.clone(), t);
        }
        Ok(order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .collect())
    }
}

fn enabled(store: &Store) -> bool {
    store.setting(SETTING).ok().flatten().as_deref() != Some("off")
}

/// Fetches the model file if needed and records where it is.
async fn fetch(sem: &Semantic) -> Result<()> {
    if kind() == Kind::Stub || sem.path.lock().unwrap().is_some() {
        return Ok(());
    }
    let dir = paths::home().join("models");
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(Error::Http)?;
    let path = download::fetch(&http, &download::MODEL, &dir, |done, total| {
        sem.set(|s| {
            s.phase = "downloading";
            s.progress = (DOWNLOAD_SHARE * 100.0 * done as f64 / total as f64) as u8;
        })
    })
    .await?;
    *sem.path.lock().unwrap() = Some(path);
    Ok(())
}

/// Starts the background indexer, unless `MERMOZ_EMBEDDER=off`.
pub fn start(svc: &Arc<Service>) {
    if kind() == Kind::Off {
        svc.semantic.set(|s| s.phase = "off");
        return;
    }
    tokio::spawn(run(svc.clone()));
}

async fn run(svc: Arc<Service>) {
    let sem = svc.semantic.clone();
    let mut events = svc.events.subscribe();
    // The app's first list and counts own the disk at launch.
    if kind() == Kind::Real {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    let mut vectors: Option<VectorStore> = None;
    loop {
        if !enabled(&svc.store) {
            sem.unload();
            sem.set(|s| {
                *s = Status {
                    enabled: false,
                    phase: "off",
                    ..Status::default()
                }
            });
            sem.kick.notified().await;
            continue;
        }
        sem.set(|s| s.enabled = true);
        if let Err(e) = fetch(&sem).await {
            warn!("search by meaning unavailable: {e}");
            sem.set(|s| s.phase = "error");
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(600)) => {}
                _ = sem.kick.notified() => {}
            }
            continue;
        }
        if vectors.is_none() {
            match open_vectors(&sem).await {
                Ok(v) => vectors = Some(v),
                Err(e) => {
                    warn!("search vectors: {e}");
                    sem.set(|s| s.phase = "error");
                    sem.kick.notified().await;
                    continue;
                }
            }
        }
        let v = vectors.as_mut().expect("opened above");
        match pass(&svc, v, &mut events).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => warn!("indexing for search: {e}"),
        }
        // Up to date: wait for new mail or a settings change, freeing an idle model.
        loop {
            tokio::select! {
                e = events.recv() => match e {
                    Ok(Event::ThreadsChanged(_)) | Err(_) => break,
                    Ok(_) => {}
                },
                _ = sem.kick.notified() => break,
                _ = tokio::time::sleep(Duration::from_secs(30)) => sem.unload_if_idle(),
            }
        }
    }
}

async fn open_vectors(sem: &Semantic) -> Result<VectorStore> {
    let path = paths::home().join("search.sqlite");
    let (store, idx) = tokio::task::spawn_blocking(move || {
        let s = VectorStore::open(&path)?;
        let idx = s.load()?;
        Ok::<_, Error>((s, idx))
    })
    .await
    .map_err(|e| Error::other(e.to_string()))??;
    info!(threads = idx.len(), "search vectors loaded");
    *sem.index.write().unwrap() = idx;
    Ok(store)
}

/// One indexing pass, newest threads first. Returns true when it stopped early to
/// look again (new mail arrived, or the setting changed).
async fn pass(
    svc: &Arc<Service>,
    vectors: &mut VectorStore,
    events: &mut tokio::sync::broadcast::Receiver<Event>,
) -> Result<bool> {
    let sem = &svc.semantic;
    let st = svc.store.clone();
    let sigs = tokio::task::spawn_blocking(move || st.semantic_sigs())
        .await
        .map_err(|e| Error::other(e.to_string()))??;
    let (pending, gone) = {
        let idx = sem.index.read().unwrap();
        let live: HashSet<&str> = sigs.iter().map(|(id, _)| id.as_str()).collect();
        let gone: Vec<String> = idx
            .ids()
            .filter(|id| !live.contains(id))
            .map(String::from)
            .collect();
        let pending: Vec<(String, String)> = sigs
            .iter()
            .filter(|(id, sig)| idx.sig(id) != Some(sig.as_str()))
            .cloned()
            .collect();
        (pending, gone)
    };
    if !gone.is_empty() {
        vectors.delete(&gone)?;
        let mut idx = sem.index.write().unwrap();
        for id in &gone {
            idx.remove(id);
        }
    }
    let total = sigs.len();
    let progress = |done: usize| {
        sem.set(|s| {
            s.indexed = done;
            s.total = total;
            s.phase = if done >= total { "ready" } else { "indexing" };
            s.progress = if total == 0 {
                100
            } else {
                (100.0 * (DOWNLOAD_SHARE + (1.0 - DOWNLOAD_SHARE) * done as f64 / total as f64))
                    as u8
            };
        })
    };
    let mut done = total - pending.len();
    progress(done);
    let rescan = Instant::now() + Duration::from_secs(10);
    for chunk in pending.chunks(BATCH) {
        let model = sem.ensure_model().await?;
        let ids: Vec<String> = chunk.iter().map(|(id, _)| id.clone()).collect();
        let st = svc.store.clone();
        let vecs = tokio::task::spawn_blocking(move || {
            let docs = st.semantic_docs(&ids)?;
            let texts: Vec<String> = docs.iter().map(|(_, d)| d.clone()).collect();
            let v = model.embed(&texts, Lane::Index)?;
            Ok::<_, Error>(
                docs.into_iter()
                    .map(|(id, _)| id)
                    .zip(v)
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .map_err(|e| Error::other(e.to_string()))??;
        let rows: Vec<(String, String, _)> = vecs
            .into_iter()
            .filter_map(|(id, v)| {
                let sig = chunk.iter().find(|(c, _)| *c == id)?.1.clone();
                Some((id, sig, quantize(&v)))
            })
            .collect();
        vectors.put(&rows)?;
        {
            let mut idx = sem.index.write().unwrap();
            for (id, sig, q) in &rows {
                idx.upsert(id, sig, q);
            }
        }
        done += chunk.len();
        progress(done);
        // New mail goes ahead of the backlog, at most every 10 s of indexing.
        let mut fresh = false;
        while let Ok(e) = events.try_recv() {
            fresh |= matches!(e, Event::ThreadsChanged(_));
        }
        if (fresh && Instant::now() > rescan) || !enabled(&svc.store) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod eval;

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::words("ferry to the island", true)]
    #[case::short("ab", false)]
    #[case::sender_filter("from:nora ferry", false)]
    #[case::subject_filter("subject:ferry", false)]
    fn which_searches_use_vectors(#[case] q: &str, #[case] want: bool) {
        assert_eq!(wants_vectors(q), want);
    }
}
