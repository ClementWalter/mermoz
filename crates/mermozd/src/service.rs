//! Daemon state: the store, one sync worker pair per account, and the event bus.

use mermoz_core::gmail::{Gmail, default_base};
use mermoz_core::oauth::{self, ClientConfig, TokenSource};
use mermoz_core::store::Store;
use mermoz_core::sync::{AccountSync, Event};
use mermoz_core::{Error, Result, tokens};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, broadcast};
use tracing::{info, warn};

pub struct Worker {
    pub sync: Arc<AccountSync>,
    pub kick: Arc<Notify>,
    importing: Arc<Mutex<bool>>,
}

pub struct Service {
    pub store: Arc<Store>,
    pub events: broadcast::Sender<Event>,
    workers: Mutex<HashMap<String, Arc<Worker>>>,
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
            store,
            events,
            workers: Mutex::new(HashMap::new()),
        })
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
        let cfg = ClientConfig::load()?;
        let refresh = tokens::load(email)?;
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
        });
        self.workers
            .lock()
            .unwrap()
            .insert(account_id.to_string(), worker.clone());
        tokio::spawn(live_loop(self.clone(), worker));
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
                let ts = Arc::new(TokenSource::new(cfg, refresh.clone()));
                let profile = Gmail::new(default_base(), ts).profile().await?;
                tokens::save(&profile.email_address, &refresh)?;
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

fn ensure_import(svc: &Arc<Service>, w: &Arc<Worker>) {
    let state = svc
        .store
        .account(&w.sync.account_id)
        .map(|a| a.sync.state)
        .unwrap_or_default();
    if state != "initial" && state != "syncing" {
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
