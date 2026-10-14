//! The demo account: a made-up mailbox for screenshots and demos, added from Settings
//! without signing in to Google. It is fully local: it never syncs, its mail never goes
//! to an AI, and what is sent from it only lands in its own Sent.
//!
//! The persona (Maya Laurent, co-founder of a small design studio in Lyon), her
//! correspondents, companies and brands are all fictional; every address is on the
//! reserved `.example` domain. The mail is generated from the scripts in
//! [`conversations`] and [`automated`], with dates counted back from the moment it is
//! added, and comes with what the AI would have said (Feed cards, ✦ chips, summaries)
//! so the demo looks alive offline. Mail older than [`FRESH_FOR_DAYS`] is regenerated
//! on the next start, so the mailbox always looks current.

mod art;
mod automated;
mod conversations;

use crate::error::{Error, Result};
use crate::feed::texts::Choice;
use crate::feed::{Judgment, Kind, Texts};
use crate::mime::{ParsedAttachment, ParsedMessage};
use crate::model::{Account, Address, Draft, Suggestion, new_id, now_ms};
use crate::store::{IncomingMessage, Store, SyncMode, ThreadPatch};
use std::collections::BTreeMap;

/// The `provider` of the demo account.
pub const PROVIDER: &str = "demo";
/// The persona's address.
pub const EMAIL: &str = "maya@fernhill.example";
pub const NAME: &str = "Maya Laurent";

/// Days after which the demo mail is generated again, counted from the current day.
pub const FRESH_FOR_DAYS: i64 = 3;

const DAY: i64 = 86_400_000;

/// Settings key holding when an account's demo mail was generated.
fn seeded_key(account_id: &str) -> String {
    format!("demo_seeded:{account_id}")
}

/// Whether an account is the local demo account.
pub fn is_demo(account: &Account) -> bool {
    account.provider == PROVIDER
}

/// Adds the demo account with its mail, or returns it when it is already there.
pub fn add(store: &Store) -> Result<Account> {
    if let Some(a) = store.accounts()?.into_iter().find(is_demo) {
        return Ok(a);
    }
    let acc = store.add_account(EMAIL, PROVIDER)?;
    seed(store, &acc.id, now_ms())?;
    store.account(&acc.id)
}

/// Generates the demo mail again when it was made more than [`FRESH_FOR_DAYS`] ago,
/// keeping the account's place and whether it is hidden. Returns whether it did.
pub fn refresh_if_stale(store: &Store, now: i64) -> Result<bool> {
    let Some(acc) = store.accounts()?.into_iter().find(is_demo) else {
        return Ok(false);
    };
    let seeded: i64 = store
        .setting(&seeded_key(&acc.id))?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if now - seeded < FRESH_FOR_DAYS * DAY {
        return Ok(false);
    }
    store.clear_account_mail(&acc.id)?;
    seed(store, &acc.id, now)?;
    Ok(true)
}

/// A person in the demo mailbox.
pub(crate) struct Person {
    pub key: &'static str,
    pub name: &'static str,
    pub email: &'static str,
}

impl Person {
    fn address(&self) -> Address {
        Address {
            name: self.name.into(),
            email: self.email.into(),
        }
    }
}

pub(crate) fn person(key: &str) -> &'static Person {
    conversations::PEOPLE
        .iter()
        .find(|p| p.key == key)
        .unwrap_or_else(|| panic!("demo person {key}"))
}

fn me() -> Address {
    Address {
        name: NAME.into(),
        email: EMAIL.into(),
    }
}

/// When a message was sent, counted back from when the demo is generated.
#[derive(Clone, Copy, Debug)]
pub(crate) enum When {
    /// Minutes ago.
    Mins(i64),
    /// Days ago, at this local hour and minute.
    Day(i64, u32, u32),
}

impl When {
    fn at(self, now: i64) -> i64 {
        match self {
            When::Mins(m) => now - m * 60_000,
            When::Day(d, h, m) => {
                let local = chrono::DateTime::from_timestamp_millis(now)
                    .unwrap_or_default()
                    .with_timezone(&chrono::Local);
                let day = local.date_naive() - chrono::Duration::days(d);
                day.and_hms_opt(h, m, 0)
                    .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(now - d * DAY)
                    .min(now - 60_000)
            }
        }
    }
}

