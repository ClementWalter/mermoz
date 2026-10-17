//! Foreground AI jobs reuse desktop feed judgments, card prompts and conversation context.

use mermoz_core::agent::{self, Provider};
use mermoz_core::chat::{self, ChatMessage};
use mermoz_core::feed::{self, dates, money, texts};
use mermoz_core::model::{new_id, now_ms};
use mermoz_core::store::Store;
use mermoz_core::{Error, Result, jev};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

#[derive(Clone, Deserialize)]
pub(crate) struct Configuration {
    pub provider: Provider,
    pub key: String,
    pub accounts: Vec<String>,
    pub sorter: Option<String>,
    #[serde(default)]
    pub sorter_key: String,
}

#[derive(Clone, Default, Serialize)]
pub(crate) struct Progress {
    pub enabled: bool,
    pub running: bool,
    pub completed: usize,
    pub total: usize,
    pub error: Option<String>,
    pub thread_id: Option<String>,
    pub text: String,
}

#[derive(Default)]
pub(crate) struct MobileAI {
    configuration: Mutex<Option<Configuration>>,
    progress: Arc<Mutex<Progress>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl MobileAI {
    pub fn status(&self) -> Progress {
        self.progress.lock().unwrap().clone()
    }

    pub fn pause(&self, runtime: &Runtime) {
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
            runtime.block_on(async {
                let _ = task.await;
            });
        }
        self.progress.lock().unwrap().running = false;
    }

    pub fn configure(
        &self,
        store: &Store,
        runtime: &Runtime,
        config: Option<Configuration>,
    ) -> Result<()> {
        if let Some(c) = &config {
            if !matches!(c.provider.kind.as_str(), "openai" | "anthropic")
                || c.provider.auth != "key"
                || c.key.trim().is_empty()
                || c.provider.model.trim().is_empty()
                || c.accounts.is_empty()
            {
                return Err(Error::InvalidParams(
                    "Choose an AI, its model and the mailboxes it may read.".into(),
                ));
            }
            if let Some(sorter) = &c.sorter
                && (jev::Provider::parse(sorter).is_none() || c.sorter_key.trim().is_empty())
            {
                return Err(Error::InvalidParams(
                    "Instant sorting needs a TypeSafe or SubQ key.".into(),
                ));
            }
            for id in &c.accounts {
                store.account(id)?;
            }
            if c.provider.kind == "openai"
                && c.provider
                    .base_url
                    .as_deref()
                    .is_none_or(|url| url.trim().is_empty())
            {
                return Err(Error::InvalidParams(
                    "An OpenAI-compatible API base URL is required.".into(),
                ));
            }
            if let Some(base) = &c.provider.base_url {
                let url = url::Url::parse(base)
                    .map_err(|_| Error::InvalidParams("Invalid AI API URL.".into()))?;
                let local = url.host_str().is_some_and(|host| {
                    host == "localhost"
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                });
                if url.scheme() != "https" && !(url.scheme() == "http" && local) {
                    return Err(Error::InvalidParams(
                        "AI API connections require HTTPS.".into(),
                    ));
                }
            }
        }
        // Cancellation completes before consent or credentials change.
        self.pause(runtime);
        for account in store.accounts()? {
            let ids = config
                .as_ref()
                .filter(|c| c.accounts.contains(&account.id))
                .map(|c| vec![c.provider.id.clone()])
                .unwrap_or_default();
            store.set_account_providers(&account.id, &ids)?;
        }
        if let Some(c) = &config {
            store.save_provider(&c.provider)?;
        }
        *self.progress.lock().unwrap() = Progress {
            enabled: config.is_some(),
            ..Default::default()
        };
        *self.configuration.lock().unwrap() = config;
        Ok(())
    }

