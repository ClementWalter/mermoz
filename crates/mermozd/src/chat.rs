//! "Chat about this" on the daemon side: gathers a thread's context, streams the AI's
//! answer to subscribers as `chat_delta` events and stores both sides of the conversation.

use crate::service::Service;
use mermoz_core::chat::{self, ChatMessage};
use mermoz_core::model::{new_id, now_ms};
use mermoz_core::sync::Event;
use mermoz_core::{Error, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How many of the user's earlier messages with these people the AI sees.
const HISTORY: i64 = 5;
/// Streaming events are spaced out so a fast answer can't flood slow clients.
const DELTA_EVERY: Duration = Duration::from_millis(60);

/// Asks the account's AI about a thread; returns the stored answer.
pub async fn send(
    svc: &Arc<Service>,
    thread_id: &str,
    question: &str,
    research: bool,
) -> Result<ChatMessage> {
    let st = &svc.store;
    if question.trim().is_empty() {
        return Err(Error::InvalidParams("Write a question first.".into()));
    }
    let thread = st.thread(thread_id)?;
    let pr = st
        .provider_for_account(&thread.account_id)?
        .ok_or_else(|| {
            Error::InvalidParams("Allow an AI for this account in Settings › Agents first.".into())
        })?;
    let key = crate::service::credential(&pr).await?;
    let me = st.account(&thread.account_id)?.email;
    // The chat covers the whole conversation the thread belongs to.
    let messages = st.conversation_messages(thread_id)?;
    let history = st.my_past_messages(thread_id, HISTORY)?;
    let statuses: Vec<(String, String)> = st
        .statuses()?
        .into_iter()
        .filter(|s| s.key != "review" && s.key != "unread")
        .map(|s| (s.key, s.name))
        .collect();
    let projects: Vec<String> = st.projects()?.into_iter().map(|p| p.name).collect();
    let today = chat::today();
    let docs = crate::drive::documents_for_chat(svc, thread_id).await;
    let mut system = chat::system_prompt(&chat::ThreadContext {
        today: &today,
        me: &me,
        thread: &thread,
        messages: &messages,
        history: &history,
        statuses: &statuses,
        projects: &projects,
    });
    system.push_str(&chat::render_documents(&docs));
    let past = st.chat_messages(thread_id)?;
    let turns = chat::provider_turns(&past, question);
    // The question is kept even if the AI fails, so the user sees what they asked.
    st.add_chat_message(&ChatMessage {
        id: new_id("cht"),
        thread_id: thread_id.to_string(),
        role: "user".into(),
        text: question.trim().to_string(),
        actions: vec![],
        searched: false,
        research_unavailable: false,
        created: now_ms(),
    })?;
    let events = svc.events.clone();
    let tid = thread_id.to_string();
    let mut last = Instant::now() - DELTA_EVERY;
    let mut on_text = move |raw: &str| {
        if last.elapsed() < DELTA_EVERY {
            return;
        }
        last = Instant::now();
        let _ = events.send(Event::ChatDelta {
            thread_id: tid.clone(),
            text: chat::visible_while_streaming(raw).to_string(),
        });
    };
    let reply = chat::stream_reply(&pr, &key, &system, &turns, research, &mut on_text).await?;
    let (text, raw_actions) = chat::split_actions(&reply.text);
    let answer = ChatMessage {
        id: new_id("cht"),
        thread_id: thread_id.to_string(),
        role: "assistant".into(),
        text,
        actions: chat::validate_actions(raw_actions, &thread, &statuses, &projects),
        searched: reply.searched,
        research_unavailable: reply.research_unavailable,
        created: now_ms(),
    };
    st.add_chat_message(&answer)?;
    Ok(answer)
}
