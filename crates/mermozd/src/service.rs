//! Daemon state: the store, one sync worker pair per account, and the event bus.

use mermoz_core::gmail::{Gmail, default_base};
use mermoz_core::oauth::{self, ClientConfig, TokenSource};
use mermoz_core::store::Store;
use mermoz_core::sync::{AccountSync, Event};
use mermoz_core::{Error, Result, jev, tokens};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, broadcast};
use tracing::{info, warn};

pub struct Worker {
    pub sync: Arc<AccountSync>,
    pub kick: Arc<Notify>,
    importing: Arc<Mutex<bool>>,
    /// Wakes the agent loop (settings changed, new mail).
    pub digest_kick: Arc<Notify>,
    /// Last provider problem (bad key, quota), shown in settings.
    pub agent_error: Arc<Mutex<Option<String>>>,
}

/// Project descriptions and when they were built.
type Descriptions = (std::time::Instant, Vec<(String, String)>);

pub struct Service {
    /// Outcome of the last "Sign in with Anthropic".
    pub login: Mutex<LoginState>,
    /// The sign-in in flight; a new attempt aborts it (a stuck browser flow never blocks a retry).
    login_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Progress of the AI directory clean-up.
    pub people_job: Mutex<JobState>,
    pub store: Arc<Store>,
    pub events: broadcast::Sender<Event>,
    workers: Mutex<HashMap<String, Arc<Worker>>>,
    /// Wakes the router (new mail, key changed).
    pub router_kick: Arc<Notify>,
    /// Last router problem (refused key, outage), shown in settings.
    pub router_error: Mutex<Option<String>>,
    /// Threads whose senders are being verified, so reopening one doesn't refetch.
    verifying: Mutex<std::collections::HashSet<String>>,
    /// Project descriptions for the router, rebuilt every [`DESCRIPTIONS_TTL`].
    descriptions: Mutex<Option<Descriptions>>,
    /// Wakes the background column sorter (key set, focus changed).
    pub column_kick: Arc<Notify>,
    /// Background column sorting state, shown in column headers.
    pub column: Mutex<ColumnState>,
    /// Statuses of the columns on screen.
    focus: Mutex<Vec<String>>,
    /// Wakes conversation grouping (new mail routed, key or AI changed).
    pub grouping_kick: Arc<Notify>,
}

