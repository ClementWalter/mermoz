//! Mobile mailbox operations and resumable foreground sync share the desktop store.

use crate::login::{self, Credential, Login};
use mermoz_core::gmail::Gmail;
use mermoz_core::model::{Draft, new_id, now_ms};
use mermoz_core::oauth::TokenSource;
use mermoz_core::store::{Store, ThreadFilter, ThreadPatch};
use mermoz_core::sync::AccountSync;
use mermoz_core::{Error, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

pub struct MobileEngine {
    pub(crate) store: Arc<Store>,
    runtime: Runtime,
    login: Mutex<Option<Login>>,
    workers: Mutex<HashMap<String, Arc<AccountSync>>>,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    gmail_base: String,
    token_url: String,
    ai: crate::ai::MobileAI,
}

#[derive(Deserialize)]
#[serde(tag = "method", content = "params")]
enum Request {
    #[serde(rename = "feed.list")]
    Feed {},
    #[serde(rename = "ai.configure")]
    AIConfigure { configuration: String },
    #[serde(rename = "ai.status")]
    AIStatus {},
    #[serde(rename = "feed.refresh")]
    FeedRefresh { ids: String },
    #[serde(rename = "chat.list")]
    ChatList { id: String },
    #[serde(rename = "chat.send")]
    ChatSend { id: String, question: String },
    #[serde(rename = "threads.list")]
    Threads {
        status: Option<String>,
        offset: Option<String>,
    },
    #[serde(rename = "threads.get")]
    Thread { id: String },
    #[serde(rename = "threads.update")]
    Update { id: String, status: String },
    #[serde(rename = "threads.read")]
    Read { id: String },
    #[serde(rename = "search")]
    Search { query: String },
    #[serde(rename = "statuses.list")]
    Statuses {},
    #[serde(rename = "accounts.list")]
    Accounts {},
    #[serde(rename = "accounts.resume")]
    Resume(Credential),
    #[serde(rename = "accounts.remove")]
    Remove { id: String },
    #[serde(rename = "auth.start")]
    Login { client_id: String },
    #[serde(rename = "auth.finish")]
    Finish { callback: String },
    #[serde(rename = "sync.start")]
    Sync {},
    #[serde(rename = "sync.pause")]
    Pause {},
    #[serde(rename = "drafts.reply")]
    Reply { id: String, body: String },
    #[serde(rename = "drafts.send")]
    Send { id: String },
    #[serde(rename = "drafts.list")]
    Drafts {},
}

impl MobileEngine {
    pub fn open(home: &Path) -> Result<Self> {
        std::fs::create_dir_all(home)?;
        let store = Arc::new(Store::open(
            &home.join("mermoz.sqlite"),
            &home.join("blobs"),
        )?);
        // Fictional mail provides an offline trial before the first Google consent.
        if store.setting("mobile_started")?.as_deref() != Some("true") {
            if store.accounts()?.is_empty() {
                mermoz_core::demo_account::add(&store)?;
            }
            store.set_setting("mobile_started", "true")?;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        Ok(Self {
            store,
            runtime,
            login: Mutex::new(None),
            workers: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            gmail_base: "https://gmail.googleapis.com/gmail/v1/users/me".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            ai: Default::default(),
        })
    }

    pub(crate) fn dispatch(&self, request: &str) -> std::result::Result<Value, String> {
        let request: Request = serde_json::from_str(request).map_err(|e| e.to_string())?;
        self.execute(request).map_err(|e| e.to_string())
    }

    fn execute(&self, request: Request) -> Result<Value> {
        match request {
            Request::Feed {} => {
                let today = mermoz_core::feed::today();
                let units = self
                    .store
                    .feed_units(&Default::default(), today, now_ms())?;
                let mut feed =
                    mermoz_core::feed::assemble(units, today, &self.store.ai_accounts()?);
                self.store.complete_feed(&mut feed)?;
                Ok(json!(feed))
            }
            Request::AIConfigure { configuration } => {
                self.ai.configure(
                    &self.store,
                    &self.runtime,
                    serde_json::from_str(&configuration)?,
                )?;
                Ok(json!({}))
            }
            Request::AIStatus {} => Ok(json!(self.ai.status())),
            Request::FeedRefresh { ids } => {
                self.ai.refresh(
                    self.store.clone(),
                    &self.runtime,
                    serde_json::from_str(&ids)?,
                )?;
                Ok(json!({}))
            }
            Request::ChatList { id } => Ok(json!(self.store.chat_messages(&id)?)),
            Request::ChatSend { id, question } => {
                self.ai
                    .chat(self.store.clone(), &self.runtime, id, question)?;
                Ok(json!({}))
            }
            Request::Threads { status, offset } => {
                let (threads, total) = self.store.threads(&ThreadFilter {
                    status: status.map(|s| vec![s]),
                    limit: 100,
                    offset: offset
                        .map(|s| s.parse::<i64>())
                        .transpose()
                        .map_err(|_| Error::InvalidParams("Invalid page offset".into()))?
                        .unwrap_or(0)
                        .max(0),
                    conversations: true,
                    ..Default::default()
                })?;
                Ok(json!({ "threads": threads, "total": total }))
            }
            Request::Thread { id } => Ok(
                json!({ "thread": self.store.thread(&id)?, "messages": self.store.conversation_messages(&id)? }),
            ),
            Request::Read { id } => {
                self.store.mark_read(&id, true)?;
                self.kick_all()?;
                Ok(json!({}))
            }
            Request::Update { id, status } => {
                self.store.thread(&id)?;
                self.store.update_threads(
                    std::slice::from_ref(&id),
                    &ThreadPatch {
                        status: Some(status),
                        ..Default::default()
                    },
                )?;
                self.kick_all()?;
                Ok(json!(self.store.thread(&id)?))
            }
            Request::Search { query } => {
                Ok(json!({ "threads": self.store.search(&query, 200, true)? }))
            }
            Request::Statuses {} => Ok(json!(self.store.statuses()?)),
            Request::Accounts {} => Ok(json!(self.store.accounts()?)),
            Request::Login { client_id } => {
                let login = Login::new(client_id)?;
                let result =
                    json!({ "url": login.auth_url, "scheme": login.redirect.split(':').next() });
                *self.login.lock().unwrap() = Some(login);
                Ok(result)
            }
            Request::Finish { callback } => self.finish_login(&callback),
            Request::Resume(credential) => {
                let account = self.store.account(&credential.account_id)?;
                if account.provider != "gmail" || credential.refresh_token.trim().is_empty() {
                    return Err(Error::InvalidParams(
                        "A Gmail credential is required".into(),
                    ));
                }
                let mut cfg = login::client(&credential.client_id)?;
                cfg.token_url = self.token_url.clone();
                let _guard = self.runtime.enter();
                let gmail = Arc::new(Gmail::new(
                    self.gmail_base.clone(),
                    Arc::new(TokenSource::new(cfg, credential.refresh_token)),
                ));
                let (events, _) = tokio::sync::broadcast::channel(32);
                self.workers.lock().unwrap().insert(
                    account.id.clone(),
                    Arc::new(AccountSync {
                        store: self.store.clone(),
                        backend: mermoz_core::provider::Backend::Gmail(gmail),
                        account_id: account.id,
                        email: account.email,
                        events,
                        read_only: false,
                    }),
                );
                Ok(json!({}))
            }
            Request::Remove { id } => {
                self.ai.pause(&self.runtime);
                // Complete cancellation before deleting data so a worker cannot repopulate it.
                let task = self.tasks.lock().unwrap().remove(&id);
                if let Some(task) = task {
                    task.abort();
                    self.runtime.block_on(async {
                        let _ = task.await;
                    });
                }
                self.workers.lock().unwrap().remove(&id);
                self.store.remove_account(&id)?;
                let accounts = self.store.accounts()?;
                if !accounts.iter().any(|a| a.provider == "gmail") {
                    for demo in accounts
                        .iter()
                        .filter(|a| mermoz_core::demo_account::is_demo(a))
                    {
                        self.store.set_account_hidden(&demo.id, false)?;
                    }
                }
                Ok(json!({}))
            }
            Request::Sync {} => Ok(json!({ "started": self.kick_all()? })),
            Request::Pause {} => {
                self.ai.pause(&self.runtime);
                let tasks: Vec<_> = self
                    .tasks
                    .lock()
                    .unwrap()
                    .drain()
                    .map(|(_, task)| task)
                    .collect();
                self.runtime.block_on(async {
                    for task in tasks {
                        task.abort();
                        let _ = task.await;
                    }
                });
                Ok(json!({}))
            }
            Request::Reply { id, body } => Ok(json!(self.reply(&id, body)?)),
            Request::Drafts {} => Ok(json!(self.store.drafts(None)?)),
            Request::Send { id } => {
                let draft = self.store.draft(&id)?;
                if draft.state != "draft" {
                    return Err(Error::other(
                        "This reply was sent or delivery is unconfirmed. Check Sent before trying again.",
                    ));
                }
                let account = self.store.account(&draft.account_id)?;
                if mermoz_core::demo_account::is_demo(&account) {
                    mermoz_core::demo_account::deliver(&self.store, &account, &draft, None)?;
                } else {
                    let worker = self
                        .workers
                        .lock()
                        .unwrap()
                        .get(&account.id)
                        .cloned()
                        .ok_or_else(|| {
                            Error::other("Reconnect this Gmail account before sending.")
                        })?;
                    // An interrupted send must not become a silently repeatable draft.
                    self.store.set_draft_state(&id, "sending")?;
                    let result = self.runtime.block_on(async {
                        tokio::time::timeout(
                            std::time::Duration::from_secs(60),
                            worker.send_draft(&draft, None),
                        )
                        .await
                        .map_err(|_| Error::other("Delivery timed out"))?
                    });
                    if let Err(error) = result {
                        if self.store.draft(&id)?.state == "sent" {
                            return Ok(json!(self.store.draft(&id)?));
                        }
                        let retryable = match &error {
                            Error::Upstream {
                                status: 400 | 401 | 403 | 404 | 429,
                                ..
                            } => true,
                            Error::Http(error) => error.is_connect(),
                            _ => false,
                        };
                        self.store
                            .set_draft_state(&id, if retryable { "draft" } else { "uncertain" })?;
                        return Err(Error::other(if retryable {
                            "Gmail couldn't send this reply. The draft is saved; check your connection and account access."
                        } else {
                            "Delivery couldn't be confirmed. Check Gmail Sent before writing another reply."
                        }));
                    }
                }
                Ok(json!(self.store.draft(&id)?))
            }
        }
    }

    fn finish_login(&self, callback: &str) -> Result<Value> {
        let login = self
            .login
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| Error::other("Start sign-in again."))?;
        let (client_id, grant) = self
            .runtime
            .block_on(login.finish(callback, &self.token_url))?;
        let cfg = login::client(&client_id)?;
        let profile = self.runtime.block_on(async {
            let gmail = Gmail::new(
                self.gmail_base.clone(),
                Arc::new(TokenSource::fixed(
                    cfg,
                    grant.access_token,
                    grant.expires_in,
                )),
            );
            gmail.profile().await
        })?;
        let account = self.store.add_account(&profile.email_address, "gmail")?;
        for demo in self
            .store
            .accounts()?
            .into_iter()
            .filter(mermoz_core::demo_account::is_demo)
        {
            self.store.set_account_hidden(&demo.id, true)?;
        }
        let credential = Credential {
            account_id: account.id.clone(),
            client_id,
            refresh_token: grant.refresh_token,
        };
        Ok(json!({ "account": account, "credential": credential }))
    }

    fn reply(&self, id: &str, body: String) -> Result<Draft> {
        if body.trim().is_empty() {
            return Err(Error::InvalidParams("Write a reply first.".into()));
        }
        let thread = self.store.thread(id)?;
        let account = self.store.account(&thread.account_id)?;
        let messages = self.store.conversation_messages(id)?;
        let target = mermoz_core::reply::target(&messages)
            .ok_or_else(|| Error::other("This conversation has nobody to reply to."))?;
        let (to, cc, subject) = self
            .store
            .reply_defaults(&target.thread_id, &account.email)?;
        if to.is_empty() {
            return Err(Error::other("This reply has no recipient."));
        }
        let now = now_ms();
        self.store.create_draft(&Draft {
            id: new_id("dft"),
            account_id: account.id,
            thread_id: Some(target.thread_id.clone()),
            to,
            cc,
            subject,
            body,
            state: "draft".into(),
            origin: "app".into(),
            created: now,
            updated: now,
            attachments: vec![],
            reply_to_message_id: Some(target.id.clone()),
        })
    }

    fn kick_all(&self) -> Result<usize> {
        let workers = self.workers.lock().unwrap();
        let mut tasks = self.tasks.lock().unwrap();
        let mut started = 0;
        for (id, worker) in workers.iter() {
            if tasks.get(id).is_some_and(|task| !task.is_finished()) {
                continue;
            }
            let worker = worker.clone();
            let store = self.store.clone();
            let id = id.clone();
            self.store.set_sync(&id, "syncing", None, None, None)?;
            tasks.insert(
                id.clone(),
                self.runtime.spawn(async move {
                    let result: Result<()> = async {
                        worker.refresh_labels().await?;
                        worker.push_ops().await?;
                        let key = format!("mobile_imported:{id}");
                        if store.setting(&key)?.as_deref() != Some("true") {
                            worker.initial_sync().await?;
                            store.set_setting(&key, "true")?;
                        }
                        worker.incremental().await?;
                        worker.push_ops().await?;
                        store.set_sync(&id, "idle", None, None, None)
                    }
                    .await;
                    if let Err(error) = result {
                        let message = match error {
                            Error::Upstream {
                                status: 400 | 401 | 403,
                                ..
                            } => "Gmail access expired or was refused. Reconnect this account."
                                .to_owned(),
                            _ => {
                                "Couldn't sync Gmail. Your local mail is available; pull to retry."
                                    .to_owned()
                            }
                        };
                        let _ = store.set_sync(&id, "error", None, None, Some(&message));
                    }
                }),
            );
            started += 1;
        }
        Ok(started)
    }
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod network_tests;
