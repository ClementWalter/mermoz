//! Background grouping of Gmail threads into conversations.
//!
//! Each round: decide the stream of new threads (feeds and lists join in code), ask
//! Jev about each person's thread against its candidates, then let the account's AI
//! settle the uncertain pairs. Recent mail goes first at full speed; older mail
//! follows at a slower pace, within the same daily Jev allowance as background sorting.

use crate::service::{Service, credential};
use mermoz_core::conversation as conv;
use mermoz_core::store::{GroupingPlan, PairCandidate};
use mermoz_core::sync::Event;
use mermoz_core::{Error, Result, jev};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Threads whose stream is decided per round (local work only).
const CLASSIFY_BATCH: i64 = 200;
/// Threads judged by Jev per round, with this many requests in flight.
const JUDGE_BATCH: i64 = 24;
const JUDGE_PARALLEL: usize = 8;
/// Pairs settled by the AI per round (each takes seconds).
const AI_BATCH: i64 = 4;

#[derive(Debug, PartialEq)]
enum Round {
    /// Recent mail was grouped; more may be waiting.
    Recent,
    /// Only older mail was grouped: keep a gentle pace.
    Older,
    Idle,
    /// Jev refused or is overloaded, or today's allowance is spent.
    Pause(Option<Duration>),
}

pub async fn grouping_loop(svc: Arc<Service>) {
    let mut last_backoff: Option<Duration> = None;
    loop {
        let round = grouping_round(&svc).await.unwrap_or_else(|e| {
            warn!("grouping: {e}");
            Round::Pause(None)
        });
        let wait = match &round {
            Round::Recent => Duration::from_millis(20),
            Round::Older => Duration::from_secs(2),
            Round::Idle => crate::service::poll_interval(),
            Round::Pause(asked) => jev::backoff(*asked, last_backoff),
        };
        last_backoff = matches!(round, Round::Pause(_)).then_some(wait);
        if matches!(round, Round::Pause(_)) {
            tokio::time::sleep(wait).await;
        } else {
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = svc.grouping_kick.notified() => {}
            }
        }
    }
}

async fn grouping_round(svc: &Arc<Service>) -> Result<Round> {
    let key = jev::load_key();
    let st = &svc.store;
    let mut changed: Vec<String> = vec![];
    let mut recent = false;
    let mut worked = false;

    for tid in st.threads_to_group("pending", CLASSIFY_BATCH, key.is_some())? {
        worked = true;
        recent |= st.is_recent_for_grouping(&tid)?;
        if st.classify_for_grouping(&tid)? == GroupingPlan::Grouped {
            changed.push(tid);
        }
    }

    let mut pause = None;
    if let Some(key) = key {
        let left = jev::calls_left(st.background_calls_today()?, st.background_daily_cap()?);
        let batch = if left == 0 {
            pause = Some(None);
            vec![]
        } else {
            st.threads_to_group("judge", JUDGE_BATCH.min(left), false)?
        };
        if !batch.is_empty() {
            worked = true;
            let (joined, older_only, stop) = judge(svc, &key, &batch).await?;
            recent |= !older_only;
            changed.extend(joined);
            pause = stop.or(pause);
        }
    }

    let worked_before_ai = worked;
    let asked = st.threads_to_group("ai", AI_BATCH, false)?;
    if !asked.is_empty() {
        worked = true;
        recent |= asked
            .iter()
            .any(|t| st.is_recent_for_grouping(t).unwrap_or(false));
        let (joined, progressed) = settle_with_ai(svc, &asked).await;
        changed.extend(joined);
        // An unreachable AI is not progress: wait for the next round instead of spinning.
        if !progressed && !worked_before_ai {
            worked = false;
        }
    }

    if !changed.is_empty() {
        let _ = svc.events.send(Event::ThreadsChanged(changed));
    }
    Ok(match (pause, worked, recent) {
        (Some(asked), _, _) => Round::Pause(asked),
        (None, false, _) => Round::Idle,
        (None, true, true) => Round::Recent,
        (None, true, false) => Round::Older,
    })
}