pub(crate) fn poll_interval() -> Duration {
    let secs = std::env::var("MERMOZ_POLL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    Duration::from_secs(secs)
}

impl Service {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            login: Mutex::new(LoginState::default()),
            login_task: Mutex::new(None),
            people_job: Mutex::new(JobState::default()),
            store,
            events,
            workers: Mutex::new(HashMap::new()),
            router_kick: Arc::new(Notify::new()),
            router_error: Mutex::new(None),
            verifying: Mutex::new(Default::default()),
            descriptions: Mutex::new(None),
            column_kick: Arc::new(Notify::new()),
            column: Mutex::new(ColumnState::default()),
            focus: Mutex::new(vec![]),
            grouping_kick: Arc::new(Notify::new()),
        })
    }

    /// Checks, in the background, the senders of a thread's older messages (imported
    /// before verification existed); the thread refreshes when done.
    pub fn verify_thread_senders(self: &Arc<Self>, thread_id: &str) {
        let Ok(items) = self.store.unverified_senders(thread_id) else {
            return;
        };
        if items.is_empty() || !self.verifying.lock().unwrap().insert(thread_id.to_string()) {
            return;
        }
        let Ok((account, _)) = self.store.provider_thread_id(thread_id) else {
            return;
        };
        let Some(w) = self.workers.lock().unwrap().get(&account).cloned() else {
            self.verifying.lock().unwrap().remove(thread_id);
            return;
        };
        let (svc, tid) = (self.clone(), thread_id.to_string());
        tokio::spawn(async move {
            match w.sync.verify_senders(&items).await {
                Ok(_) => {
                    let _ = svc.events.send(Event::ThreadsChanged(vec![tid.clone()]));
                }
                Err(e) => warn!(thread = %tid, "sender check failed: {e}"),
            }
            svc.verifying.lock().unwrap().remove(&tid);
        });
    }

    /// Starts workers for every stored account; accounts without a token show an error.
    pub fn start_all(self: &Arc<Self>) {
        let accounts = match self.store.accounts() {
            Ok(a) => a,
            Err(e) => return warn!("cannot list accounts: {e}"),
        };
        for acc in accounts {
            if let Err(e) = self.start_account(&acc.id, &acc.email) {
                warn!(account = %acc.email, "not starting: {e}");
                let _ = self.store.set_sync(
                    &acc.id,
                    "error",
                    None,
                    None,
                    Some("Sign in again to resume syncing"),
                );
            }
        }
    }

    pub fn start_account(self: &Arc<Self>, account_id: &str, email: &str) -> Result<()> {
        let grant = tokens::load_grant(email)?;
        let cfg = match grant.client() {
            Some(c) => c,
            None => {
                // Legacy entry: bind it to the client configured today, which issued it.
                let c = ClientConfig::load()?;
                tokens::save_grant(email, &grant.refresh_token, &c)?;
                c
            }
        };
        let refresh = grant.refresh_token;
        let ts = Arc::new(TokenSource::new(cfg, refresh));
        let gmail = Arc::new(Gmail::new(default_base(), ts));
        let sync = Arc::new(AccountSync {
            store: self.store.clone(),
            gmail,
            account_id: account_id.to_string(),
            email: email.to_string(),
            events: self.events.clone(),
        });
        let worker = Arc::new(Worker {
            sync,
            kick: Arc::new(Notify::new()),
            importing: Arc::new(Mutex::new(false)),
            digest_kick: Arc::new(Notify::new()),
            agent_error: Arc::new(Mutex::new(None)),
        });
        self.workers
            .lock()
            .unwrap()
            .insert(account_id.to_string(), worker.clone());
        tokio::spawn(live_loop(self.clone(), worker.clone()));
        tokio::spawn(digest_loop(self.clone(), worker));
        Ok(())
    }

    pub fn worker(&self, account_id: &str) -> Result<Arc<Worker>> {
        self.workers
            .lock()
            .unwrap()
            .get(account_id)
            .cloned()
            .ok_or_else(|| {
                Error::other(format!(
                    "account {account_id} is not connected; sign in again"
                ))
            })
    }

    /// Wakes an account's live loop so a local edit reaches Gmail within milliseconds.
    pub fn kick(&self, account_id: &str) {
        if let Some(w) = self.workers.lock().unwrap().get(account_id) {
            w.kick.notify_one();
        }
    }

    pub fn kick_all(&self) {
        for w in self.workers.lock().unwrap().values() {
            w.kick.notify_one();
        }
    }

    pub fn kick_agents(&self) {
        for w in self.workers.lock().unwrap().values() {
            w.digest_kick.notify_one();
        }
    }

    pub fn agent_error(&self, account_id: &str) -> Option<String> {
        self.workers
            .lock()
            .unwrap()
            .get(account_id)
            .and_then(|w| w.agent_error.lock().unwrap().clone())
    }

    pub fn stop_account(&self, account_id: &str) {
        self.workers.lock().unwrap().remove(account_id);
    }

    /// Begins Google sign-in; the account appears once the browser flow completes.
    pub async fn add_gmail(self: &Arc<Self>) -> Result<String> {
        let cfg = ClientConfig::load()?;
        let pending = oauth::start(cfg.clone()).await?;
        let url = pending.auth_url.clone();
        let svc = self.clone();
        tokio::spawn(async move {
            let result: Result<()> = async {
                let refresh = pending.finish(Duration::from_secs(600)).await?;
                let ts = Arc::new(TokenSource::new(cfg.clone(), refresh.clone()));
                let profile = Gmail::new(default_base(), ts).profile().await?;
                tokens::save_grant(&profile.email_address, &refresh, &cfg)?;
                let acc = svc.store.add_account(&profile.email_address, "gmail")?;
                svc.store
                    .set_sync(&acc.id, "initial", None, profile.messages_total, None)?;
                svc.start_account(&acc.id, &acc.email)?;
                info!(account = %acc.email, "account added");
                Ok(())
            }
            .await;
            if let Err(e) = result {
                warn!("sign-in failed: {e}");
            }
            let _ = svc.events.send(Event::AccountsChanged);
        });
        Ok(url)
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct JobState {
    pub running: bool,
    pub done: usize,
    pub total: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LoginState {
    pub running: bool,
    pub error: Option<String>,
}

/// The secret for a provider: a fresh OAuth access token, or the stored API key.
pub async fn credential(p: &mermoz_core::agent::Provider) -> Result<String> {
    if p.kind == "chatgpt" {
        return mermoz_core::chatgpt::access_token(&p.id).await;
    }
    if p.auth == "oauth" {
        let profile = p
            .profile
            .as_deref()
            .ok_or_else(|| Error::other("signed-in provider has no profile"))?;
        mermoz_core::ant::access_token(profile).await
    } else {
        tokens::load_provider_key(&p.id)
    }
}

impl Service {
    /// Runs one sign-in attempt in the background, replacing any attempt still waiting.
    fn run_login<F>(self: &Arc<Self>, attempt: F)
    where
        F: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        if let Some(old) = self.login_task.lock().unwrap().take() {
            old.abort();
        }
        *self.login.lock().unwrap() = LoginState {
            running: true,
            error: None,
        };
        let svc = self.clone();
        let handle = tokio::spawn(async move {
            let result = attempt.await;
            *svc.login.lock().unwrap() = LoginState {
                running: false,
                error: result.err().map(|e| e.to_string()),
            };
            let _ = svc.events.send(Event::ProvidersChanged);
            svc.kick_agents();
        });
        *self.login_task.lock().unwrap() = Some(handle);
    }

    /// Lets the AI merge duplicate people and clean their names, in batches, in the background.
    pub fn clean_people(self: &Arc<Self>) -> Result<usize> {
        let accounts = self.store.accounts()?;
        let provider = accounts
            .iter()
            .find_map(|a| self.store.provider_for_account(&a.id).ok().flatten());
        let Some(pr) = provider else {
            return Err(Error::InvalidParams(
                "Allow an AI for one of your accounts in Settings › Agents first.".into(),
            ));
        };
        let people = self.store.people(None, i64::MAX)?;
        // Sorted by family name, so variants of one name share a batch.
        const BATCH: usize = 100;
        let total = people.len().div_ceil(BATCH);
        {
            let mut j = self.people_job.lock().unwrap();
            if j.running {
                return Ok(j.total);
            }
            *j = JobState {
                running: true,
                done: 0,
                total,
                error: None,
            };
        }
        let svc = self.clone();
        tokio::spawn(async move {
            let result: Result<()> = async {
                let key = credential(&pr).await?;
                let client = mermoz_core::agent::Client::new();
                for (i, chunk) in people.chunks(BATCH).enumerate() {
                    let entries: Vec<mermoz_core::agent::PersonEntry> = chunk
                        .iter()
                        .map(|p| mermoz_core::agent::PersonEntry {
                            id: p.email.clone(),
                            name: p.card.name.clone(),
                            emails: p.emails.clone(),
                            company: p.card.company.clone(),
                        })
                        .collect();
                    let groups = client.people_groups(&pr, &key, &entries).await?;
                    svc.store.apply_person_groups(chunk, &groups)?;
                    svc.people_job.lock().unwrap().done = i + 1;
                    let _ = svc.events.send(Event::ContactsChanged);
                }
                Ok(())
            }
            .await;
            let mut j = svc.people_job.lock().unwrap();
            j.running = false;
            j.error = result.err().map(|e| e.to_string());
            drop(j);
            let _ = svc.events.send(Event::ContactsChanged);
        });
        Ok(total)
    }

    /// Starts "Continue with ChatGPT"; returns the URL the app opens in the browser.
    pub async fn login_chatgpt(self: &Arc<Self>) -> Result<String> {
        let host = match self.store.setting("chatgpt_host_id")? {
            Some(h) => h,
            None => {
                let h = mermoz_core::chatgpt::new_host_id();
                self.store.set_setting("chatgpt_host_id", &h)?;
                h
            }
        };
        let pending = mermoz_core::chatgpt::start(&host, None).await?;
        let url = pending.url.clone();
        let svc = self.clone();
        self.run_login(async move {
            let cred = pending.finish(Duration::from_secs(600)).await?;
            // The account's own catalog decides the model; the first listed is the default.
            let (model, _) = mermoz_core::chatgpt::models(&cred.access_token)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| Error::other("No ChatGPT model is available on this plan"))?;
            let id = mermoz_core::model::new_id("prv");
            mermoz_core::chatgpt::save(&id, &cred)?;
            svc.store.save_provider(&mermoz_core::agent::Provider {
                id,
                name: "ChatGPT".into(),
                kind: "chatgpt".into(),
                base_url: None,
                model,
                auth: "oauth".into(),
                profile: None,
            })
        });
        Ok(url)
    }

    /// Runs the browser sign-in; on success the provider appears in Settings.
    pub fn login_anthropic(self: &Arc<Self>) -> Result<()> {
        if mermoz_core::ant::binary().is_none() {
            return Err(Error::other(
                "Anthropic sign-in is not available in this build",
            ));
        }
        let svc = self.clone();
        self.run_login(async move {
            let id = mermoz_core::model::new_id("prv");
            mermoz_core::ant::login(&id).await?;
            // Proves the login yields a token before showing the provider as connected.
            mermoz_core::ant::access_token(&id).await?;
            svc.store.save_provider(&mermoz_core::agent::Provider {
                id: id.clone(),
                name: "Claude".into(),
                kind: "anthropic".into(),
                base_url: None,
                model: "claude-opus-5-5".into(),
                auth: "oauth".into(),
                profile: Some(id.clone()),
            })
        });
        Ok(())
    }
}

