//! The Feed on the daemon side: Jev judges open threads in the background (one call per
//! thread, redone when new mail arrives), and the account's AI writes a card's words
//! only when the app shows that card.

use crate::service::{Service, pause_reason, transient};
use mermoz_core::feed::{self, dates, later, money, texts};
use mermoz_core::model::now_ms;
use mermoz_core::store::ColumnScope;
use mermoz_core::sync::Event;
use mermoz_core::{Result, jev};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Threads judged per round; small, so new mail is picked up quickly.
const BATCH: i64 = 24;
/// Jev requests in flight.
const PARALLEL: usize = 8;
/// Card words written at once; the visible handful, never the whole feed.
const TEXTS_PARALLEL: usize = 3;

/// The feed as the app shows it, for one board scope.
pub fn list(svc: &Service, scope: &ColumnScope) -> Result<Value> {
    let st = &svc.store;
    let units = st.feed_units(scope, feed::today(), now_ms())?;
    let mut f = feed::assemble(units, feed::today(), &st.ai_accounts()?);
    st.complete_feed(&mut f)?;
    let paused = svc.paused();
    let sorting = jev::load_key().is_some() && f.judged < f.open && paused.is_none();
    let mut v = serde_json::to_value(f)?;
    v["sorting"] = json!(sorting);
    v["paused"] = json!(paused);
    Ok(v)
}

/// How a judging round ended.
enum Round {
    /// More may be waiting.
    Worked,
    Idle,
    Capped,
    Pause(Option<Duration>),
}

pub fn start(svc: &Arc<Service>) {
    tokio::spawn(judge_loop(svc.clone()));
    tokio::spawn(later_loop(svc.clone()));
}

/// The Later picker's quick choices for now, with when each comes back.
pub fn later_options() -> Value {
    let now = later::now();
    json!(
        later::options(now)
            .into_iter()
            .map(
                |o| json!({ "key": o.key, "label": o.label, "until": later::millis(o.at),
                             "when": later::describe(o.at, now) })
            )
            .collect::<Vec<_>>()
    )
}

/// What the user typed in the Later picker, resolved (null when it names no time).
pub fn later_parse(text: &str) -> Value {
    let now = later::now();
    match later::parse(text, now) {
        Some(at) => json!({ "until": later::millis(at), "when": later::describe(at, now) }),
        None => Value::Null,
    }
}

/// Brings put-off mail back when its time comes: Unread again, first in the feed.
async fn later_loop(svc: Arc<Service>) {
    loop {
        match svc.store.due_snoozes(now_ms()) {
            Ok(due) if !due.is_empty() => {
                let mut back = vec![];
                for id in due {
                    match svc.store.wake_snoozed(&id) {
                        Ok(_) => back.push(id),
                        Err(e) => warn!(thread = %id, "later: {e}"),
                    }
                }
                for id in &back {
                    if let Ok((acc, _)) = svc.store.provider_thread_id(id) {
                        svc.kick(&acc);
                    }
                }
                info!(back = back.len(), "mail back from Later");
                let _ = svc.events.send(Event::ThreadsChanged(back));
                let _ = svc.events.send(Event::FeedChanged);
            }
            Ok(_) => {}
            Err(e) => warn!("later: {e}"),
        }
        tokio::select! {
            _ = tokio::time::sleep(crate::service::poll_interval()) => {}
            _ = svc.later_kick.notified() => {}
        }
    }
}

/// Judges every open thread once per new message, while a TypeSafe key is set.
async fn judge_loop(svc: Arc<Service>) {
    let mut last_backoff: Option<Duration> = None;
    loop {
        // While the AI is on hold (credits ran out, rate limit), the feed asks nothing.
        let round = match jev::load_key() {
            None => Round::Idle,
            Some(_) if svc.paused().is_some() => Round::Idle,
            Some(key) => judge_round(&svc, &key).await.unwrap_or_else(|e| {
                warn!("feed: {e}");
                Round::Pause(None)
            }),
        };
        let wait = match round {
            Round::Worked => Duration::from_millis(20),
            Round::Idle => Duration::from_secs(30),
            Round::Capped => Duration::from_secs(600),
            Round::Pause(asked) => jev::backoff(asked, last_backoff),
        };
        last_backoff = match round {
            Round::Pause(_) => Some(wait),
            _ => None,
        };
        if matches!(round, Round::Pause(_)) {
            // A kick must not cut a backoff short.
            tokio::time::sleep(wait).await;
        } else {
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = svc.feed_kick.notified() => {}
            }
        }
    }
}

