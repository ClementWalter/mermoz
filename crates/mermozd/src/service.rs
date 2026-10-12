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
}

fn poll_interval() -> Duration {
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

    pub fn start_router(self: &Arc<Self>) {
        tokio::spawn(router_loop(self.clone()));
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

/// Routes every new Unread thread through Jev while a TypeSafe key is set.
async fn router_loop(svc: Arc<Service>) {
    let mut descriptions: Option<(std::time::Instant, Vec<(String, String)>)> = None;
    loop {
        if let Some(key) = jev::load_key() {
            match route_pending(&svc, &key, &mut descriptions).await {
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

/// Routes up to one batch of pending threads, eight requests in flight.
async fn route_pending(
    svc: &Arc<Service>,
    key: &str,
    descriptions: &mut Option<(std::time::Instant, Vec<(String, String)>)>,
) -> Result<usize> {
    let pending = svc.store.threads_to_route(32)?;
    if pending.is_empty() {
        return Ok(0);
    }
    if descriptions
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() > DESCRIPTIONS_TTL)
    {
        *descriptions = Some((std::time::Instant::now(), svc.store.project_descriptions()?));
    }
    let descs = &descriptions.as_ref().unwrap().1;
    let known: Arc<Vec<String>> = Arc::new(descs.iter().map(|(n, _)| n.clone()).collect());
    let questions = Arc::new(jev::questions(descs));
    let client = Arc::new(jev::Client::new(key.to_string()));
    let mut routed = vec![];
    for chunk in pending.chunks(8) {
        let mut set = tokio::task::JoinSet::new();
        for (tid, last_date) in chunk.iter().cloned() {
            let input = svc.store.route_input(&tid)?;
            let (client, questions, known) = (client.clone(), questions.clone(), known.clone());
            set.spawn(async move {
                (
                    tid,
                    last_date,
                    client.route(&input, &questions, &known).await,
                )
            });
        }
        while let Some(joined) = set.join_next().await {
            let Ok((tid, last_date, res)) = joined else {
                continue;
            };
            match res {
                Ok(sg) => {
                    svc.store
                        .save_suggestion(&tid, &sg, "jev-latest", last_date)?;
                    routed.push(tid);
                }
                // A refused key or an outage stops the round; it retries on the next tick.
                Err(e @ (Error::InvalidParams(_) | Error::Http(_))) => return Err(e),
                Err(Error::Upstream { status, .. }) if status == 429 || status >= 500 => {
                    return Err(Error::other(format!("TypeSafe is unavailable ({status})")));
                }
                // Anything else is specific to this thread: record "no suggestion" so it
                // is not retried forever.
                Err(e) => {
                    warn!(thread = %tid, "router skipped thread: {e}");
                    svc.store.save_suggestion(
                        &tid,
                        &Default::default(),
                        "jev-latest",
                        last_date,
                    )?;
                }
            }
        }
    }
    let n = routed.len();
    let _ = svc.events.send(Event::ThreadsChanged(routed));
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::import_needed;
    use rstest::rstest;

    #[rstest]
    #[case::fresh("initial", true)]
    #[case::running("syncing", true)]
    #[case::after_error("error", true)]
    #[case::finished("idle", false)]
    fn import_resumes_unless_finished(#[case] state: &str, #[case] want: bool) {
        assert_eq!(import_needed(state), want);
    }
}