/// Days of past mail the agent processes when switched on; new mail is always processed.
pub fn backfill_since(store: &Store) -> i64 {
    let days: i64 = store
        .setting("digest_backfill_days")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    mermoz_core::model::now_ms() - days * 24 * 3600 * 1000
}

/// Network, rate-limit and server errors are retried later, never recorded as a verdict.
fn transient(e: &Error) -> bool {
    match e {
        Error::Http(_) => true,
        Error::Upstream { status, .. } => {
            *status == 429 || *status >= 500 || *status == 401 || *status == 403
        }
        _ => false,
    }
}

/// Per-account agent loop: cleans received mail and grows the directory, newest first,
/// only when the user enabled it and a provider is allowed for this account.
async fn digest_loop(svc: Arc<Service>, w: Arc<Worker>) {
    let id = w.sync.account_id.clone();
    let client = Arc::new(mermoz_core::agent::Client::new());
    loop {
        if !svc.workers.lock().unwrap().contains_key(&id) {
            return;
        }
        let mut idle = Duration::from_secs(15);
        let enabled = svc
            .store
            .setting(&format!("digest:{id}"))
            .ok()
            .flatten()
            .as_deref()
            == Some("true");
        let provider = if enabled {
            svc.store.provider_for_account(&id).ok().flatten()
        } else {
            None
        };
        if let Some(p) = provider {
            match credential(&p).await {
                Err(e) => *w.agent_error.lock().unwrap() = Some(format!("{}: {e}", p.name)),
                Ok(key) => {
                    let batch = svc
                        .store
                        .pending_digests(&id, backfill_since(&svc.store), 8)
                        .unwrap_or_default();
                    let mut set = tokio::task::JoinSet::new();
                    for (mid, from_name, from_email, subject, body) in batch {
                        let (c, p, key) = (client.clone(), p.clone(), key.clone());
                        set.spawn(async move {
                            let from = if from_name.is_empty() {
                                from_email
                            } else {
                                format!("{from_name} <{from_email}>")
                            };
                            (mid, c.digest(&p, &key, &from, &subject, &body).await)
                        });
                    }
                    let mut threads = vec![];
                    let mut backoff = false;
                    while let Some(Ok((mid, outcome))) = set.join_next().await {
                        if let Err(e) = &outcome
                            && transient(e)
                        {
                            *w.agent_error.lock().unwrap() = Some(e.to_string());
                            backoff = true;
                            continue;
                        }
                        if let Ok(t) = svc.store.save_digest(&mid, &p.id, &outcome) {
                            threads.push(t);
                        }
                    }
                    if !threads.is_empty() {
                        *w.agent_error.lock().unwrap() = None;
                        let _ = svc.events.send(Event::ThreadsChanged(threads));
                        let _ = svc.events.send(Event::ContactsChanged);
                        // More work queued: keep going without waiting.
                        idle = if backoff {
                            Duration::from_secs(60)
                        } else {
                            Duration::from_millis(50)
                        };
                    } else if backoff {
                        idle = Duration::from_secs(60);
                    }
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(idle) => {}
            _ = w.digest_kick.notified() => {}
        }
    }
}

/// Per-account loop: keep the import running, pull changes, push local edits.
async fn live_loop(svc: Arc<Service>, w: Arc<Worker>) {
    let id = w.sync.account_id.clone();
    if let Err(e) = w.sync.refresh_labels().await {
        warn!(account = %w.sync.email, "labels: {e}");
    }
    loop {
        if !svc.workers.lock().unwrap().contains_key(&id) {
            return;
        }
        ensure_import(&svc, &w);
        if let Err(e) = w.sync.incremental().await {
            warn!(account = %w.sync.email, "incremental: {e}");
        }
        svc.router_kick.notify_one();
        match w.sync.push_ops().await {
            Ok(n) if n > 0 => info!(account = %w.sync.email, pushed = n, "pushed local changes"),
            Ok(_) => {}
            Err(e) => warn!(account = %w.sync.email, "push: {e}"),
        }
        tokio::select! {
            _ = tokio::time::sleep(poll_interval()) => {}
            _ = w.kick.notified() => {}
        }
    }
}

/// Anything short of a finished import ("idle") resumes, including after an error.
fn import_needed(state: &str) -> bool {
    state != "idle"
}

fn ensure_import(svc: &Arc<Service>, w: &Arc<Worker>) {
    let state = svc
        .store
        .account(&w.sync.account_id)
        .map(|a| a.sync.state)
        .unwrap_or_default();
    if !import_needed(&state) {
        return;
    }
    let mut running = w.importing.lock().unwrap();
    if *running {
        return;
    }
    *running = true;
    let w = w.clone();
    let store = svc.store.clone();
    let events = svc.events.clone();
    tokio::spawn(async move {
        let mut delay = Duration::from_secs(5);
        loop {
            match w.sync.initial_sync().await {
                Ok(()) => break,
                Err(e) => {
                    warn!(account = %w.sync.email, "import paused: {e}");
                    let _ = store.set_sync(
                        &w.sync.account_id,
                        "error",
                        None,
                        None,
                        Some(&e.to_string()),
                    );
                    let _ = events.send(Event::AccountsChanged);
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(300));
                }
            }
        }
        *w.importing.lock().unwrap() = false;
    });
}

/// Project descriptions change slowly; rebuilding them costs a scan of filed mail.
const DESCRIPTIONS_TTL: Duration = Duration::from_secs(600);

/// Requests in flight for the background pass (12 ran clean against the live API).
const COLUMN_PARALLEL: usize = 12;

/// Threads taken per background round; small enough that a focus change or new
/// mail is picked up within a couple of seconds.
const COLUMN_BATCH: i64 = 48;

/// What a routing call needs besides the thread: the client and the questions.
struct RouteKit {
    client: Arc<jev::Client>,
    questions: Arc<serde_json::Value>,
    known: Arc<Vec<String>>,
}

/// Background sorting of the open columns, as the app shows it.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ColumnState {
    /// Working through threads right now.
    pub sorting: bool,
    /// Why sorting is on hold: "limit" (daily cap reached) or "unavailable" (backing off).
    pub paused: Option<String>,
}