/// Asks Jev about every candidate of each thread and applies the verdicts. Returns the
/// threads that joined a conversation, whether the batch held only older mail, and
/// why to pause when Jev refused or is overloaded.
async fn judge(
    svc: &Arc<Service>,
    key: &str,
    batch: &[String],
) -> Result<(Vec<String>, bool, Option<Option<Duration>>)> {
    let st = &svc.store;
    let client = Arc::new(jev::Client::new(key.to_string()));
    let questions = Arc::new(conv::questions());
    let mut older_only = true;
    let mut plans = vec![];
    for tid in batch {
        older_only &= !st.is_recent_for_grouping(tid)?;
        match st.grouping_candidates(tid)? {
            GroupingPlan::Judge {
                people,
                join_at,
                candidates,
            } => plans.push((tid.clone(), people, join_at, candidates)),
            _ => st.finish_grouping(tid)?,
        }
    }
    let mut set = tokio::task::JoinSet::new();
    let mut answers: Vec<(String, PairCandidate, Result<serde_json::Value>)> = vec![];
    for (tid, _, _, candidates) in &plans {
        for c in candidates.iter().cloned() {
            if set.len() >= JUDGE_PARALLEL
                && let Some(Ok(done)) = set.join_next().await
            {
                answers.push(done);
            }
            let (client, questions, tid) = (client.clone(), questions.clone(), tid.clone());
            set.spawn(async move {
                let res = client.ask(&c.state, &questions).await;
                (tid, c, res)
            });
        }
    }
    while let Some(joined) = set.join_next().await {
        if let Ok(done) = joined {
            answers.push(done);
        }
    }
    st.add_background_calls(answers.len() as i64)?;
    let mut stop = None;
    let mut joined = vec![];
    for (tid, people, join_at, _) in plans {
        let mine: Vec<_> = answers.iter().filter(|(t, ..)| *t == tid).collect();
        let mut verdicts = vec![];
        let mut failed = false;
        for (_, c, res) in mine {
            match res {
                Ok(a) => match conv::same_probability(a) {
                    Some(p) => verdicts.push((c.clone(), p)),
                    None => failed = true,
                },
                Err(e) => {
                    if let Some(asked) = pause_reason(e) {
                        stop = Some(asked);
                    }
                    failed = true;
                }
            }
        }
        // A thread with a failed call is judged again next round, never half-judged.
        if failed && stop.is_some() {
            continue;
        }
        if let Some(into) = st.apply_jev_verdicts(&tid, &people, join_at, &verdicts)? {
            info!(thread = %tid, conversation = %into, "joined a conversation");
            joined.push(tid);
        }
    }
    Ok((joined, older_only, stop))
}

fn pause_reason(e: &Error) -> Option<Option<Duration>> {
    match e {
        Error::RateLimited { retry_after } => Some(*retry_after),
        Error::InvalidParams(_) | Error::Http(_) => Some(None),
        Error::Upstream { status, .. } if *status == 429 || *status >= 500 => Some(None),
        _ => None,
    }
}

/// Lets the account's AI decide each uncertain pair. Without an allowed AI the pair
/// stays apart: two threads are never joined on a coin flip. Returns the threads that
/// joined, and whether any pair was settled.
async fn settle_with_ai(svc: &Arc<Service>, threads: &[String]) -> (Vec<String>, bool) {
    let st = &svc.store;
    let client = mermoz_core::agent::Client::new();
    let schema = conv::ai_schema();
    let mut joined = vec![];
    let mut progressed = false;
    for tid in threads {
        let Ok(Some((account, earlier, state))) = st.next_ai_pair(tid) else {
            let _ = st.finish_grouping(tid);
            progressed = true;
            continue;
        };
        let provider = st.provider_for_account(&account).ok().flatten();
        let verdict = match provider {
            None => Some(false),
            Some(p) => match credential(&p).await {
                Err(e) => {
                    warn!("grouping AI unavailable: {e}");
                    None
                }
                Ok(key) => {
                    let user = state.to_string();
                    match client
                        .complete(&p, &key, conv::AI_SYSTEM, &user, &schema)
                        .await
                    {
                        Ok(raw) => Some(conv::ai_verdict(&raw).unwrap_or(false)),
                        Err(e) => {
                            warn!(thread = %tid, "grouping AI failed: {e}");
                            None
                        }
                    }
                }
            },
        };
        // An unreachable AI leaves the pair waiting for a later round.
        let Some(same) = verdict else {
            continue;
        };
        progressed = true;
        match st.apply_ai_verdict(tid, &earlier, same) {
            Ok(Some(_)) => joined.push(tid.clone()),
            Ok(None) => {}
            Err(e) => warn!(thread = %tid, "grouping: {e}"),
        }
    }
    (joined, progressed)
}