/// One generated message.
pub(crate) struct Mail {
    /// Its thread's key, unique in the demo.
    pub thread: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
    pub at: i64,
    pub files: Vec<ParsedAttachment>,
    pub list_id: Option<String>,
    pub unread: bool,
    pub inbox: bool,
    pub starred: bool,
    /// Folders (labels) it is filed in.
    pub folders: Vec<&'static str>,
    pub bounce: Option<crate::dsn::Dsn>,
}

impl Mail {
    pub(crate) fn new(thread: &str, from: Address, subject: &str, text: String, at: i64) -> Self {
        Mail {
            thread: thread.into(),
            from,
            to: vec![me()],
            cc: vec![],
            subject: subject.into(),
            text,
            html: None,
            at,
            files: vec![],
            list_id: None,
            unread: false,
            inbox: true,
            starred: false,
            folders: vec![],
            bounce: None,
        }
    }
}

/// What the AI would have said about a thread, and where the user put it.
#[derive(Default)]
pub(crate) struct Plan {
    pub thread: String,
    pub status: Option<&'static str>,
    pub priority: Option<i64>,
    /// Put off with Later for this many hours.
    pub later_hours: Option<i64>,
    pub judgment: Option<Judgment>,
    pub card: Option<Texts>,
    pub chip: Option<Suggestion>,
    /// The reading pane's summary and next step.
    pub gist: Option<(String, String)>,
    /// The user archived it from the Feed before: three of a sender's make the Feed
    /// gather that sender's mail in its batch cards.
    pub archived_from_feed: bool,
}

/// A judgment for automated mail (a newsletter, a receipt) the Feed gathers in a batch.
pub(crate) fn automated_judgment(kind: Kind) -> Judgment {
    Judgment {
        kind: Some(kind),
        needs_reply: Some(0.02),
        urgency: Some(0.1),
        automated: Some(0.97),
        ..Default::default()
    }
}

/// A ✦ chip the AI is sure about.
pub(crate) fn chip(status: Option<&str>, folders: &[&str]) -> Suggestion {
    Suggestion {
        status: status.map(String::from),
        status_confidence: status.map(|_| 0.94),
        projects: folders.iter().map(|f| f.to_string()).collect(),
        project_confidence: (!folders.is_empty()).then_some(0.92),
        priority: None,
        automated: None,
    }
}

pub(crate) fn choice(
    label: &str,
    detail: &str,
    recommended: bool,
    reason: &str,
    reply: &str,
) -> Choice {
    Choice {
        label: label.into(),
        detail: detail.into(),
        recommended,
        reason: reason.into(),
        reply: reply.into(),
    }
}

/// Every message and plan of the demo, for a mailbox generated at `now`.
pub(crate) fn mailbox(now: i64) -> (Vec<Mail>, Vec<Plan>) {
    let mut mails = vec![];
    let mut plans = vec![];
    conversations::build(now, &mut mails, &mut plans);
    automated::build(now, &mut mails, &mut plans);
    (mails, plans)
}