impl Service {
    pub fn start_router(self: &Arc<Self>) {
        tokio::spawn(router_loop(self.clone()));
        tokio::spawn(column_loop(self.clone()));
        tokio::spawn(crate::grouping::grouping_loop(self.clone()));
    }

    /// The columns on screen; the background pass sorts them first.
    pub fn set_focus(&self, statuses: Vec<String>) {
        let mut f = self.focus.lock().unwrap();
        if *f != statuses {
            *f = statuses;
            self.column_kick.notify_one();
        }
    }

    fn route_kit(&self, key: &str) -> Result<RouteKit> {
        let mut cache = self.descriptions.lock().unwrap();
        if cache
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > DESCRIPTIONS_TTL)
        {
            *cache = Some((
                std::time::Instant::now(),
                self.store.project_descriptions()?,
            ));
        }
        let descs = &cache.as_ref().unwrap().1;
        Ok(RouteKit {
            client: Arc::new(jev::Client::new(key.to_string())),
            questions: Arc::new(jev::questions(descs)),
            known: Arc::new(descs.iter().map(|(n, _)| n.clone()).collect()),
        })
    }
}

type Outcome = (String, i64, Result<jev::Routed>);

/// Routes threads through Jev with at most `parallel` requests in flight.
async fn route_many(
    svc: &Arc<Service>,
    kit: &RouteKit,
    items: &[(String, i64)],
    parallel: usize,
) -> Vec<Outcome> {
    let mut set = tokio::task::JoinSet::new();
    let mut out = vec![];
    for (tid, last_date) in items.iter().cloned() {
        if set.len() >= parallel
            && let Some(Ok(done)) = set.join_next().await
        {
            out.push(done);
        }
        let input = match svc.store.route_input(&tid) {
            Ok(i) => i,
            Err(e) => {
                out.push((tid, last_date, Err(e)));
                continue;
            }
        };
        let (client, questions, known) =
            (kit.client.clone(), kit.questions.clone(), kit.known.clone());
        set.spawn(async move {
            let res = client.route(&input, &questions, &known).await;
            (tid, last_date, res)
        });
    }
    while let Some(joined) = set.join_next().await {
        if let Ok(done) = joined {
            out.push(done);
        }
    }
    out
}