    fn config(&self) -> Result<Configuration> {
        self.configuration
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::InvalidParams("Connect your AI in Settings first.".into()))
    }

    pub fn refresh(&self, store: Arc<Store>, runtime: &Runtime, ids: Vec<String>) -> Result<()> {
        if self.status().running {
            return Ok(());
        }
        let c = self.config()?;
        *self.progress.lock().unwrap() = Progress {
            enabled: true,
            running: true,
            ..Default::default()
        };
        let progress = self.progress.clone();
        *self.task.lock().unwrap() = Some(runtime.spawn(async move {
            let result = refresh(&store, &c, &ids, &progress).await;
            finish(&progress, result);
        }));
        Ok(())
    }

    pub fn chat(
        &self,
        store: Arc<Store>,
        runtime: &Runtime,
        id: String,
        question: String,
    ) -> Result<()> {
        let c = self.config()?;
        let thread = store.thread(&id)?;
        if !c.accounts.contains(&thread.account_id) || question.trim().is_empty() {
            return Err(Error::InvalidParams(
                "Allow AI for this mailbox and write a question first.".into(),
            ));
        }
        if mermoz_core::demo_account::is_demo(&store.account(&thread.account_id)?) {
            return Err(Error::InvalidParams("Connect your mailbox to chat about your own mail. Demo cards use fictional examples.".into()));
        }
        // Interactive questions take priority over speculative card preparation.
        self.pause(runtime);
        *self.progress.lock().unwrap() = Progress {
            enabled: true,
            running: true,
            thread_id: Some(id.clone()),
            ..Default::default()
        };
        let progress = self.progress.clone();
        *self.task.lock().unwrap() = Some(runtime.spawn(async move {
            let result = answer(&store, &c, &id, &question, &progress).await;
            finish(&progress, result);
        }));
        Ok(())
    }
}

fn finish(progress: &Mutex<Progress>, result: Result<()>) {
    let mut p = progress.lock().unwrap();
    p.running = false;
    p.error = result.err().map(|error| match error {
        Error::RateLimited { .. } => "The AI is rate limited. Try again shortly.".into(),
        Error::OutOfCredits(_) => "Your AI account is out of credits.".into(),
        _ => "The AI couldn't finish. Check your provider, model, key and connection in Settings."
            .into(),
    });
}

async fn refresh(
    store: &Store,
    c: &Configuration,
    ids: &[String],
    progress: &Mutex<Progress>,
) -> Result<()> {
    let today = feed::today();
    progress.lock().unwrap().total = store.feed_texts_needed(ids)?.len();
    {
        let limit = jev::calls_left(
            store.background_calls_today()?,
            store.background_daily_cap()?,
        )
        .min(24);
        let mut candidates = vec![];
        for account in &c.accounts {
            candidates.extend(store.threads_to_judge_scoped(
                &today.to_string(),
                limit - candidates.len() as i64,
                Some(account),
            )?);
        }
        candidates.sort_by_key(|(id, ..)| {
            ids.iter()
                .position(|visible| visible == id)
                .unwrap_or(usize::MAX)
        });
        let client = c
            .sorter
            .as_deref()
            .and_then(jev::Provider::parse)
            .map(|sorter| jev::Client::new(sorter, c.sorter_key.clone()));
        for (id, for_date, backlog) in candidates {
            let thread = store.thread(&id)?;
            if !c.accounts.contains(&thread.account_id) {
                continue;
            }
            let (input, written) = store.feed_input(&id)?;
            let mut parts = vec![(input.subject.as_str(), feed::day_of(for_date))];
            for (m, at) in input.recent_messages.iter().zip(&written) {
                parts.push((m.text.as_str(), feed::day_of(*at)));
            }
            let dates = dates::candidates_in(&parts);
            if backlog && !feed::backlog_worth_judging(&dates, today) {
                store.save_judgment(
                    &id,
                    &Default::default(),
                    for_date,
                    false,
                    &today.to_string(),
                )?;
                continue;
            }
            let amounts = money::candidates(&Store::feed_text(&input));
            store.add_background_calls(1)?;
            let mut judgment = if let Some(client) = &client {
                let answers = client
                    .ask(&input.state(), &feed::questions(&dates, &amounts))
                    .await?;
                feed::judgment(&answers, &dates, &amounts)
            } else {
                crate::ai_judgment::judge(c, &input.state(), &dates, &amounts).await?
            };
            (judgment.sender_weight, judgment.typical_reply_days) = store.sender_stats(&id)?;
            store.save_judgment(&id, &judgment, for_date, true, &today.to_string())?;
            if ids.contains(&id)
                && !store
                    .feed_texts_needed(std::slice::from_ref(&id))?
                    .is_empty()
            {
                write_texts(store, c, &id, for_date).await?;
                progress.lock().unwrap().completed += 1;
            }
        }
    }
    let needed = store.feed_texts_needed(ids)?;
    for (id, date) in needed {
        let thread = store.thread(&id)?;
        if !c.accounts.contains(&thread.account_id) {
            continue;
        }
        write_texts(store, c, &id, date).await?;
        progress.lock().unwrap().completed += 1;
    }
    Ok(())
}