/// Fills the demo account with its mail and the AI's words about it.
pub fn seed(store: &Store, account_id: &str, now: i64) -> Result<()> {
    store.set_sync(account_id, "idle", Some(0), Some(0), None)?;
    let (mails, plans) = mailbox(now);
    let mut folders: BTreeMap<&str, String> = BTreeMap::new();
    for m in &mails {
        for f in &m.folders {
            let n = folders.len();
            folders
                .entry(f)
                .or_insert_with(|| format!("Label_demo_{n}"));
        }
    }
    let labels: Vec<(String, String)> = folders
        .iter()
        .map(|(name, id)| (id.clone(), name.to_string()))
        .collect();
    store.set_labels(account_id, &labels)?;
    let mut incoming = Vec::with_capacity(mails.len());
    for (i, m) in mails.iter().enumerate() {
        let from_me = m.from.email == EMAIL;
        let mut label_ids: Vec<String> = vec![];
        if m.inbox {
            label_ids.push("INBOX".into());
        }
        if m.unread && !from_me {
            label_ids.push("UNREAD".into());
        }
        if from_me {
            label_ids.push("SENT".into());
        }
        if m.starred {
            label_ids.push("STARRED".into());
        }
        label_ids.extend(m.folders.iter().map(|f| folders[f].clone()));
        let auto_kind = if m.bounce.is_some() { "bounce" } else { "" };
        incoming.push(IncomingMessage {
            provider_id: format!("demo-{i:04}"),
            provider_thread_id: format!("demo-{}", m.thread),
            label_ids,
            internal_date: m.at,
            parsed: ParsedMessage {
                from: Some(m.from.clone()),
                to: m.to.clone(),
                cc: m.cc.clone(),
                subject: m.subject.clone(),
                date: Some(m.at),
                body_text: m.text.clone(),
                body_html: m.html.clone(),
                message_id: Some(format!("<demo-{i:04}@fernhill.example>")),
                attachments: m.files.clone(),
                list_id: m.list_id.clone(),
                auto_kind: auto_kind.into(),
                dsn: m.bounce.clone(),
                ..Default::default()
            },
        });
    }
    let ids = store.ingest_many(account_id, EMAIL, &incoming, SyncMode::Initial)?;
    let mut thread_of: BTreeMap<&str, String> = BTreeMap::new();
    for (m, id) in mails.iter().zip(ids) {
        thread_of.insert(&m.thread, id);
    }
    for t in store.threads_to_group("pending", 10_000, false)? {
        store.classify_for_grouping(&t)?;
    }
    let today = chrono::DateTime::from_timestamp_millis(now)
        .unwrap_or_default()
        .with_timezone(&chrono::Local)
        .date_naive()
        .to_string();
    let mut taught = vec![];
    for p in plans {
        let tid = thread_of
            .get(p.thread.as_str())
            .ok_or_else(|| Error::NotFound(format!("demo thread {}", p.thread)))?
            .clone();
        if p.status.is_some() || p.priority.is_some() {
            store.update_threads(
                std::slice::from_ref(&tid),
                &ThreadPatch {
                    status: p.status.map(String::from),
                    priority: p.priority,
                    ..Default::default()
                },
            )?;
        }
        let last = store.thread(&tid)?.last_date;
        if let Some(j) = &p.judgment {
            store.save_judgment(&tid, j, last, true, &today)?;
        }
        if let Some(t) = p.card {
            store.save_feed_texts(&tid, last, PROVIDER, &Ok(t))?;
        }
        if let Some(sg) = &p.chip {
            store.save_suggestion(&tid, sg, PROVIDER, last)?;
        }
        if let Some((summary, next)) = &p.gist {
            store.set_thread_gist(&tid, summary, next)?;
        }
        if let Some(h) = p.later_hours {
            store.snooze(std::slice::from_ref(&tid), now + h * 3_600_000, None)?;
        }
        if p.archived_from_feed {
            taught.push(tid);
        }
    }
    store.learn_archive(&taught)?;
    store.set_setting(&seeded_key(account_id), &now.to_string())?;
    tracing::info!(threads = thread_of.len(), "demo mailbox generated");
    Ok(())
}

/// Sends a draft from the demo account: nothing leaves the machine, the message lands in
/// the account's Sent (and its conversation) as if delivered.
pub fn deliver(
    store: &Store,
    account: &Account,
    draft: &Draft,
    after_status: Option<&str>,
) -> Result<String> {
    let (raw, provider_thread) = crate::sync::draft_message(store, &account.email, draft)?;
    let parsed = crate::mime::parse(&raw)
        .ok_or_else(|| Error::other("the message could not be read back"))?;
    let now = now_ms();
    let msg = IncomingMessage {
        provider_id: new_id("demo-sent"),
        provider_thread_id: provider_thread.unwrap_or_else(|| new_id("demo-thread")),
        label_ids: vec!["SENT".into()],
        internal_date: now,
        parsed,
    };
    let tid = store.ingest(&account.id, &account.email, &msg, SyncMode::Incremental)?;
    store.set_draft_state(&draft.id, "sent")?;
    if let Err(e) = store.purge_draft_files(&draft.id) {
        tracing::debug!("could not remove sent draft files: {e}");
    }
    // Like a real send, the conversation then waits on the other side.
    let status = after_status.unwrap_or("waiting").to_string();
    store.update_threads(
        std::slice::from_ref(&tid),
        &ThreadPatch {
            status: Some(status),
            ..Default::default()
        },
    )?;
    Ok(tid)
}

#[cfg(test)]
mod tests;