async fn judge_round(svc: &Arc<Service>, key: &str) -> Result<Round> {
    let st = &svc.store;
    let today = feed::today();
    let today_s = today.to_string();
    let left = jev::calls_left(st.background_calls_today()?, st.background_daily_cap()?);
    if left == 0 {
        return Ok(Round::Capped);
    }
    let batch = st.threads_to_judge(&today_s, left.min(BATCH))?;
    if batch.is_empty() {
        return Ok(Round::Idle);
    }
    let client = Arc::new(jev::Client::new(key.to_string()));
    let mut set = tokio::task::JoinSet::new();
    let mut outcomes = vec![];
    let mut scanned = vec![];
    for (tid, last_date, backlog) in batch {
        let input = st.route_input(&tid)?;
        let text = mermoz_core::store::Store::feed_text(&input);
        let found = dates::candidates(&text, feed::day_of(last_date));
        // Backlog is only worth a call when a date it mentions is close.
        if backlog && !feed::backlog_worth_judging(&found, today) {
            st.save_judgment(&tid, &Default::default(), last_date, false, &today_s)?;
            scanned.push(tid);
            continue;
        }
        let amounts = money::candidates(&text);
        if set.len() >= PARALLEL
            && let Some(Ok(done)) = set.join_next().await
        {
            outcomes.push(done);
        }
        let (client, s, q) = (
            client.clone(),
            input.state(),
            feed::questions(&found, &amounts),
        );
        set.spawn(async move {
            let res = client.ask(&s, &q).await;
            (tid, last_date, found, amounts, res)
        });
    }
    while let Some(joined) = set.join_next().await {
        if let Ok(done) = joined {
            outcomes.push(done);
        }
    }
    st.add_background_calls(outcomes.len() as i64)?;
    let mut judged = scanned;
    let mut pause = None;
    for (tid, last_date, found, amounts, res) in outcomes {
        match res {
            Ok(answers) => {
                let mut j = feed::judgment(&answers, &found, &amounts);
                (j.sender_weight, j.typical_reply_days) = st.sender_stats(&tid)?;
                st.save_judgment(&tid, &j, last_date, true, &today_s)?;
                judged.push(tid);
            }
            Err(e) => match pause_reason(&e) {
                Some(asked) => {
                    if matches!(e, mermoz_core::Error::OutOfCredits) {
                        // The same hold as the router's: everything waits for a top-up.
                        svc.pause_for(&e, None);
                    }
                    pause = Some(asked.or(pause.flatten()))
                }
                // Specific to this thread: record "no judgment" so it is not retried forever.
                None => {
                    warn!(thread = %tid, "feed skipped thread: {e}");
                    st.save_judgment(&tid, &Default::default(), last_date, true, &today_s)?;
                }
            },
        }
    }
    info!(judged = judged.len(), "feed round");
    let _ = svc.events.send(Event::FeedChanged);
    Ok(match pause {
        Some(asked) => Round::Pause(asked),
        None => Round::Worked,
    })
}

/// Asks the account's AI for the words of these cards, in the background; returns how
/// many were queued. Cards whose words are fresh, or already being written, are skipped.
pub fn request_texts(svc: &Arc<Service>, ids: &[String]) -> Result<usize> {
    let needed = svc.store.feed_texts_needed(ids)?;
    let queued: Vec<(String, i64)> = {
        let mut inflight = svc.feed_inflight.lock().unwrap();
        needed
            .into_iter()
            .filter(|(id, _)| inflight.insert(id.clone()))
            .collect()
    };
    let n = queued.len();
    if n > 0 {
        let svc = svc.clone();
        tokio::spawn(async move {
            let mut set = tokio::task::JoinSet::new();
            for (tid, for_date) in queued {
                if set.len() >= TEXTS_PARALLEL {
                    let _ = set.join_next().await;
                }
                let svc = svc.clone();
                set.spawn(async move {
                    if let Err(e) = write_texts(&svc, &tid, for_date).await {
                        warn!(thread = %tid, "card words: {e}");
                    }
                    svc.feed_inflight.lock().unwrap().remove(&tid);
                });
            }
            while set.join_next().await.is_some() {}
        });
    }
    Ok(n)
}

/// Writes one card's words with the AI allowed for its account; nothing leaves the
/// device for an account without one.
async fn write_texts(svc: &Arc<Service>, tid: &str, for_date: i64) -> Result<()> {
    let st = &svc.store;
    let thread = st.thread(tid)?;
    let Some(pr) = st.provider_for_account(&thread.account_id)? else {
        return Ok(());
    };
    let key = crate::service::credential(&pr).await?;
    let me = st.account(&thread.account_id)?.email;
    let kind = st.feed_kind(tid)?;
    let messages = st.messages(tid)?;
    let user = texts::user_prompt(
        kind,
        &mermoz_core::chat::today(),
        &me,
        &thread.subject,
        &messages,
    );
    let outcome = mermoz_core::agent::Client::new()
        .complete(&pr, &key, texts::SYSTEM, &user, &texts::schema())
        .await
        .and_then(|raw| texts::parse(&raw));
    if let Err(e) = &outcome
        && transient(e)
    {
        // Retried the next time the card is shown.
        return Err(mermoz_core::Error::other(e.to_string()));
    }
    st.save_feed_texts(tid, for_date, &pr.id, &outcome)?;
    let _ = svc.events.send(Event::FeedChanged);
    Ok(())
}