async fn write_texts(store: &Store, c: &Configuration, id: &str, date: i64) -> Result<()> {
    let thread = store.thread(id)?;
    let me = store.account(&thread.account_id)?.email;
    let messages = store.conversation_messages(id)?;
    let prompt = texts::user_prompt(
        store.feed_kind(id)?,
        &chat::today(),
        &me,
        &thread.subject,
        &messages,
    );
    let raw = agent::Client::new()
        .complete(
            &c.provider,
            &c.key,
            texts::SYSTEM,
            &prompt,
            &texts::schema(),
        )
        .await?;
    let words = texts::parse(&raw)?;
    store.save_feed_texts(id, date, &c.provider.id, &Ok(words))
}

async fn answer(
    store: &Store,
    c: &Configuration,
    id: &str,
    question: &str,
    progress: &Mutex<Progress>,
) -> Result<()> {
    let thread = store.thread(id)?;
    let messages = store.conversation_messages(id)?;
    let history = store.my_past_messages(id, 5)?;
    let statuses: Vec<_> = store
        .statuses()?
        .into_iter()
        .filter(|s| s.key != "review" && s.key != "unread")
        .map(|s| (s.key, s.name))
        .collect();
    let projects: Vec<_> = store.projects()?.into_iter().map(|p| p.name).collect();
    let me = store.account(&thread.account_id)?.email;
    let today = chat::today();
    let system = chat::system_prompt(&chat::ThreadContext {
        today: &today,
        me: &me,
        thread: &thread,
        messages: &messages,
        history: &history,
        statuses: &statuses,
        projects: &projects,
    });
    let past = store.chat_messages(id)?;
    let turns = chat::provider_turns(&past, question);
    store.add_chat_message(&ChatMessage {
        id: new_id("cht"),
        thread_id: id.into(),
        role: "user".into(),
        text: question.trim().into(),
        actions: vec![],
        searched: false,
        research_unavailable: false,
        created: now_ms(),
    })?;
    let mut delta = |text: &str| {
        progress.lock().unwrap().text = chat::visible_while_streaming(text).into();
    };
    let reply = chat::stream_reply(&c.provider, &c.key, &system, &turns, false, &mut delta).await?;
    let (text, actions) = chat::split_actions(&reply.text);
    store.add_chat_message(&ChatMessage {
        id: new_id("cht"),
        thread_id: id.into(),
        role: "assistant".into(),
        text,
        actions: chat::validate_actions(actions, &thread, &statuses, &projects),
        searched: reply.searched,
        research_unavailable: reply.research_unavailable,
        created: now_ms(),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    #[fixture]
    fn mailbox() -> (TempDir, crate::MobileEngine) {
        let home = tempfile::tempdir().unwrap();
        let engine = crate::MobileEngine::open(home.path()).unwrap();
        (home, engine)
    }

    #[rstest]
    fn ai_requires_explicit_configuration(mailbox: (TempDir, crate::MobileEngine)) {
        assert!(
            mailbox
                .1
                .dispatch(r#"{"method":"feed.refresh","params":{"ids":"[]"}}"#)
                .is_err()
        );
    }

    #[rstest]
    fn demo_feed_keeps_its_prepared_ai_words(mailbox: (TempDir, crate::MobileEngine)) {
        let feed = mailbox
            .1
            .dispatch(r#"{"method":"feed.list","params":{}}"#)
            .unwrap();
        assert!(
            feed["sections"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|s| s["cards"].as_array().unwrap())
                .any(|c| c["texts"]["draft"].as_str().is_some_and(|s| !s.is_empty()))
        );
    }

    #[rstest]
    fn empty_mailbox_consent_is_rejected(mailbox: (TempDir, crate::MobileEngine)) {
        let config = serde_json::json!({"provider":{"id":"p","name":"AI","kind":"anthropic","model":"test"},"key":"private-key-test","accounts":[]});
        let request = serde_json::json!({"method":"ai.configure","params":{"configuration":config.to_string()}});
        assert!(mailbox.1.dispatch(&request.to_string()).is_err());
    }
}