/// A refused key, a rate limit or an outage: stop and retry later, never a verdict.
fn pause_reason(e: &Error) -> Option<Option<Duration>> {
    match e {
        Error::RateLimited { retry_after } => Some(*retry_after),
        Error::InvalidParams(_) | Error::Http(_) => Some(None),
        Error::Upstream { status, .. } if *status == 429 || *status >= 500 => Some(None),
        _ => None,
    }
}

fn input_tokens(outcomes: &[Outcome]) -> i64 {
    outcomes
        .iter()
        .filter_map(|(_, _, r)| r.as_ref().ok().and_then(|r| r.input_tokens))
        .sum()
}

/// Routes every new Unread thread through Jev while a TypeSafe key is set.
async fn router_loop(svc: Arc<Service>) {
    loop {
        if let Some(key) = jev::load_key() {
            match route_pending(&svc, &key).await {
                Ok(n) => {
                    if n > 0 {
                        info!(routed = n, "routed new mail");
                    }
                    *svc.router_error.lock().unwrap() = None;
                }
                Err(e) => {
                    warn!("router: {e}");
                    *svc.router_error.lock().unwrap() = Some(e.to_string());
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(poll_interval()) => {}
            _ = svc.router_kick.notified() => {}
        }
    }
}

/// Routes up to one batch of pending Unread threads, eight requests in flight.
async fn route_pending(svc: &Arc<Service>, key: &str) -> Result<usize> {
    let pending = svc.store.threads_to_route(32)?;
    if pending.is_empty() {
        return Ok(0);
    }
    let kit = svc.route_kit(key)?;
    let outcomes = route_many(svc, &kit, &pending, 8).await;
    let tokens = input_tokens(&outcomes);
    let mut routed = vec![];
    let mut stop = None;
    for (tid, last_date, res) in outcomes {
        match res {
            Ok(r) => {
                svc.store
                    .save_suggestion(&tid, &r.suggestion, "jev-latest", last_date)?;
                routed.push(tid);
            }
            // A refused key or an outage stops the round; it retries on the next tick.
            Err(e) if pause_reason(&e).is_some() => stop = Some(e),
            // Anything else is specific to this thread: record "no suggestion" so it
            // is not retried forever.
            Err(e) => {
                warn!(thread = %tid, "router skipped thread: {e}");
                svc.store
                    .save_suggestion(&tid, &Default::default(), "jev-latest", last_date)?;
            }
        }
    }
    if tokens > 0 {
        info!(
            threads = routed.len(),
            input_tokens = tokens,
            "routed new mail"
        );
    }
    let n = routed.len();
    let _ = svc.events.send(Event::ThreadsChanged(routed));
    // Routed mail knows whether it is automated: grouping can take it now.
    svc.grouping_kick.notify_one();
    match stop {
        Some(Error::InvalidParams(m)) => Err(Error::InvalidParams(m)),
        Some(Error::Http(e)) => Err(Error::Http(e)),
        Some(_) => Err(Error::other("TypeSafe is unavailable right now")),
        None => Ok(n),
    }
}

/// How a background round ended.
#[derive(Debug, PartialEq)]
enum Round {
    /// Threads were classified; more may be waiting.
    Worked,
    /// Nothing left to classify.
    Idle,
    /// New Unread mail goes first.
    Yield,
    /// Today's cap is spent.
    Capped,
    /// TypeSafe refused or is overloaded; wait (as asked, if it said) before retrying.
    Pause(Option<Duration>),
}

/// Sorts the open columns in the background: every already-read thread without a
/// current verdict gets one Jev call, the column on screen first. Suggestions only;
/// nothing moves until the user accepts.
async fn column_loop(svc: Arc<Service>) {
    let mut last_backoff: Option<Duration> = None;
    loop {
        let round = match jev::load_key() {
            None => Round::Idle,
            Some(key) => column_round(&svc, &key).await.unwrap_or_else(|e| {
                warn!("background sorting: {e}");
                Round::Pause(None)
            }),
        };
        let (sorting, paused, wait) = match round {
            Round::Worked => (true, None, Duration::from_millis(20)),
            Round::Yield => (true, None, Duration::from_millis(500)),
            Round::Idle => (false, None, Duration::from_secs(30)),
            Round::Capped => (false, Some("limit"), Duration::from_secs(600)),
            Round::Pause(asked) => {
                let d = jev::backoff(asked, last_backoff);
                warn!(wait_ms = d.as_millis() as u64, "background sorting paused");
                (false, Some("unavailable"), d)
            }
        };
        last_backoff = match round {
            Round::Pause(asked) => Some(jev::backoff(asked, last_backoff)),
            _ => None,
        };
        let state = ColumnState {
            sorting,
            paused: paused.map(String::from),
        };
        let changed = {
            let mut s = svc.column.lock().unwrap();
            let changed = s.sorting != state.sorting || s.paused != state.paused;
            *s = state;
            changed
        };
        if changed {
            let _ = svc.events.send(Event::SuggestionsChanged);
        }
        if paused.is_some() {
            // A kick must not cut a backoff short.
            tokio::time::sleep(wait).await;
        } else {
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = svc.column_kick.notified() => {}
            }
        }
    }
}

/// Calls the next background round may make: what is left of a running resort's
/// ceiling, else of today's cap.
fn allowance(resort: Option<&mermoz_core::store::Resort>, used_today: i64, cap: i64) -> i64 {
    match resort {
        Some(r) => (r.ceiling - r.calls).max(0),
        None => jev::calls_left(used_today, cap),
    }
}

async fn column_round(svc: &Arc<Service>, key: &str) -> Result<Round> {
    // Threads keep ageing while the daemon runs; their pending live statuses go too.
    if svc
        .store
        .drop_stale_suggestions(mermoz_core::model::now_ms())?
        > 0
    {
        let _ = svc.events.send(Event::SuggestionsChanged);
    }
    if !svc.store.threads_to_route(1)?.is_empty() {
        return Ok(Round::Yield);
    }
    let used = svc.store.background_calls_today()?;
    let cap = svc.store.background_daily_cap()?;
    let mut resort = svc.store.resort()?;
    if let Some(r) = resort.as_ref().filter(|r| r.calls >= r.ceiling) {
        warn!(
            calls = r.calls,
            ceiling = r.ceiling,
            "whole-mailbox sort reached its ceiling; stopping it"
        );
        svc.store.finish_resort(r.since)?;
        resort = None;
    }
    let left = allowance(resort.as_ref(), used, cap);
    if left == 0 {
        info!(
            calls_today = used,
            cap, "background sorting reached today's limit"
        );
        return Ok(Round::Capped);
    }
    let focus = svc.focus.lock().unwrap().clone();
    let batch = svc
        .store
        .threads_to_classify(&focus, left.min(COLUMN_BATCH))?;
    if batch.is_empty() {
        // Only the resort this round started with: one asked for meanwhile has work.
        if let Some(r) = resort
            .as_ref()
            .map(|r| svc.store.finish_resort(r.since))
            .transpose()?
            .flatten()
        {
            info!(
                calls = r.calls,
                input_tokens = r.input_tokens,
                "whole-mailbox sort finished"
            );
            let _ = svc.events.send(Event::SuggestionsChanged);
        }
        return Ok(Round::Idle);
    }
    let kit = svc.route_kit(key)?;
    let outcomes = route_many(svc, &kit, &batch, COLUMN_PARALLEL).await;
    let tokens = input_tokens(&outcomes);
    // A resort's calls count against its own ceiling, not today's allowance.
    let calls_today = if resort.is_some() {
        if let Some(r) = svc
            .store
            .record_resort_round(outcomes.len() as i64, tokens)?
        {
            info!(
                calls = r.calls,
                ceiling = r.ceiling,
                remaining = svc.store.resort_remaining()?,
                input_tokens = r.input_tokens,
                "whole-mailbox sort progress"
            );
        }
        used
    } else {
        svc.store.add_background_calls(outcomes.len() as i64)?
    };
    let (mut classified, mut shown) = (0, vec![]);
    let mut pause = None;
    for (tid, last_date, res) in outcomes {
        match res {
            Ok(r) => {
                classified += 1;
                if svc
                    .store
                    .save_column_suggestion(&tid, &r.suggestion, "jev-latest", last_date)?
                {
                    shown.push(tid);
                }
            }
            Err(e) => match pause_reason(&e) {
                Some(asked) => pause = Some(asked.or(pause.flatten())),
                None => {
                    warn!(thread = %tid, "background sorting skipped thread: {e}");
                    svc.store.save_column_suggestion(
                        &tid,
                        &Default::default(),
                        "jev-latest",
                        last_date,
                    )?;
                }
            },
        }
    }
    info!(
        classified,
        suggested = shown.len(),
        input_tokens = tokens,
        calls_today,
        "background sorting round"
    );
    // One coarse event per round: the app refreshes chips and column bars together.
    let _ = svc.events.send(Event::SuggestionsChanged);
    Ok(match pause {
        Some(asked) => Round::Pause(asked),
        None => Round::Worked,
    })
}

#[cfg(test)]
mod tests {
    use super::{allowance, import_needed};
    use mermoz_core::store::Resort;
    use rstest::rstest;

    fn resort(calls: i64, ceiling: i64) -> Resort {
        Resort {
            since: 0,
            statuses: vec!["archive".into()],
            total: 100,
            ceiling,
            calls,
            input_tokens: 0,
        }
    }

    #[rstest]
    #[case::daily_cap_without_resort(None, 2000)]
    #[case::resort_goes_past_the_daily_cap(Some(resort(0, 30_000)), 30_000)]
    #[case::resort_counts_its_own_calls(Some(resort(29_000, 30_000)), 1000)]
    #[case::resort_stops_at_its_ceiling(Some(resort(30_500, 30_000)), 0)]
    fn background_allowance(#[case] r: Option<Resort>, #[case] want: i64) {
        assert_eq!(allowance(r.as_ref(), 18_000, 20_000), want);
    }

    #[rstest]
    #[case::fresh("initial", true)]
    #[case::running("syncing", true)]
    #[case::after_error("error", true)]
    #[case::finished("idle", false)]
    fn import_resumes_unless_finished(#[case] state: &str, #[case] want: bool) {
        assert_eq!(import_needed(state), want);
    }
}
