//! "Ask Mermoz to do" on the daemon side: the account's AI turns a sentence into a
//! typed plan (never acting itself), the plan is resolved locally into a preview, and a
//! confirmed run applies it in batches through the same store calls as the RPCs, with
//! every thread's state logged first so one Undo reverts the whole run.

use crate::service::Service;
use mermoz_core::model::{ThreadState, new_id, now_ms};
use mermoz_core::plan::{
    self, Action, CandidateSender, CandidateSubject, Candidates, Outcome, Pick, Plan, Step, Vocab,
};
use mermoz_core::store::{BulkOp, PlanMatch, ThreadPatch};
use mermoz_core::sync::Event;
use mermoz_core::{Error, Result};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Threads changed per write: short enough that a click never waits long behind one.
const BATCH: usize = 200;
/// Subjects shown under each step of a preview.
const SAMPLE: usize = 5;

pub const NO_AI: &str = "To let Mermoz do this for you, connect an AI in Settings › Agents and allow it for an account.";

/// The user's statuses, projects and accounts, which a plan may name.
pub fn vocab(svc: &Service) -> Result<Vocab> {
    let st = &svc.store;
    Ok(Vocab {
        statuses: st
            .statuses()?
            .into_iter()
            .map(|s| (s.key, s.name))
            .collect(),
        projects: st.projects()?.into_iter().map(|p| p.name).collect(),
        accounts: st
            .accounts()?
            .into_iter()
            .map(|a| (a.id, a.email))
            .collect(),
    })
}

