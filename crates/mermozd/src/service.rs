//! Daemon state: the store, one sync worker pair per account, and the event bus.

use mermoz_core::gmail::{Gmail, default_base};
use mermoz_core::model::{Address, Message};
use mermoz_core::oauth::{self, ClientConfig, TokenSource};
use mermoz_core::outlook::{self, Outlook};
use mermoz_core::provider::Backend;
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
    /// The token came from another OAuth client than the app's (see
    /// [`tokens::GoogleGrant::needs_reconnect`]); it keeps refreshing with that client.
    pub needs_reconnect: bool,
    /// The account's Google sign-in; refused once Google stops accepting it.
    pub tokens: Arc<TokenSource>,
}

/// Project descriptions and when they were built.
type Descriptions = (std::time::Instant, Vec<(String, String)>);

pub struct Service {
    /// Outcome of the last "Sign in with Anthropic".
    pub login: Mutex<LoginState>,
    /// Outcome of the last Google mail sign-in, with Google's refusal in plain words.
    pub gmail_sign_in: Mutex<GmailSignIn>,
    /// The sign-in in flight; a new attempt aborts it (a stuck browser flow never blocks a retry).
    login_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Progress of the AI directory clean-up.
    pub people_job: Mutex<JobState>,
    pub store: Arc<Store>,
    pub events: broadcast::Sender<Event>,
    workers: Mutex<HashMap<String, Arc<Worker>>>,
    /// Wakes the router (new mail, key changed).
    pub router_kick: Arc<Notify>,
    /// Last problem per provider (refused key, outage), shown in settings.
    router_error: Mutex<HashMap<jev::Provider, String>>,
    /// Threads whose senders are being verified, so reopening one doesn't refetch.
    verifying: Mutex<std::collections::HashSet<String>>,
    /// Senders being researched on the web, by (name, address).
    researching: Mutex<std::collections::HashSet<(String, String)>>,
    /// Project descriptions for the router, rebuilt every [`DESCRIPTIONS_TTL`].
    descriptions: Mutex<Option<Descriptions>>,
    /// Why each provider is on hold, and until when (out of credits, rate limited…).
    pause: Mutex<HashMap<jev::Provider, Pause>>,
    /// Wakes conversation grouping (new mail routed, key or AI changed).
    pub grouping_kick: Arc<Notify>,
    /// Google Drive connections, picks and the local Drive cache.
    pub drive: crate::drive::Hub,
    /// Wakes the feed's background judging (new mail, key set).
    pub feed_kick: Arc<Notify>,
    /// Wakes the Later loop (a thread was just put off).
    pub later_kick: Arc<Notify>,
    /// Threads whose card words are being written, so a re-render doesn't ask twice.
    pub feed_inflight: Mutex<std::collections::HashSet<String>>,
    /// Search by meaning: the model, the vectors and their indexing.
    pub semantic: Arc<crate::semantic::Semantic>,
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
            gmail_sign_in: Mutex::new(GmailSignIn::default()),
            login_task: Mutex::new(None),
            people_job: Mutex::new(JobState::default()),
            store,
            events,
            workers: Mutex::new(HashMap::new()),
            router_kick: Arc::new(Notify::new()),
            router_error: Mutex::new(HashMap::new()),
            verifying: Mutex::new(Default::default()),
            researching: Mutex::new(Default::default()),
            descriptions: Mutex::new(None),
            pause: Mutex::new(HashMap::new()),
            grouping_kick: Arc::new(Notify::new()),
            drive: Default::default(),
            feed_kick: Arc::new(Notify::new()),
            later_kick: Arc::new(Notify::new()),
            feed_inflight: Mutex::new(Default::default()),
            semantic: Default::default(),
        })
    }

    /// Checks, in the background, the senders of a thread's older messages (imported
    /// before verification existed) and learns their `Reply-To`; the thread refreshes
    /// when done.
    pub fn verify_thread_senders(self: &Arc<Self>, thread_id: &str) {
        let Ok(items) = self.store.unverified_senders(thread_id) else {
            return;
        };
        let replies = self.store.missing_headers(thread_id).unwrap_or_default();
        if (items.is_empty() && replies.is_empty())
            || !self.verifying.lock().unwrap().insert(thread_id.to_string())
        {
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
            // Reply-To first: the reply composer reads it.
            if let Err(e) = w.sync.backfill_headers(&replies).await {
                warn!(thread = %tid, "reply-to backfill failed: {e}");
            }
            if let Err(e) = w.sync.verify_senders(&items).await {
                warn!(thread = %tid, "sender check failed: {e}");
            }
            let _ = svc.events.send(Event::ThreadsChanged(vec![tid.clone()]));
            svc.verifying.lock().unwrap().remove(&tid);
        });
    }

    /// Researches on the web, in the background, who the new and lookalike senders of
    /// these messages claim to be; their threads refresh as each answer comes. Only the
    /// sender's name and address go to the AI, never the message.
    pub fn research_senders(self: &Arc<Self>, account_id: &str, messages: &[Message]) {
        let todo: Vec<(String, Address)> = messages
            .iter()
            .filter(|m| matches!(m.auth.as_deref(), Some("new" | "lookalike")))
            .filter(|m| m.sender_claim.is_none() && !m.from.name.trim().is_empty())
            .filter(|m| !self.store.has_sender_claim(&m.from).unwrap_or(true))
            .filter(|m| {
                let key = (m.from.name.clone(), m.from.email.to_lowercase());
                self.researching.lock().unwrap().insert(key)
            })
            .map(|m| (m.thread_id.clone(), m.from.clone()))
            .collect();
        if todo.is_empty() {
            return;
        }
        let Some(p) = self
            .store
            .provider_for_account(account_id)
            .ok()
            .flatten()
            .filter(mermoz_core::chat::can_research)
        else {
            self.done_researching(&todo);
            return;
        };
        let svc = self.clone();
        tokio::spawn(async move {
            let key = match credential(&p).await {
                Ok(k) => k,
                Err(e) => {
                    warn!(provider = %p.name, "sender check: {e}");
                    return svc.done_researching(&todo);
                }
            };
            for (tid, from) in &todo {
                if !svc.store.take_sender_check().unwrap_or(false) {
                    break;
                }
                match mermoz_core::sender_check::research(&p, &key, &from.name, &from.email).await {
                    Ok(Some(claim)) => {
                        if let Err(e) = svc.store.save_sender_claim(from, &claim) {
                            warn!("sender check not saved: {e}");
                        }
                        let _ = svc.events.send(Event::ThreadsChanged(vec![tid.clone()]));
                    }
                    Ok(None) => break,
                    Err(e) => warn!(provider = %p.name, "sender check failed: {e}"),
                }
            }
            svc.done_researching(&todo);
        });
    }

    fn done_researching(&self, todo: &[(String, Address)]) {
        let mut r = self.researching.lock().unwrap();
        for (_, from) in todo {
            r.remove(&(from.name.clone(), from.email.to_lowercase()));
        }
    }

    /// Starts workers for every stored account; accounts without a token show an error.
    pub fn start_all(self: &Arc<Self>) {
        let accounts = match self.store.accounts() {
            Ok(a) => a,
            Err(e) => return warn!("cannot list accounts: {e}"),
        };
        for acc in accounts {
            // The demo account is local: nothing to sync, its mail is generated.
            if mermoz_core::demo_account::is_demo(&acc) {
                match mermoz_core::demo_account::refresh_if_stale(
                    &self.store,
                    mermoz_core::model::now_ms(),
                ) {
                    Ok(true) => info!("demo mailbox generated again"),
                    Ok(false) => {}
                    Err(e) => warn!("demo mailbox: {e}"),
                }
                continue;
            }
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
        if self.store.account(account_id)?.provider == "outlook" {
            return self.start_outlook(account_id, email);
        }
        let grant = tokens::load_grant(email)?;
        let needs_reconnect =
            ClientConfig::load().is_ok_and(|app| grant.needs_reconnect(&app.client_id));
        if needs_reconnect {
            warn!(account = %email, "signed in with another OAuth client; asking to reconnect");
        }
        let cfg = match grant.client() {
            Some(c) => c,
            None => {
                // Legacy entry: bind it to the client configured today, which issued it.
                let c = ClientConfig::load()?;
                tokens::save_grant(email, &grant.refresh_token, &c, false)?;
                c
            }
        };
        let refresh = grant.refresh_token;
        let ts = self.token_source(
            account_id,
            TokenSource::new(cfg, refresh).with_on_refused(self.on_refused(account_id, email)),
        )?;
        let gmail = Arc::new(Gmail::new(default_base(), ts.clone()));
        self.spawn_worker(
            account_id,
            email,
            Backend::Gmail(gmail),
            ts,
            needs_reconnect,
        );
        Ok(())
    }

    /// An Outlook account's worker: its Microsoft refresh token, which Microsoft
    /// replaces on every refresh, is saved again each time. A refused sign-in
    /// (`invalid_grant`) asks to sign in again, as for Google.
    fn start_outlook(self: &Arc<Self>, account_id: &str, email: &str) -> Result<()> {
        let grant = tokens::load_grant(email)?;
        let mut cfg = grant
            .client()
            .ok_or_else(|| Error::other("Outlook sign-in has no client recorded"))?;
        cfg.scope = Some(oauth::MICROSOFT_SCOPES.into());
        let (keep, owner) = (cfg.clone(), email.to_string());
        let ts = self.token_source(
            account_id,
            TokenSource::rotating(
                cfg,
                grant.refresh_token,
                Box::new(move |new| {
                    if let Err(e) = tokens::save_grant(&owner, new, &keep, false) {
                        warn!(account = %owner, "could not save the new sign-in: {e}");
                    }
                }),
            )
            .with_on_refused(self.on_refused(account_id, email)),
        )?;
        let client = Arc::new(Outlook::new(outlook::default_base(), ts.clone()));
        self.spawn_worker(account_id, email, Backend::Outlook(client), ts, false);
        Ok(())
    }

    /// The account's token source, starting refused when its sign-in was refused before.
    fn token_source(&self, account_id: &str, ts: TokenSource) -> Result<Arc<TokenSource>> {
        if self.store.sign_in_expired(account_id)? {
            ts.mark_refused();
        }
        Ok(Arc::new(ts))
    }

    /// Registers an account's sync and starts its loops.
    fn spawn_worker(
        self: &Arc<Self>,
        account_id: &str,
        email: &str,
        backend: Backend,
        ts: Arc<TokenSource>,
        needs_reconnect: bool,
    ) {
        let sync = Arc::new(AccountSync {
            store: self.store.clone(),
            backend,
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
            needs_reconnect,
            tokens: ts,
        });
        self.workers
            .lock()
            .unwrap()
            .insert(account_id.to_string(), worker.clone());
        tokio::spawn(live_loop(self.clone(), worker.clone()));
        tokio::spawn(digest_loop(self.clone(), worker));
    }

    /// Records that Google refused the account's sign-in and tells the app, once per
    /// refusal; sync and write-back then wait for the user to sign in again.
    fn on_refused(&self, account_id: &str, email: &str) -> oauth::OnRefused {
        let (store, events) = (self.store.clone(), self.events.clone());
        let (id, email) = (account_id.to_string(), email.to_string());
        Box::new(move || {
            warn!(account = %email, "Google refused the sign-in; waiting for the user to sign in again");
            if let Err(e) = store.set_sign_in_expired(&id, true) {
                warn!(account = %email, "cannot record the refused sign-in: {e}");
            }
            let _ = events.send(Event::AccountsChanged);
        })
    }

    /// Google refused the account's sign-in: nothing syncs until the user signs in again.
    pub fn sign_in_expired(&self, account_id: &str) -> bool {
        self.store.sign_in_expired(account_id).unwrap_or(false)
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
    /// `custom` is a sign-in client the user brings (their company's own Google
    /// project); it is stored with the token and used for every later consent.
    pub async fn add_gmail(self: &Arc<Self>, custom: Option<ClientConfig>) -> Result<String> {
        match custom {
            Some(c) => self.sign_in_gmail(None, c, true).await,
            None => self.sign_in_gmail(None, ClientConfig::load()?, false).await,
        }
    }

    /// Signs an existing account in again, replacing its token. Mail, labels and
    /// settings stay as they are. An account on its own client keeps that client.
    pub async fn reconnect_gmail(self: &Arc<Self>, account_id: &str) -> Result<String> {
        let acc = self.store.account(account_id)?;
        if acc.provider == "outlook" {
            return self.sign_in_outlook(Some(acc.email)).await;
        }
        let (cfg, custom) = tokens::consent_client(&acc.email)?;
        self.sign_in_gmail(Some(acc.email), cfg, custom).await
    }

    /// True when the account's running token was issued by another OAuth client.
    pub fn needs_reconnect(&self, account_id: &str) -> bool {
        self.workers
            .lock()
            .unwrap()
            .get(account_id)
            .is_some_and(|w| w.needs_reconnect)
    }

    /// Consents use the app's own client, so Google names the app's project, unless
    /// the user chose their own (`custom`); `expect` is the account being signed in again.
    async fn sign_in_gmail(
        self: &Arc<Self>,
        expect: Option<String>,
        cfg: ClientConfig,
        custom: bool,
    ) -> Result<String> {
        let req = oauth::AuthRequest {
            login_hint: expect.clone(),
            ..oauth::AuthRequest::gmail()
        };
        let pending = oauth::start_request(cfg.clone(), req).await?;
        let url = pending.auth_url.clone();
        *self.gmail_sign_in.lock().unwrap() = GmailSignIn {
            running: true,
            ..Default::default()
        };
        let svc = self.clone();
        let client_id = cfg.client_id.clone();
        tokio::spawn(async move {
            let result: Result<()> = async {
                let refresh = pending.finish(Duration::from_secs(600)).await?;
                let ts = Arc::new(TokenSource::new(cfg.clone(), refresh.clone()));
                let profile = Gmail::new(default_base(), ts).profile().await?;
                let email = profile.email_address;
                if let Some(want) = &expect
                    && !want.eq_ignore_ascii_case(&email)
                {
                    return Err(Error::other(format!(
                        "You chose {email} in Google. Sign in with {want}."
                    )));
                }
                let known = svc.store.account(&email).ok();
                tokens::save_grant(&email, &refresh, &cfg, custom)?;
                let acc = svc.store.add_account(&email, "gmail")?;
                svc.store.set_sign_in_expired(&acc.id, false)?;
                // A known, healthy account keeps its sync position and local data.
                if known.as_ref().is_none_or(|a| a.sync.state == "error") {
                    svc.store
                        .set_sync(&acc.id, "initial", None, profile.messages_total, None)?;
                }
                svc.start_account(&acc.id, &acc.email)?;
                info!(account = %acc.email, again = known.is_some(), "account signed in");
                Ok(())
            }
            .await;
            let outcome = match result {
                Ok(()) => GmailSignIn::default(),
                Err(e) => {
                    warn!("sign-in failed: {e}");
                    GmailSignIn::failed(&e, &client_id, custom)
                }
            };
            *svc.gmail_sign_in.lock().unwrap() = outcome;
            let _ = svc.events.send(Event::AccountsChanged);
        });
        Ok(url)
    }

    /// Begins Microsoft sign-in for an Outlook account (personal or work or school);
    /// the account appears once the browser flow completes.
    pub async fn add_outlook(self: &Arc<Self>) -> Result<String> {
        self.sign_in_outlook(None).await
    }

    /// `expect` is the account being signed in again.
    async fn sign_in_outlook(self: &Arc<Self>, expect: Option<String>) -> Result<String> {
        let cfg = oauth::ClientConfig::load_microsoft()
            .ok_or_else(|| Error::InvalidParams(OUTLOOK_UNAVAILABLE.into()))?;
        let req = oauth::AuthRequest {
            login_hint: expect.clone(),
            ..oauth::AuthRequest::outlook()
        };
        let pending = oauth::start_request(cfg.clone(), req).await?;
        let url = pending.auth_url.clone();
        *self.gmail_sign_in.lock().unwrap() = GmailSignIn {
            running: true,
            provider: Some("outlook".into()),
            ..Default::default()
        };
        let svc = self.clone();
        tokio::spawn(async move {
            let result: Result<()> = async {
                let refresh = pending.finish(Duration::from_secs(600)).await?;
                // The refresh token in hand may be replaced by the first refresh: keep the
                // newest for the account.
                let latest = Arc::new(Mutex::new(refresh.clone()));
                let l2 = latest.clone();
                let ts = Arc::new(TokenSource::rotating(
                    cfg.clone(),
                    refresh,
                    Box::new(move |new| *l2.lock().unwrap() = new.to_string()),
                ));
                let me = Outlook::new(outlook::default_base(), ts).me().await?;
                let email = me.email().ok_or_else(|| {
                    Error::other("Microsoft didn't say which email address this account uses.")
                })?;
                if let Some(want) = &expect
                    && !want.eq_ignore_ascii_case(&email)
                {
                    return Err(Error::other(format!(
                        "You chose {email} in Microsoft. Sign in with {want}."
                    )));
                }
                let known = svc.store.account(&email).ok();
                if known.as_ref().is_some_and(|a| a.provider != "outlook") {
                    return Err(Error::other(format!(
                        "{email} is already in Mermoz as a Google account."
                    )));
                }
                let refresh = latest.lock().unwrap().clone();
                tokens::save_grant(&email, &refresh, &cfg, false)?;
                let acc = svc.store.add_account(&email, "outlook")?;
                svc.store.set_sign_in_expired(&acc.id, false)?;
                if known.as_ref().is_none_or(|a| a.sync.state == "error") {
                    svc.store.set_sync(&acc.id, "initial", None, None, None)?;
                }
                svc.start_account(&acc.id, &acc.email)?;
                info!(account = %acc.email, again = known.is_some(), "outlook account signed in");
                Ok(())
            }
            .await;
            let outcome = match result {
                Ok(()) => GmailSignIn::default(),
                Err(e) => {
                    warn!("outlook sign-in failed: {e}");
                    GmailSignIn {
                        running: false,
                        error: Some(outlook_failure(&e)),
                        problem: None,
                        provider: Some("outlook".into()),
                    }
                }
            };
            *svc.gmail_sign_in.lock().unwrap() = outcome;
            let _ = svc.events.send(Event::AccountsChanged);
        });
        Ok(url)
    }

    /// A worker's loops stop once another worker replaces it (account signed in again).
    fn is_current(&self, w: &Arc<Worker>) -> bool {
        self.workers
            .lock()
            .unwrap()
            .get(&w.sync.account_id)
            .is_some_and(|cur| Arc::ptr_eq(cur, w))
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct JobState {
    pub running: bool,
    pub done: usize,
    pub total: usize,
    pub error: Option<String>,
}

/// The last mail sign-in. `problem` is set when Google refused it.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GmailSignIn {
    pub running: bool,
    pub error: Option<String>,
    pub problem: Option<SignInProblem>,
    /// "outlook" for a Microsoft sign-in; absent for Google.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// Why "Add Outlook account" can't start, in plain words: this build has no Microsoft
/// sign-in set up.
pub const OUTLOOK_UNAVAILABLE: &str =
    "Outlook sign-in isn't set up in this version of Mermoz yet. It will come with an update.";

/// A failed Microsoft sign-in in plain words. Microsoft's own words (AADSTS codes) go
/// to the log, not the screen.
pub fn outlook_failure(e: &Error) -> String {
    let said = match e {
        Error::Declined(s) => s.clone(),
        Error::Upstream { body, .. } => body.clone(),
        // Our own sentences are plain already; internal ones are not shown.
        Error::Other(s) if !s.contains("oauth") && !s.contains("outlook:") => return s.clone(),
        _ => String::new(),
    };
    let t = said.to_ascii_lowercase();
    if [
        "aadsts65001",
        "aadsts90094",
        "aadsts90099",
        "consent_required",
        "admin",
    ]
    .iter()
    .any(|w| t.contains(w))
    {
        "Your organization needs its administrator to approve Mermoz before you can add this account. Ask your IT team, then try again.".into()
    } else if t.contains("access_denied") {
        "Microsoft didn't give Mermoz access. If you didn't cancel, try again, or ask your IT team whether Mermoz is allowed.".into()
    } else {
        "Couldn't add the Outlook account. Please try again.".into()
    }
}

/// Google's refusal in plain words, with the client ID an admin would allow-list.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SignInProblem {
    pub kind: oauth::Refusal,
    pub message: String,
    pub client_id: String,
    /// The consent used the user's own sign-in client.
    pub custom: bool,
}

impl GmailSignIn {
    fn failed(e: &Error, client_id: &str, custom: bool) -> Self {
        let said = match e {
            Error::Declined(s) => s.clone(),
            Error::Upstream { body, .. } => body.clone(),
            _ => String::new(),
        };
        let problem = oauth::Refusal::classify(&said).map(|kind| SignInProblem {
            kind,
            message: kind.message(custom),
            client_id: client_id.to_string(),
            custom,
        });
        GmailSignIn {
            running: false,
            error: Some(match &problem {
                Some(p) => p.message.clone(),
                None => e.to_string(),
            }),
            problem,
            provider: None,
        }
    }
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
pub(crate) fn transient(e: &Error) -> bool {
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
        if !svc.is_current(&w) {
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

/// How often drafts written in Gmail are checked against Gmail's own Drafts.
const DRAFTS_CHECK_EVERY: Duration = Duration::from_secs(3600);

/// First wait before asking Google again whether a refused sign-in works after all.
const PROBE_FIRST: Duration = Duration::from_secs(15 * 60);
/// Longest wait between those checks.
const PROBE_MAX: Duration = Duration::from_secs(6 * 3600);

/// The wait before the next check of a refused sign-in: doubles, up to [`PROBE_MAX`].
pub(crate) fn next_probe(prev: Option<Duration>) -> Duration {
    prev.map_or(PROBE_FIRST, |d| (d * 2).min(PROBE_MAX))
}

/// While Google refuses the sign-in: no sync, no write-back (the queue waits), only a
/// rare single token check, in case access came back without a new sign-in. Signing in
/// again replaces the worker and ends this wait.
async fn wait_for_sign_in(svc: &Arc<Service>, w: &Arc<Worker>) {
    let mut wait = None;
    while svc.is_current(w) {
        let d = next_probe(wait);
        wait = Some(d);
        tokio::time::sleep(d).await;
        if !svc.is_current(w) {
            return;
        }
        if w.tokens.probe().await.is_ok() {
            info!(account = %w.sync.email, "Google accepts the sign-in again");
            let _ = svc.store.set_sign_in_expired(&w.sync.account_id, false);
            let _ = svc.events.send(Event::AccountsChanged);
            return;
        }
    }
}

/// Per-account loop: keep the import running, pull changes, push local edits.
async fn live_loop(svc: Arc<Service>, w: Arc<Worker>) {
    if !w.tokens.refused()
        && let Err(e) = w.sync.refresh_labels().await
    {
        warn!(account = %w.sync.email, "labels: {e}");
    }
    let mut drafts_checked: Option<std::time::Instant> = None;
    loop {
        if !svc.is_current(&w) {
            return;
        }
        if w.tokens.refused() {
            wait_for_sign_in(&svc, &w).await;
            continue;
        }
        ensure_import(&svc, &w);
        // On start (drafts stored as mail by older versions just moved back) and hourly.
        if drafts_checked.is_none_or(|t| t.elapsed() >= DRAFTS_CHECK_EVERY) {
            drafts_checked = Some(std::time::Instant::now());
            match w.sync.reconcile_gmail_drafts().await {
                Ok(_) | Err(Error::SignInExpired) => {}
                Err(e) => warn!(account = %w.sync.email, "Gmail drafts not checked: {e}"),
            }
        }
        match w.sync.incremental().await {
            // Only mail that changed is routed or judged again; echoes of our own
            // pushes change nothing.
            Ok(n) if n > 0 => {
                svc.router_kick.notify_one();
                svc.feed_kick.notify_one();
            }
            Ok(_) => {}
            // Recorded and shown once, when Google refused it.
            Err(Error::SignInExpired) => continue,
            Err(e) => warn!(account = %w.sync.email, "incremental: {e}"),
        }
        match w.sync.push_ops().await {
            Ok(n) if n > 0 => info!(account = %w.sync.email, pushed = n, "pushed local changes"),
            Ok(_) => {}
            Err(Error::SignInExpired) => continue,
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
                // Resumes where it stopped once the user signs in again.
                Err(Error::SignInExpired) => break,
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

/// What a routing call needs besides the thread: the client and the questions.
struct RouteKit {
    client: Arc<jev::Client>,
    questions: Arc<serde_json::Value>,
    known: Arc<Vec<String>>,
}

/// A provider is on hold: `reason` is "credits" (its credits ran out) or
/// "unavailable" (rate limited or down); nothing is asked before `until`.
#[derive(Debug, Clone)]
pub struct Pause {
    pub reason: String,
    pub until: std::time::Instant,
}

/// Wait before one probe call after the credits ran out: topping up is a human step.
const CREDITS_RETRY: Duration = Duration::from_secs(900);

impl Service {
    pub fn start_router(self: &Arc<Self>) {
        tokio::spawn(router_loop(self.clone()));
        tokio::spawn(crate::grouping::grouping_loop(self.clone()));
    }

    /// The System One provider the user picked.
    pub fn provider(&self) -> jev::Provider {
        self.store.jev_provider().unwrap_or_default()
    }

    /// The picked provider with its key, when it has one.
    pub fn active_key(&self) -> Option<(jev::Provider, String)> {
        let p = self.provider();
        jev::load_key(p).map(|k| (p, k))
    }

    /// Why the picked provider is on hold right now, if it is.
    pub fn paused(&self) -> Option<String> {
        self.paused_on(self.provider())
    }

    pub fn paused_on(&self, p: jev::Provider) -> Option<String> {
        self.pause
            .lock()
            .unwrap()
            .get(&p)
            .filter(|h| h.until > std::time::Instant::now())
            .map(|h| h.reason.clone())
    }

    /// Why suggestions cannot ask the AI for new threads right now (see
    /// [`jev::sort_hold`]); "no_key" is left to the caller's key check.
    pub fn sort_hold(&self) -> Option<String> {
        jev::sort_hold(
            self.active_key().is_some(),
            self.paused().as_deref(),
            self.store
                .background_daily_cap()
                .unwrap_or(jev::DEFAULT_DAILY_CAP),
            self.store.background_calls_today().unwrap_or(0),
        )
    }

    /// The other provider, in plain words, when the picked one ran out of credits and
    /// the other has a key. Only offered: switching is the user's call.
    pub fn offer(&self) -> Option<serde_json::Value> {
        let p = self.provider();
        let o = p.other();
        (self.paused_on(p).as_deref() == Some("credits") && jev::load_key(o).is_some()).then(|| {
            serde_json::json!({
                "provider": o.id(),
                "name": o.name(),
                "message": format!("{} credits ran out. Use {} instead?", p.name(), o.name()),
            })
        })
    }

    pub fn last_error(&self, p: jev::Provider) -> Option<String> {
        self.router_error.lock().unwrap().get(&p).cloned()
    }

    pub fn set_error(&self, p: jev::Provider, e: Option<String>) {
        let mut m = self.router_error.lock().unwrap();
        match e {
            Some(e) => m.insert(p, e),
            None => m.remove(&p),
        };
    }

    /// Puts `p` on hold after a refusal that says to stop asking for a while.
    pub fn pause_for(&self, p: jev::Provider, e: &Error, previous: Option<Duration>) -> Duration {
        let (reason, wait) = match e {
            Error::OutOfCredits(_) => ("credits", CREDITS_RETRY),
            Error::RateLimited { retry_after } => {
                ("unavailable", jev::backoff(*retry_after, previous))
            }
            _ => ("unavailable", jev::backoff(None, previous)),
        };
        warn!(
            provider = p.id(),
            reason,
            wait_s = wait.as_secs(),
            "AI sorting paused"
        );
        self.pause.lock().unwrap().insert(
            p,
            Pause {
                reason: reason.into(),
                until: std::time::Instant::now() + wait,
            },
        );
        wait
    }

    /// A new key (or a top-up) lifts that provider's hold at once.
    pub fn resume(&self, p: jev::Provider) {
        self.pause.lock().unwrap().remove(&p);
    }

    /// A new daily limit lifts every hold.
    pub fn resume_all(&self) {
        self.pause.lock().unwrap().clear();
    }

    fn route_kit(&self, p: jev::Provider, key: &str) -> Result<RouteKit> {
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
            client: Arc::new(jev::Client::new(p, key.to_string())),
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

/// No credits, a refused key, a rate limit or an outage: stop and retry later, never a
/// verdict on the thread.
pub(crate) fn pause_reason(e: &Error) -> Option<Option<Duration>> {
    match e {
        Error::OutOfCredits(_) => Some(Some(CREDITS_RETRY)),
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

/// Threads one suggestion request covers: the card on screen and the next few.
pub const SUGGEST_MAX: usize = 6;

/// Requests in flight while preparing suggestions.
const SUGGEST_PARALLEL: usize = 4;

/// A thread's confident suggestion, and why, in plain words.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ThreadSuggestion {
    pub id: String,
    pub suggestion: mermoz_core::model::Suggestion,
    pub why: Option<String>,
}

/// What a suggestion request returns: the suggestions ready now, why the AI is on hold
/// if it is, and the other provider to offer when it ran out of credits.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SuggestBatch {
    pub suggestions: Vec<ThreadSuggestion>,
    pub paused: Option<String>,
    pub provider_name: &'static str,
    pub offer: Option<serde_json::Value>,
}

/// Most threads one ✨ on a column asks about.
pub const COLUMN_SUGGEST_MAX: usize = 50;

/// What ✨ on a column did: how many threads it looked at, which now show a
/// suggestion, and why the AI is on hold if it is.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ColumnSuggestions {
    pub asked: usize,
    pub suggested: Vec<String>,
    pub paused: Option<String>,
    pub provider_name: &'static str,
    pub offer: Option<serde_json::Value>,
}

impl Service {
    /// Confident suggestions for `ids` (at most [`SUGGEST_MAX`], the cards on screen
    /// and just after): stored ones, a learned sender rule, else one AI call per thread
    /// just before it is shown. Threads with nothing confident are left out.
    pub async fn suggest(self: &Arc<Self>, ids: &[String]) -> Result<SuggestBatch> {
        let ids = &ids[..ids.len().min(SUGGEST_MAX)];
        self.prepare_suggestions(ids).await?;
        let mut suggestions = vec![];
        for id in ids {
            if let Some(s) = self.shown_suggestion(id)? {
                suggestions.push(s);
            }
        }
        Ok(SuggestBatch {
            suggestions,
            paused: self.sort_hold().filter(|h| h != "no_key"),
            provider_name: self.provider().name(),
            offer: self.offer(),
        })
    }

    /// ✨ on a column: the same suggestions as [`Self::suggest`] for the newest
    /// `limit` threads of that open column (at most [`COLUMN_SUGGEST_MAX`]), shown as
    /// chips on its cards. Only on request, so a column is never sorted in the background.
    pub async fn suggest_column(
        self: &Arc<Self>,
        status: &str,
        limit: usize,
    ) -> Result<ColumnSuggestions> {
        let limit = limit.clamp(1, COLUMN_SUGGEST_MAX);
        let ids = self.store.column_to_suggest(status, limit as i64)?;
        self.prepare_suggestions(&ids).await?;
        let mut suggested = vec![];
        for id in &ids {
            if self.shown_suggestion(id)?.is_some() {
                suggested.push(id.clone());
            }
        }
        let _ = self.events.send(Event::ThreadsChanged(ids.clone()));
        Ok(ColumnSuggestions {
            asked: ids.len(),
            suggested,
            // Asked for by a click, so a missing key is said too.
            paused: self.sort_hold(),
            provider_name: self.provider().name(),
            offer: self.offer(),
        })
    }

    /// Stored suggestions for `ids` are kept, a learned sender rule answers without the
    /// AI, and the rest get one AI call each.
    async fn prepare_suggestions(self: &Arc<Self>, ids: &[String]) -> Result<()> {
        let mut to_route = vec![];
        for c in self.store.suggest_candidates(ids)? {
            if c.ready {
                // A No given since this suggestion was made applies to it too.
                self.store.close_if_refused(&c.id)?;
            } else if let Some(sg) = self.learned_suggestion(&c.id)? {
                self.store
                    .save_column_suggestion(&c.id, &sg, "learned", c.last_date)?;
            } else {
                to_route.push((c.id, c.last_date));
            }
        }
        if !to_route.is_empty() {
            self.route_for_suggestions(to_route).await?;
        }
        Ok(())
    }

    /// Asks the AI about these threads, within today's allowance and unless on hold.
    async fn route_for_suggestions(
        self: &Arc<Self>,
        mut to_route: Vec<(String, i64)>,
    ) -> Result<()> {
        let Some((p, key)) = self.active_key() else {
            return Ok(());
        };
        if self.sort_hold().is_some() {
            return Ok(());
        }
        let room = jev::calls_left(
            self.store.background_calls_today()?,
            self.store.background_daily_cap()?,
        );
        to_route.truncate(room.max(0) as usize);
        if to_route.is_empty() {
            return Ok(());
        }
        let kit = self.route_kit(p, &key)?;
        let outcomes = route_many(self, &kit, &to_route, SUGGEST_PARALLEL).await;
        let tokens = input_tokens(&outcomes);
        let ok = outcomes.iter().filter(|(_, _, r)| r.is_ok()).count() as i64;
        let calls_today = self.store.add_background_calls(ok)?;
        self.store.add_provider_calls(p, ok)?;
        info!(
            provider = p.id(),
            calls = ok,
            input_tokens = tokens,
            calls_today,
            "suggestions routed"
        );
        let mut backoff = None;
        for (tid, last_date, res) in outcomes {
            match res {
                Ok(r) => {
                    self.store
                        .save_column_suggestion(&tid, &r.suggestion, p.model(), last_date)?;
                }
                // No verdict is recorded: the thread is asked again later.
                Err(e) if pause_reason(&e).is_some() => {
                    backoff = Some(self.pause_for(p, &e, backoff));
                    self.set_error(p, Some(e.to_string()));
                }
                Err(e) => {
                    warn!(thread = %tid, "suggestion skipped thread: {e}");
                    self.store.save_column_suggestion(
                        &tid,
                        &Default::default(),
                        p.model(),
                        last_date,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// The pending suggestion of a thread as a card shows it: only what is confident
    /// and would change something. An Unread thread's stored suggestion is narrowed to
    /// that too, so accepting applies exactly what was shown.
    fn shown_suggestion(&self, thread_id: &str) -> Result<Option<ThreadSuggestion>> {
        let t = self.store.thread(thread_id)?;
        let Some(sg) = &t.suggestion else {
            return Ok(None);
        };
        let Some(mut shown) = jev::column_suggestion(sg, &t.status, &t.projects) else {
            return Ok(None);
        };
        shown.priority = sg.priority;
        if t.status == "unread" && (shown.status != sg.status || shown.projects != sg.projects) {
            let model = self.store.suggestion_model(thread_id)?.unwrap_or_default();
            self.store
                .save_suggestion(thread_id, &shown, &model, t.last_date)?;
        }
        let why = self.why(&t, &shown)?;
        Ok(Some(ThreadSuggestion {
            id: t.id,
            suggestion: shown,
            why,
        }))
    }

    /// A trusted sender's destination (see [`mermoz_core::store::TRUST_AFTER`]) as a
    /// suggestion, without asking the AI.
    fn learned_suggestion(
        &self,
        thread_id: &str,
    ) -> Result<Option<mermoz_core::model::Suggestion>> {
        let sender = self.store.thread_sender(thread_id)?;
        let Some(g) = self.store.sender_rule(&sender)? else {
            return Ok(None);
        };
        let days = jev::days_ago(
            self.store.thread(thread_id)?.last_date,
            mermoz_core::model::now_ms(),
        );
        let sg = mermoz_core::model::Suggestion {
            status_confidence: g.status.as_ref().map(|_| 1.0),
            project_confidence: (!g.projects.is_empty()).then_some(1.0),
            status: g.status,
            projects: g.projects,
            ..Default::default()
        };
        Ok(Some(jev::guard_stale(sg, days)))
    }

    /// The evidence behind a thread's suggestion, in plain words.
    fn why(
        &self,
        thread: &mermoz_core::model::ThreadSummary,
        sg: &mermoz_core::model::Suggestion,
    ) -> Result<Option<String>> {
        let model = self.store.suggestion_model(&thread.id)?;
        Ok(match model.as_deref() {
            Some("learned") => Some(format!(
                "Learned: mail from {} goes to {}",
                thread.last_from,
                target_words(sg)
            )),
            _ if !sg.projects.is_empty() => {
                let input = self.store.route_input(&thread.id)?;
                let p = &sg.projects[0];
                input
                    .sender_history
                    .iter()
                    .find(|(n, _)| n == p)
                    .filter(|_| input.peer_threads > 0)
                    .map(|(_, n)| {
                        format!(
                            "You filed {n} of your {} other threads with these people in {p}",
                            input.peer_threads
                        )
                    })
            }
            _ => None,
        })
    }
}

/// "Archive · Voyage": a destination in plain words.
fn target_words(sg: &mermoz_core::model::Suggestion) -> String {
    let mut parts: Vec<String> = sg
        .status
        .iter()
        .map(|s| {
            mermoz_core::model::DEFAULT_STATUSES
                .iter()
                .find(|(k, ..)| k == s)
                .map_or(s.clone(), |(_, name, ..)| name.to_string())
        })
        .collect();
    parts.extend(sg.projects.iter().cloned());
    parts.join(" · ")
}

/// New mail routed per round: one wave of requests, so the first suggestions land
/// after one call's latency rather than after a long batch.
const ROUTE_BATCH: i64 = 8;

/// Routes every new Unread thread while the picked provider has a key and is not on
/// hold. Runs apart from sync and RPC: nothing on screen waits for it.
async fn router_loop(svc: Arc<Service>) {
    let mut backoff = None;
    loop {
        let mut more = false;
        if let Some((p, key)) = svc
            .active_key()
            .filter(|(p, _)| svc.paused_on(*p).is_none())
        {
            match route_pending(&svc, p, &key).await {
                Ok((n, full)) => {
                    if n > 0 {
                        info!(provider = p.id(), routed = n, "routed new mail");
                    }
                    svc.set_error(p, None);
                    backoff = None;
                    more = full;
                }
                Err(e) => {
                    warn!(provider = p.id(), "router: {e}");
                    if pause_reason(&e).is_some() {
                        backoff = Some(svc.pause_for(p, &e, backoff));
                    }
                    svc.set_error(p, Some(e.to_string()));
                }
            }
        }
        if more {
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(poll_interval()) => {}
            _ = svc.router_kick.notified() => {}
        }
    }
}

/// Routes up to one batch of pending Unread threads, all in flight at once. Returns how
/// many were routed and whether the batch was full (more may be waiting).
async fn route_pending(svc: &Arc<Service>, p: jev::Provider, key: &str) -> Result<(usize, bool)> {
    let pending = svc.store.threads_to_route(ROUTE_BATCH)?;
    if pending.is_empty() {
        return Ok((0, false));
    }
    let full = pending.len() as i64 >= ROUTE_BATCH;
    let kit = svc.route_kit(p, key)?;
    let started = std::time::Instant::now();
    let outcomes = route_many(svc, &kit, &pending, ROUTE_BATCH as usize).await;
    let tokens = input_tokens(&outcomes);
    let answered = outcomes.iter().filter(|(_, _, r)| r.is_ok()).count() as i64;
    svc.store.add_provider_calls(p, answered)?;
    let mut routed = vec![];
    let mut stop = None;
    for (tid, last_date, res) in outcomes {
        match res {
            Ok(r) => {
                svc.store
                    .save_suggestion(&tid, &r.suggestion, p.model(), last_date)?;
                routed.push(tid);
            }
            // A refused key or an outage stops the round; it retries on the next tick.
            Err(e) if pause_reason(&e).is_some() => stop = Some(e),
            // Anything else is specific to this thread: record "no suggestion" so it
            // is not retried forever.
            Err(e) => {
                warn!(thread = %tid, "router skipped thread: {e}");
                svc.store
                    .save_suggestion(&tid, &Default::default(), p.model(), last_date)?;
            }
        }
    }
    if tokens > 0 {
        info!(
            provider = p.id(),
            threads = routed.len(),
            input_tokens = tokens,
            wave_ms = started.elapsed().as_millis() as u64,
            "routed new mail"
        );
    }
    let n = routed.len();
    let _ = svc.events.send(Event::ThreadsChanged(routed));
    // Routed mail knows whether it is automated: grouping can take it now.
    svc.grouping_kick.notify_one();
    match stop {
        Some(
            e @ (Error::InvalidParams(_)
            | Error::Http(_)
            | Error::OutOfCredits(_)
            | Error::RateLimited { .. }),
        ) => Err(e),
        Some(_) => Err(Error::other(format!(
            "{} is unavailable right now",
            p.name()
        ))),
        None => Ok((n, full)),
    }
}

#[cfg(test)]
mod tests {
    use super::{import_needed, outlook_failure, pause_reason};
    use mermoz_core::Error;
    use rstest::rstest;

    #[rstest]
    #[case::admin_consent(Error::Declined("consent_required: AADSTS65001 The user or administrator has not consented".into()), "Your organization needs its administrator to approve Mermoz before you can add this account. Ask your IT team, then try again.")]
    #[case::cancelled(Error::Declined("access_denied: AADSTS65004 User declined to consent".into()), "Microsoft didn't give Mermoz access. If you didn't cancel, try again, or ask your IT team whether Mermoz is allowed.")]
    #[case::our_own_words(
        Error::other("You chose a@b.example in Microsoft. Sign in with c@d.example."),
        "You chose a@b.example in Microsoft. Sign in with c@d.example."
    )]
    #[case::internal(
        Error::other("oauth state mismatch"),
        "Couldn't add the Outlook account. Please try again."
    )]
    #[case::token_endpoint(Error::Upstream { status: 400, body: "invalid_grant AADSTS70000".into() }, "Couldn't add the Outlook account. Please try again.")]
    fn outlook_sign_in_failures_read_plainly(#[case] e: Error, #[case] want: &str) {
        assert_eq!(outlook_failure(&e), want);
    }

    #[rstest]
    fn out_of_credits_pauses_instead_of_judging_the_thread() {
        assert!(pause_reason(&Error::OutOfCredits("TypeSafe")).is_some());
    }

    #[rstest]
    fn a_thread_specific_error_is_no_pause() {
        assert!(
            pause_reason(&Error::Upstream {
                status: 400,
                body: String::new()
            })
            .is_none()
        );
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
