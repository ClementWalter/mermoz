//! "Ask Mermoz to do" on the daemon side: the account's AI turns a sentence into a
//! typed plan (never acting itself), the plan is resolved locally into a preview, and a
//! confirmed run applies it in batches through the same store calls as the RPCs, with
//! every thread's state logged first so one Undo reverts the whole run.

use crate::service::Service;
use mermoz_core::model::{ThreadState, new_id, now_ms};
use mermoz_core::plan::{self, Action, Outcome, Plan, Vocab};
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

/// Asks the AI for a plan (once more, with the reason, when its first answer is
/// refused) and previews it.
pub async fn ask(svc: &Arc<Service>, sentence: &str, origin: &str) -> Result<Value> {
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
    let ai = mermoz_core::agent::Client::new();
    let mut user = sentence.to_string();
    let mut refusal = String::new();
    for _ in 0..2 {
        let raw = ai
            .complete(&pr, &key, &system, &user, &plan::schema())
            .await?;
        let checked = serde_json::from_str::<Value>(&raw)
            .map_err(|_| "The answer wasn't a plan.".to_string())
            .and_then(|v| plan::validate(&v, &vocab, today));
        match checked {
            Ok(Outcome::Plan(p)) => return preview(svc, sentence, p, origin),
            Ok(Outcome::Question(q)) => return Ok(json!({ "kind": "question", "message": q })),
            Err(why) => {
                tracing::info!("plan refused: {why}");
                user = format!(
                    "{sentence}\n\nYour previous plan was refused: {why} Answer again with the schema, \
                     or set understood to false if the instruction can't be done with it."
                );
                refusal = why;
            }
        }
    }
    Ok(
        json!({ "kind": "question", "message": format!("I couldn't make a safe plan from that. {refusal}") }),
    )
}

/// Checks a plan given as JSON (CLI, agents, tests) and previews it, without the AI.
pub fn preview_json(svc: &Arc<Service>, sentence: &str, v: &Value, origin: &str) -> Result<Value> {
    let vocab = vocab(svc)?;
    match plan::validate(v, &vocab, plan::today()) {
        Ok(Outcome::Plan(p)) => preview(svc, sentence, p, origin),
        Ok(Outcome::Question(q)) => Ok(json!({ "kind": "question", "message": q })),
        Err(why) => Err(Error::InvalidParams(why)),
    }
}

/// Resolves every step locally, logs the run as planned and describes it in plain words.
fn preview(svc: &Arc<Service>, sentence: &str, p: Plan, origin: &str) -> Result<Value> {
    let st = &svc.store;
    let vocab = vocab(svc)?;
    let now = now_ms();
    let mut steps = vec![];
    let mut targets: Vec<Vec<String>> = vec![];
    let mut all: BTreeSet<String> = BTreeSet::new();
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
        steps.push(json!({
            "summary": plan::sentence(s, matches.len(), &vocab),
            "interpretation": plan::interpretation(&s.filter, &vocab),
            "action": plan::verb(&s.action, &vocab),
            "count": matches.len(),
            "by_status": counts.iter().map(|(k, n)| json!({ "status": k, "name": vocab.status_name(k), "count": n })).collect::<Vec<_>>(),
            "sample": matches.iter().take(SAMPLE).collect::<Vec<&PlanMatch>>(),
        }));
        targets.push(matches.into_iter().map(|m| m.id).collect());
    }
    let total = all.len();
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
        "total": total, "deletes": p.deletes(), "warning": warning,
        // A delete is confirmed by typing the count; everything else by Enter.
        "confirm": if p.deletes() { "type_count" } else { "enter" },
    }))
}

/// The thread states a batch is about to change: the targets, plus the open threads
/// of their conversations when the action follows the conversation.
fn snapshot(svc: &Service, ids: &[String], follows: bool) -> Result<Vec<ThreadState>> {
    let st = &svc.store;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = vec![];
    for id in ids {
        let mut group = vec![id.clone()];
        if follows {
            group.extend(st.open_siblings(id).unwrap_or_default());
        }
        for t in group {
            if seen.insert(t.clone())
                && let Ok(ts) = st.thread(&t)
            {
                out.push(ThreadState::from(&ts));
            }
        }
    }
    Ok(out)
}

/// Applies one action to a batch through the store, as the matching RPCs do.
fn apply(svc: &Service, action: &Action, ids: &[String]) -> Result<()> {
    let st = &svc.store;
    let patch = |p: ThreadPatch| st.update_threads(ids, &p).map(|_| ());
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

/// Tells the app and Gmail sync that these threads changed.
fn changed(svc: &Service, ids: Vec<String>) {
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
pub async fn run(svc: &Arc<Service>, id: &str, confirm: Option<i64>) -> Result<Value> {
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
    if p.deletes() && confirm != Some(op.total) {
        return Err(Error::InvalidParams(format!(
            "This moves {} threads to the Trash. Type {} to confirm.",
            plan::count(op.total as usize),
            op.total
        )));
    }
    st.set_bulk_op_state(id, "running", 0, None)?;
    let mut done: BTreeSet<String> = BTreeSet::new();
    let total = op.total;
    let progress = |n: usize| {
        let _ = st.set_bulk_op_state(id, "running", n as i64, None);
        let _ = svc.events.send(Event::DoProgress {
            id: id.to_string(),
            done: n as i64,
            total,
        });
    };
    for (i, step) in p.steps.iter().enumerate() {
        let targets = st.bulk_op_targets(id, i)?;
        let follows = !matches!(
            step.action,
            Action::MarkRead | Action::MarkUnread | Action::Later { .. }
        );
        for chunk in targets.chunks(BATCH) {
            // Threads gone since the preview (deleted elsewhere) are skipped.
            let live: Vec<String> = chunk
                .iter()
                .filter(|t| st.thread(t).is_ok())
                .cloned()
                .collect();
            let outcome = snapshot(svc, &live, follows)
                .and_then(|before| st.save_bulk_before(id, &before))
                .and_then(|_| apply(svc, &step.action, &live));
            if let Err(e) = outcome {
                st.set_bulk_op_state(id, "failed", done.len() as i64, Some(&e.to_string()))?;
                return Err(e);
            }
            done.extend(live.iter().cloned());
            changed(svc, live);
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