/// The first account's allowed AI. Only the sentence and the names of statuses,
/// projects and accounts go to it, never mail.
fn provider(svc: &Service) -> Result<Option<mermoz_core::agent::Provider>> {
    for a in svc.store.accounts()? {
        if let Some(p) = svc.store.provider_for_account(&a.id)? {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// Whether a sentence can be planned now, and what to say when it can't.
pub fn status(svc: &Service) -> Result<Value> {
    Ok(match provider(svc)? {
        Some(p) => json!({ "available": true, "provider_name": p.name }),
        None => json!({ "available": false, "message": NO_AI }),
    })
}

/// One earlier round of a clarification: the AI's question and the user's answer.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Round {
    pub question: String,
    pub answer: String,
}

/// Rounds of questions the user answers before the AI must plan or give up.
pub const MAX_ROUNDS: usize = 2;

/// A candidate as the palette offers it: a label and the exact filter it picks.
fn choice_sender(s: &CandidateSender) -> Value {
    let mut label = if s.name.is_empty() {
        format!("{} · {} threads", s.email, plan::count(s.threads as usize))
    } else {
        format!(
            "{} · {} · {} threads",
            s.name,
            s.email,
            plan::count(s.threads as usize)
        )
    };
    if let Some(p) = s.subjects.first() {
        label.push_str(&format!(" · “{p}…”"));
    }
    json!({ "label": label, "pick": { "from": s.email }, "threads": s.threads })
}

fn choice_subject(s: &CandidateSubject) -> Value {
    json!({ "label": format!("Subject “{}…” · {} threads", s.prefix, plan::count(s.threads as usize)),
            "pick": { "subject_prefix": s.prefix }, "threads": s.threads })
}

fn choices(c: &Candidates) -> Vec<Value> {
    c.senders
        .iter()
        .map(choice_sender)
        .chain(c.subjects.iter().map(choice_subject))
        .collect()
}

fn question(message: &str, c: &Candidates, round: usize) -> Value {
    json!({ "kind": "question", "message": message, "choices": choices(c),
            "round": round, "can_answer": round <= MAX_ROUNDS })
}

/// Asks the AI for a plan grounded in what the mailbox holds for the sentence's words
/// (once more, with the reason, when its answer is refused), then previews it. `rounds`
/// are the questions already answered; `pick` is a candidate the user chose.
pub async fn ask(
    svc: &Arc<Service>,
    sentence: &str,
    rounds: &[Round],
    pick: Option<&Pick>,
    origin: &str,
) -> Result<Value> {
    let sentence = sentence.trim();
    if sentence.is_empty() {
        return Err(Error::InvalidParams("Say what to do first.".into()));
    }
    let Some(pr) = provider(svc)? else {
        return Ok(json!({ "kind": "needs_ai", "message": NO_AI }));
    };
    let key = crate::service::credential(&pr).await?;
    let vocab = vocab(svc)?;
    let today = plan::today();
    let system = plan::system_prompt(&vocab, today);
    let candidates = {
        let (st, s) = (svc.store.clone(), sentence.to_string());
        tokio::task::spawn_blocking(move || st.candidates(&s))
            .await
            .map_err(|e| Error::other(e.to_string()))??
    };
    let pairs: Vec<(String, String)> = rounds
        .iter()
        .map(|r| (r.question.clone(), r.answer.clone()))
        .collect();
    let mut base = plan::user_prompt(sentence, &pairs, &candidates, pick);
    if rounds.len() >= MAX_ROUNDS {
        base.push_str(
            "\n\nThe user has answered enough questions: return a plan, or set understood to false \
             with one short sentence saying why it can't be done.",
        );
    }
    let ai = mermoz_core::agent::Client::new();
    let mut user = base.clone();
    let mut refusal = String::new();
    for _ in 0..2 {
        let raw = ai
            .complete(&pr, &key, &system, &user, &plan::schema())
            .await?;
        let checked = serde_json::from_str::<Value>(&raw)
            .map_err(|_| "The answer wasn't a plan.".to_string())
            .and_then(|v| plan::validate(&v, &vocab, today));
        match checked {
            Ok(Outcome::Plan(mut p)) => {
                if let Some(pick) = pick {
                    pick.apply(&mut p);
                }
                return preview(svc, sentence, p, origin);
            }
            Ok(Outcome::Question(q)) => {
                // A picked filter with a plain verb needs no more questions.
                if let (Some(pick), Some(action)) = (pick, plan::action_from_sentence(sentence)) {
                    let mut p = Plan {
                        steps: vec![Step {
                            filter: Default::default(),
                            action,
                        }],
                    };
                    pick.apply(&mut p);
                    return preview(svc, sentence, p, origin);
                }
                return Ok(question(&q, &candidates, rounds.len() + 1));
            }
            Err(why) => {
                tracing::info!("plan refused: {why}");
                user = format!(
                    "{base}\n\nYour previous plan was refused: {why} Answer again with the schema, \
                     or set understood to false if the instruction can't be done with it."
                );
                refusal = why;
            }
        }
    }
    Ok(question(
        &format!("I couldn't make a safe plan from that. {refusal}"),
        &candidates,
        rounds.len() + 1,
    ))
}

/// Checks a plan given as JSON (CLI, agents, tests) and previews it, without the AI.
pub fn preview_json(svc: &Arc<Service>, sentence: &str, v: &Value, origin: &str) -> Result<Value> {
    let vocab = vocab(svc)?;
    match plan::validate(v, &vocab, plan::today()) {
        Ok(Outcome::Plan(p)) => preview(svc, sentence, p, origin),
        Ok(Outcome::Question(q)) => Ok(json!({ "kind": "question", "message": q, "choices": [] })),
        Err(why) => Err(Error::InvalidParams(why)),
    }
}

/// A planned preview again with the user's picked filter in every step (no AI).
pub fn replan(svc: &Arc<Service>, id: &str, pick: &Pick, origin: &str) -> Result<Value> {
    let op = svc.store.bulk_op(id)?;
    if op.state != "planned" {
        return Err(Error::InvalidParams(
            "This plan already ran. Ask again for a new one.".into(),
        ));
    }
    let mut p: Plan = serde_json::from_value(op.plan.clone())
        .map_err(|e| Error::other(format!("stored plan unreadable: {e}")))?;
    pick.apply(&mut p);
    preview(svc, &op.sentence, p, origin)
}

/// "from a@x (34), b@y (2) and 3 more": who the sender words resolved to.
fn resolved_words(senders: &[CandidateSender]) -> Option<String> {
    let shown: Vec<String> = senders
        .iter()
        .take(3)
        .map(|s| format!("{} ({})", s.email, plan::count(s.threads as usize)))
        .collect();
    if shown.is_empty() {
        return None;
    }
    let more = senders.len().saturating_sub(3);
    let tail = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    Some(format!("from {}{tail}", shown.join(", ")))
}

/// Resolves every step locally, logs the run as planned and describes it in plain words.
/// A step that matches nothing offers the closest senders, and the sentence's candidates.
fn preview(svc: &Arc<Service>, sentence: &str, p: Plan, origin: &str) -> Result<Value> {
    let st = &svc.store;
    let vocab = vocab(svc)?;
    let now = now_ms();
    let mut steps = vec![];
    let mut targets: Vec<Vec<String>> = vec![];
    let mut all: BTreeSet<String> = BTreeSet::new();
    let mut offered: Vec<Value> = vec![];
    for s in &p.steps {
        let matches = st.plan_matches(&s.filter, now)?;
        let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
        for m in &matches {
            *by_status.entry(m.status.clone()).or_default() += 1;
        }
        let order: Vec<String> = vocab.statuses.iter().map(|(k, _)| k.clone()).collect();
        let mut counts: Vec<(String, usize)> = by_status.into_iter().collect();
        counts.sort_by_key(|(k, _)| order.iter().position(|o| o == k).unwrap_or(usize::MAX));
        all.extend(matches.iter().map(|m| m.id.clone()));
        let senders = if s.filter.from.is_empty() {
            vec![]
        } else {
            st.resolve_senders(&s.filter.from)?
        };
        let suggestions: Vec<Value> = if matches.is_empty() && !s.filter.from.is_empty() {
            st.sender_suggestions(&s.filter.from, 3)?
                .iter()
                .map(choice_sender)
                .collect()
        } else {
            vec![]
        };
        offered.extend(suggestions.iter().cloned());
        // One sender matched: the sentence names it as the user knows it.
        let mut shown = s.clone();
        if let [one] = senders.as_slice() {
            shown.filter.from = vec![one.display()];
        }
        steps.push(json!({
            "summary": plan::sentence(&shown, matches.len(), &vocab),
            "interpretation": plan::interpretation(&s.filter, &vocab),
            "resolved": resolved_words(&senders),
            "senders": senders.iter().take(5).collect::<Vec<_>>(),
            "suggestions": suggestions,
            "action": plan::verb(&s.action, &vocab),
            "count": matches.len(),
            "by_status": counts.iter().map(|(k, n)| json!({ "status": k, "name": vocab.status_name(k), "count": n })).collect::<Vec<_>>(),
            "sample": matches.iter().take(SAMPLE).collect::<Vec<&PlanMatch>>(),
        }));
        targets.push(matches.into_iter().map(|m| m.id).collect());
    }
    let total = all.len();
    if total == 0 {
        // Never a dead end: what the mailbox holds for the sentence's words too.
        for c in choices(&st.candidates(sentence)?) {
            if offered.len() < 5 && !offered.iter().any(|o| o["pick"] == c["pick"]) {
                offered.push(c);
            }
        }
    }
    let summary = steps
        .iter()
        .filter_map(|s| s["summary"].as_str())
        .collect::<Vec<_>>()
        .join("; then ");
    let interpretation = p
        .steps
        .iter()
        .map(|s| plan::interpretation(&s.filter, &vocab))
        .collect::<Vec<_>>()
        .join("; then ");
    let warning = if total == 0 {
        Some(format!("Nothing matches. I understood: {interpretation}."))
    } else if p.steps.iter().any(|s| s.filter.is_empty()) {
        Some("This applies to every email in Mermoz.".to_string())
    } else {
        None
    };
    let id = new_id("bop");
    let plan_json = serde_json::to_value(&p)?;
    st.save_bulk_op(
        &BulkOp {
            id: id.clone(),
            sentence: sentence.to_string(),
            plan: plan_json.clone(),
            summary: summary.clone(),
            state: "planned".into(),
            total: total as i64,
            done: 0,
            origin: origin.to_string(),
            error: None,
            created: now,
            finished: None,
            undone: None,
        },
        &targets,
    )?;
    Ok(json!({
        "kind": "plan", "id": id, "sentence": sentence, "plan": plan_json,
        "summary": summary, "interpretation": interpretation, "steps": steps,
        "total": total, "deletes": p.deletes(), "warning": warning, "choices": offered,
        // A delete is confirmed by typing the count; everything else by Enter.
        "confirm": if p.deletes() { "type_count" } else { "enter" },
    }))
}

/// A step's targets, each with the open threads of its conversation when the action
/// follows the conversation, as they are when the run starts.
fn with_siblings(svc: &Service, targets: &[String], follows: bool) -> Vec<(String, Vec<String>)> {
    targets
        .iter()
        .map(|t| {
            let sib = if follows {
                svc.store.open_siblings(t).unwrap_or_default()
            } else {
                vec![]
            };
            (t.clone(), sib)
        })
        .collect()
}

/// Splits a step into writes of `size` targets; each write takes its targets and their
/// siblings, every thread once per step.
fn writes(step: &[(String, Vec<String>)], size: usize) -> Vec<(Vec<String>, Vec<String>)> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    step.chunks(size)
        .map(|chunk| {
            let mut ids = vec![];
            for (t, sib) in chunk {
                for id in std::iter::once(t).chain(sib) {
                    if seen.insert(id) {
                        ids.push(id.clone());
                    }
                }
            }
            (chunk.iter().map(|(t, _)| t.clone()).collect(), ids)
        })
        .collect()
}

/// Applies one action to a batch through the store, as the matching RPCs do.
fn apply(svc: &Service, action: &Action, ids: &[String]) -> Result<()> {
    let st = &svc.store;
    // Exactly the run's snapshot: a thread that joined a conversation since is not
    // pulled along.
    let patch = |p: ThreadPatch| st.update_threads_exactly(ids, &p).map(|_| ());
    match action {
        Action::SetStatus { status } => patch(ThreadPatch {
            status: Some(status.clone()),
            ..Default::default()
        }),
        Action::Delete => patch(ThreadPatch {
            status: Some("deleted".into()),
            ..Default::default()
        }),
        Action::AddProject { project } => patch(ThreadPatch {
            add_projects: vec![project.clone()],
            ..Default::default()
        }),
        Action::RemoveProject { project } => patch(ThreadPatch {
            remove_projects: vec![project.clone()],
            ..Default::default()
        }),
        Action::MarkRead | Action::MarkUnread => {
            let read = *action == Action::MarkRead;
            for id in ids {
                if st.thread(id)?.unread == read {
                    st.mark_read(id, read)?;
                }
            }
            Ok(())
        }
        Action::Later { until } => {
            let at = plan::later_ms(until)
                .ok_or_else(|| Error::InvalidParams(format!("'{until}' is not a date")))?;
            st.snooze(ids, at, Some(&plan::day_words(until)))?;
            let _ = svc.events.send(Event::FeedChanged);
            svc.later_kick.notify_one();
            Ok(())
        }
    }
}

/// Tells the app and Gmail sync that these threads changed. Their write-backs wait
/// behind the user's own edits.
fn changed(svc: &Service, ids: Vec<String>) {
    if let Err(e) = svc.store.mark_ops_bulk(&ids) {
        tracing::warn!("bulk write-backs keep their place in the queue: {e}");
    }
    let mut accounts: BTreeSet<String> = BTreeSet::new();
    for id in &ids {
        if let Ok((acc, _)) = svc.store.provider_thread_id(id) {
            accounts.insert(acc);
        }
    }
    let _ = svc.events.send(Event::ThreadsChanged(ids));
    for acc in accounts {
        svc.kick(&acc);
    }
}

/// Runs a planned operation in batches. A delete needs `confirm` equal to the count
/// shown in its preview.
pub async fn run(
    svc: &Arc<Service>,
    id: &str,
    confirm: Option<i64>,
    exclude: &BTreeSet<String>,
) -> Result<Value> {
    let st = &svc.store;
    let op = st.bulk_op(id)?;
    if op.state != "planned" {
        return Err(Error::InvalidParams(format!(
            "This plan already ran ({}). Ask again for a new one.",
            op.state
        )));
    }
    let p: Plan = serde_json::from_value(op.plan.clone())
        .map_err(|e| Error::other(format!("stored plan unreadable: {e}")))?;
    // The user unticked some threads in the preview: the run changes exactly the rest.
    let ticked = st
        .bulk_op_threads(id)?
        .iter()
        .filter(|t| !exclude.contains(&t.id))
        .count() as i64;
    if ticked == 0 {
        return Err(Error::InvalidParams("No thread is ticked.".into()));
    }
    if p.deletes() && confirm != Some(ticked) {
        return Err(Error::InvalidParams(format!(
            "This moves {} threads to the Trash. Type {} to confirm.",
            plan::count(ticked as usize),
            ticked
        )));
    }
    st.set_bulk_op_state(id, "running", 0, None)?;
    // One snapshot before any change: the threads each step acts on and their state.
    // The run changes exactly these, so its undo restores every one of them.
    let mut steps: Vec<Vec<(String, Vec<String>)>> = vec![];
    for (i, step) in p.steps.iter().enumerate() {
        // Unticked threads, and threads gone since the preview, are skipped.
        let live: Vec<String> = st
            .bulk_op_targets(id, i)?
            .into_iter()
            .filter(|t| !exclude.contains(t) && st.thread(t).is_ok())
            .collect();
        let follows = !matches!(
            step.action,
            Action::MarkRead | Action::MarkUnread | Action::Later { .. }
        );
        steps.push(with_siblings(svc, &live, follows));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut before: Vec<ThreadState> = vec![];
    for (t, sib) in steps.iter().flatten() {
        for tid in std::iter::once(t).chain(sib) {
            if seen.insert(tid)
                && let Ok(ts) = st.thread(tid)
            {
                before.push(ThreadState::from(&ts));
            }
        }
    }
    if let Err(e) = st.save_bulk_before(id, &before) {
        st.set_bulk_op_state(id, "failed", 0, Some(&e.to_string()))?;
        return Err(e);
    }
    let mut done: BTreeSet<String> = BTreeSet::new();
    let total = ticked;
    let progress = |n: usize| {
        let _ = st.set_bulk_op_state(id, "running", n as i64, None);
        let _ = svc.events.send(Event::DoProgress {
            id: id.to_string(),
            done: n as i64,
            total,
        });
    };
    for (step, threads) in p.steps.iter().zip(&steps) {
        for (targets, ids) in writes(threads, BATCH) {
            if let Err(e) = apply(svc, &step.action, &ids) {
                st.set_bulk_op_state(id, "failed", done.len() as i64, Some(&e.to_string()))?;
                return Err(e);
            }
            done.extend(targets);
            changed(svc, ids);
            progress(done.len());
            tokio::task::yield_now().await;
        }
    }
    st.set_bulk_op_state(id, "done", done.len() as i64, None)?;
    let _ = svc.events.send(Event::SuggestionsChanged);
    let vocab = vocab(svc)?;
    let label = match p.steps.as_slice() {
        [one] => plan::verb(&one.action, &vocab),
        _ => "Ask Mermoz".to_string(),
    };
    Ok(json!({ "id": id, "done": done.len(), "total": total, "undo_label": label }))
}

/// Reverts a whole run: every thread it changed goes back as it was (threads changed
/// again since keep their newer state only where the run had not touched them).
pub fn undo(svc: &Arc<Service>, id: &str) -> Result<Value> {
    let st = &svc.store;
    let op = st.bulk_op(id)?;
    match op.state.as_str() {
        "done" | "failed" => {}
        "undone" => return Err(Error::InvalidParams("This was already undone.".into())),
        _ => return Err(Error::InvalidParams("This hasn't run yet.".into())),
    }
    let p: Plan = serde_json::from_value(op.plan.clone()).unwrap_or(Plan { steps: vec![] });
    for (i, step) in p.steps.iter().enumerate() {
        if matches!(step.action, Action::Later { .. }) {
            st.unsnooze(&st.bulk_op_targets(id, i)?)?;
            let _ = svc.events.send(Event::FeedChanged);
        }
    }
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    let before: Vec<ThreadState> = st
        .bulk_before(id)?
        .into_iter()
        .filter(|b| match st.thread(&b.id) {
            Ok(now) => {
                let now = ThreadState::from(&now);
                now.unread != b.unread
                    || now.status != b.status
                    || now.priority != b.priority
                    || now.due != b.due
                    || sorted(now.projects) != sorted(b.projects.clone())
            }
            Err(_) => false,
        })
        .collect();
    let mut restored = 0;
    for chunk in before.chunks(BATCH) {
        restored += st.restore_threads(chunk)?;
        changed(svc, chunk.iter().map(|t| t.id.clone()).collect());
    }
    st.set_bulk_op_state(id, "undone", op.done, None)?;
    let _ = svc.events.send(Event::SuggestionsChanged);
    Ok(json!({ "id": id, "restored": restored }))
}

#[cfg(test)]
mod tests;
