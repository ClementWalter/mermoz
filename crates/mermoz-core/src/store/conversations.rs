//! Conversations in the store: which Gmail threads read as one chat, which thread
//! leads each conversation in lists, and the grouping queue the daemon works through.
//!
//! A thread's `conversation_id` is its own id while alone. The lead is the latest open
//! thread (else the latest one); a trigger keeps it current whenever a status, a date
//! or a membership changes, so every list shows one row per conversation.

use super::Store;
use crate::conversation::{self as conv, Kind, PairItem, PairMessage};
use crate::error::{Error, Result};
use crate::model::*;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use std::collections::{BTreeSet, HashMap};

pub(super) const MIGRATION: &str = r#"
ALTER TABLE threads ADD COLUMN conversation_id TEXT;
ALTER TABLE threads ADD COLUMN conv_lead INTEGER NOT NULL DEFAULT 1;
ALTER TABLE threads ADD COLUMN conv_kind TEXT NOT NULL DEFAULT 'person';
ALTER TABLE threads ADD COLUMN conv_key TEXT;
-- pending: stream not decided · judge: waiting for Jev · ai: waiting for the AI · done
ALTER TABLE threads ADD COLUMN conv_state TEXT NOT NULL DEFAULT 'pending';
UPDATE threads SET conversation_id = id;
CREATE INDEX IF NOT EXISTS threads_conversation ON threads(conversation_id, last_date DESC);
CREATE INDEX IF NOT EXISTS threads_conv_state ON threads(conv_state, last_date DESC);
CREATE INDEX IF NOT EXISTS threads_conv_key ON threads(conv_kind, conv_key);
-- Covering index for one-row-per-conversation counts (status, account, priority, kind).
CREATE INDEX IF NOT EXISTS threads_conv_lead ON threads(conv_lead, status, account_id, priority, conv_kind);
ALTER TABLE messages ADD COLUMN list_id TEXT;
CREATE TABLE IF NOT EXISTS correspondents (email TEXT PRIMARY KEY);
INSERT OR IGNORE INTO correspondents
  SELECT DISTINCT lower(json_extract(j.value, '$.email')) FROM messages m, json_each(m.to_json) j
  WHERE m.is_from_me = 1 AND json_extract(j.value, '$.email') IS NOT NULL;
INSERT OR IGNORE INTO correspondents
  SELECT DISTINCT lower(json_extract(j.value, '$.email')) FROM messages m, json_each(m.cc_json) j
  WHERE m.is_from_me = 1 AND json_extract(j.value, '$.email') IS NOT NULL;
-- Every judged pair: the learning record, and the AI's queue (decision 'ask_ai').
CREATE TABLE IF NOT EXISTS conversation_pairs (
  later TEXT NOT NULL, earlier TEXT NOT NULL, people TEXT NOT NULL DEFAULT '[]',
  jev REAL, ai INTEGER, decision TEXT NOT NULL, source TEXT NOT NULL, created INTEGER NOT NULL,
  PRIMARY KEY (later, earlier));
CREATE INDEX IF NOT EXISTS conversation_pairs_decision ON conversation_pairs(decision, jev DESC);
CREATE TRIGGER IF NOT EXISTS threads_conv_lead_insert AFTER INSERT ON threads BEGIN
  UPDATE threads SET conv_lead = (id = (SELECT x.id FROM threads x WHERE x.conversation_id = NEW.conversation_id
      ORDER BY x.status IN (SELECT key FROM statuses WHERE archived = 0) DESC, x.last_date DESC, x.id LIMIT 1))
    WHERE conversation_id = NEW.conversation_id;
END;
CREATE TRIGGER IF NOT EXISTS threads_conv_lead_update AFTER UPDATE OF status, last_date, conversation_id ON threads BEGIN
  UPDATE threads SET conv_lead = (id = (SELECT x.id FROM threads x WHERE x.conversation_id = NEW.conversation_id
      ORDER BY x.status IN (SELECT key FROM statuses WHERE archived = 0) DESC, x.last_date DESC, x.id LIMIT 1))
    WHERE conversation_id = NEW.conversation_id
      AND conv_lead != (id = (SELECT x.id FROM threads x WHERE x.conversation_id = NEW.conversation_id
      ORDER BY x.status IN (SELECT key FROM statuses WHERE archived = 0) DESC, x.last_date DESC, x.id LIMIT 1));
  UPDATE threads SET conv_lead = (id = (SELECT x.id FROM threads x WHERE x.conversation_id = OLD.conversation_id
      ORDER BY x.status IN (SELECT key FROM statuses WHERE archived = 0) DESC, x.last_date DESC, x.id LIMIT 1))
    WHERE OLD.conversation_id IS NOT NEW.conversation_id AND conversation_id = OLD.conversation_id;
END;
"#;

/// Remembers who the user writes to: only they count as people for grouping.
pub(super) fn note_correspondents<'a>(
    tx: &Connection,
    addrs: impl Iterator<Item = &'a Address>,
) -> Result<()> {
    let mut st =
        tx.prepare_cached("INSERT OR IGNORE INTO correspondents (email) VALUES (lower(?1))")?;
    for a in addrs {
        if !a.email.trim().is_empty() {
            st.execute([a.email.trim()])?;
        }
    }
    Ok(())
}

/// Gmail threads per conversation, for the rows given.
pub(super) fn sizes(conn: &Connection, rows: &[ThreadSummary]) -> Result<HashMap<String, i64>> {
    let ids: BTreeSet<&str> = rows.iter().map(|t| t.conversation_id.as_str()).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let marks = vec!["?"; ids.len()].join(",");
    let mut st = conn.prepare(&format!(
        "SELECT conversation_id, COUNT(*) FROM threads WHERE conversation_id IN ({marks}) GROUP BY conversation_id"
    ))?;
    let out = st
        .query_map(params_from_iter(ids.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

/// What the grouping worker does with one thread.
#[derive(Debug, Clone, PartialEq)]
pub enum GroupingPlan {
    /// A feed or list: joined to its stream in code (already done by the store).
    Grouped,
    /// Nothing to judge: it stays its own conversation.
    Alone,
    /// Ask Jev about each candidate; `join_at` reflects the user's past splits.
    Judge {
        people: Vec<String>,
        join_at: f64,
        candidates: Vec<PairCandidate>,
    },
}

/// An earlier conversation a new thread may continue, as Jev will see the pair.
#[derive(Debug, Clone, PartialEq)]
pub struct PairCandidate {
    /// The earlier thread the pair is judged against.
    pub earlier: String,
    pub conversation_id: String,
    pub state: serde_json::Value,
}

/// One thread's people and stream signals.
struct ThreadPeople {
    account_id: String,
    first_date: i64,
    last_date: i64,
    /// Correspondents other than the user (empty for announcements).
    people: BTreeSet<String>,
    first_sender: String,
    user_wrote: bool,
    list_id: Option<String>,
}

/// Every address the user writes from on this account.
fn my_addresses(c: &Connection, account_id: &str) -> Result<BTreeSet<String>> {
    let mut me: BTreeSet<String> = c
        .prepare_cached("SELECT lower(email) FROM accounts WHERE id = ?1")?
        .query_map([account_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    me.extend(
        c.prepare_cached(
            "SELECT DISTINCT lower(from_email) FROM messages WHERE account_id = ?1 AND is_from_me = 1 LIMIT 50",
        )?
        .query_map([account_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?,
    );
    Ok(me)
}

fn thread_people(c: &Connection, tid: &str, me: &BTreeSet<String>) -> Result<Option<ThreadPeople>> {
    let head: Option<(String, i64, i64)> = c
        .query_row(
            "SELECT account_id, first_date, last_date FROM threads WHERE id = ?1",
            [tid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((account_id, first_date, last_date)) = head else {
        return Ok(None);
    };
    let mut st = c.prepare_cached(
        "SELECT lower(from_email), is_from_me, to_json, cc_json, list_id FROM messages WHERE thread_id = ?1 ORDER BY date",
    )?;
    let rows: Vec<(String, bool, String, String, Option<String>)> = st
        .query_map([tid], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, i64>(1)? != 0,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut everyone: BTreeSet<String> = BTreeSet::new();
    let (mut first_sender, mut user_wrote, mut list_id) = (String::new(), false, None);
    for (from, mine, to, cc, list) in &rows {
        if list_id.is_none() {
            list_id = list.clone();
        }
        if *mine || me.contains(from) {
            user_wrote = true;
            let to: Vec<Address> = serde_json::from_str(to).unwrap_or_default();
            let cc: Vec<Address> = serde_json::from_str(cc).unwrap_or_default();
            everyone.extend(to.iter().chain(&cc).map(|a| a.email.to_lowercase()));
        } else {
            if first_sender.is_empty() {
                first_sender = from.clone();
            }
            everyone.insert(from.clone());
        }
    }
    everyone.retain(|e| !me.contains(e) && !e.is_empty());
    let mut people = BTreeSet::new();
    if everyone.len() <= conv::MAX_PEOPLE {
        let mut known = c.prepare_cached("SELECT 1 FROM correspondents WHERE email = ?1")?;
        for e in everyone {
            if !conv::is_bulk(&e) && known.exists([&e])? {
                people.insert(e);
            }
        }
    }
    Ok(Some(ThreadPeople {
        account_id,
        first_date,
        last_date,
        people,
        first_sender,
        user_wrote,
        list_id,
    }))
}

/// A thread as one side of a pair: subject, size and its last two messages.
fn pair_item(c: &Connection, tid: &str) -> Result<PairItem> {
    let mut st = c.prepare_cached(
        "SELECT m.id, from_name, from_email, is_from_me, date, subject, body_text FROM messages m
         WHERE thread_id = ?1 ORDER BY date",
    )?;
    let rows: Vec<(String, String, String, bool, i64, String, String)> = st
        .query_map([tid], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get::<_, i64>(3)? != 0,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut files =
        c.prepare_cached("SELECT filename FROM attachments WHERE message_id = ?1 LIMIT 5")?;
    let mut last = vec![];
    for (mid, name, email, mine, date, subject, body) in &rows[rows.len().saturating_sub(2)..] {
        last.push(PairMessage {
            from: if *mine {
                "the user".into()
            } else if name.is_empty() {
                email.clone()
            } else {
                format!("{name} <{email}>")
            },
            date: chrono::DateTime::from_timestamp_millis(*date)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            subject: conv::bare_subject(subject),
            text: conv::excerpt(body),
            attachments: files
                .query_map([mid], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?,
        });
    }
    Ok(PairItem {
        subject: last.first().map(|m| m.subject.clone()).unwrap_or_default(),
        messages_in_thread: rows.len() as i64,
        last_messages: last,
    })
}

fn conversation_of(c: &Connection, tid: &str) -> Result<String> {
    c.query_row(
        "SELECT COALESCE(conversation_id, id) FROM threads WHERE id = ?1",
        [tid],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| Error::NotFound(format!("thread {tid}")))
}

/// Moves every thread of `from` into `into`.
fn absorb(tx: &Connection, from: &str, into: &str) -> Result<()> {
    if from != into {
        tx.execute(
            "UPDATE threads SET conversation_id = ?2 WHERE conversation_id = ?1",
            params![from, into],
        )?;
    }
    Ok(())
}

fn record_pair(
    tx: &Connection,
    later: &str,
    earlier: &str,
    people: &[String],
    // Jev's probability and the AI's verdict, when they were asked.
    (jev, ai): (Option<f64>, Option<bool>),
    decision: &str,
    source: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO conversation_pairs (later, earlier, people, jev, ai, decision, source, created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(later, earlier) DO UPDATE SET people = excluded.people, jev = COALESCE(excluded.jev, jev),
           ai = COALESCE(excluded.ai, ai), decision = excluded.decision, source = excluded.source, created = excluded.created",
        params![
            later,
            earlier,
            serde_json::to_string(people)?,
            jev,
            ai.map(|b| b as i64),
            decision,
            source,
            now_ms()
        ],
    )?;
    Ok(())
}

impl Store {
    /// Threads waiting in one grouping stage, recent mail first (the rest follows).
    /// With `wait_for_router`, new Unread mail waits for its routing verdict, whose
    /// "automated" probability tells feeds from people.
    pub fn threads_to_group(
        &self,
        state: &str,
        limit: i64,
        wait_for_router: bool,
    ) -> Result<Vec<String>> {
        let r = self.bg.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT t.id FROM threads t WHERE t.conv_state = ?1 AND t.status != 'deleted'
               AND NOT (?3 AND t.status = 'unread' AND NOT EXISTS (SELECT 1 FROM suggestions s WHERE s.thread_id = t.id))
             ORDER BY t.last_date DESC LIMIT ?2",
        )?;
        Ok(st
            .query_map(params![state, limit, wait_for_router], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Whether a thread is newer than the recent window (grouped first, at full speed).
    pub fn is_recent_for_grouping(&self, tid: &str) -> Result<bool> {
        let r = self.r.lock().unwrap();
        let last: i64 = r
            .query_row("SELECT last_date FROM threads WHERE id = ?1", [tid], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(0);
        Ok(now_ms() - last <= conv::RECENT_MS)
    }

    /// Decides a pending thread's stream. Feeds and lists join their stream right away;
    /// a person's thread moves on to judgment when it has someone to be judged against.
    pub fn classify_for_grouping(&self, tid: &str) -> Result<GroupingPlan> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let account: Option<String> = tx
            .query_row("SELECT account_id FROM threads WHERE id = ?1", [tid], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(account) = account else {
            return Ok(GroupingPlan::Alone);
        };
        let me = my_addresses(&tx, &account)?;
        let Some(tp) = thread_people(&tx, tid, &me)? else {
            return Ok(GroupingPlan::Alone);
        };
        let automated: Option<f64> = tx
            .query_row(
                "SELECT automated FROM suggestions WHERE thread_id = ?1",
                [tid],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let kind = conv::kind(
            tp.list_id.as_deref(),
            &tp.first_sender,
            tp.user_wrote,
            automated,
        );
        tx.execute(
            "UPDATE threads SET conv_kind = ?2, conv_key = ?3 WHERE id = ?1",
            params![tid, kind.name(), kind.key()],
        )?;
        let plan = match &kind {
            Kind::Feed(key) | Kind::List(key) => {
                let into: Option<String> = tx
                    .query_row(
                        "SELECT conversation_id FROM threads WHERE conv_kind = ?1 AND conv_key = ?2 AND account_id = ?3
                           AND id != ?4 AND conversation_id IS NOT NULL ORDER BY last_date DESC LIMIT 1",
                        params![kind.name(), key, account, tid],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(into) = into {
                    absorb(&tx, &conversation_of(&tx, tid)?, &into)?;
                }
                tx.execute(
                    "UPDATE threads SET conv_state = 'done' WHERE id = ?1",
                    [tid],
                )?;
                GroupingPlan::Grouped
            }
            Kind::Person if tp.people.is_empty() => {
                tx.execute(
                    "UPDATE threads SET conv_state = 'done' WHERE id = ?1",
                    [tid],
                )?;
                GroupingPlan::Alone
            }
            Kind::Person => {
                tx.execute(
                    "UPDATE threads SET conv_state = 'judge' WHERE id = ?1",
                    [tid],
                )?;
                GroupingPlan::Judge {
                    people: tp.people.into_iter().collect(),
                    join_at: conv::JOIN,
                    candidates: vec![],
                }
            }
        };
        tx.commit()?;
        Ok(plan)
    }

    /// The earlier conversations a person's thread may continue, with the pair states
    /// Jev judges: at most two, sharing someone, active within 30 days before it started.
    pub fn grouping_candidates(&self, tid: &str) -> Result<GroupingPlan> {
        let r = self.bg.lock().unwrap();
        let account: Option<String> = r
            .query_row("SELECT account_id FROM threads WHERE id = ?1", [tid], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(account) = account else {
            return Ok(GroupingPlan::Alone);
        };
        let me = my_addresses(&r, &account)?;
        let Some(tp) = thread_people(&r, tid, &me)? else {
            return Ok(GroupingPlan::Alone);
        };
        if tp.people.is_empty() {
            return Ok(GroupingPlan::Alone);
        }
        let own = conversation_of(&r, tid)?;
        let mut st = r.prepare_cached(
            "SELECT id, COALESCE(conversation_id, id) FROM threads
             WHERE account_id = ?1 AND last_date >= ?2 AND first_date < ?3 AND id != ?4
               AND conv_kind = 'person' AND status != 'deleted'
             ORDER BY last_date DESC LIMIT 400",
        )?;
        let window: Vec<(String, String)> = st
            .query_map(
                params![
                    tp.account_id,
                    tp.first_date - conv::WINDOW_MS,
                    tp.first_date,
                    tid
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?
            .collect::<rusqlite::Result<_>>()?;
        let mut seen: BTreeSet<String> = BTreeSet::from([own.clone()]);
        let mut candidates = vec![];
        let later = pair_item(&r, tid)?;
        for (u, cid) in window {
            if candidates.len() >= conv::MAX_CANDIDATES {
                break;
            }
            if seen.contains(&cid) {
                continue;
            }
            // Never re-judge what the user (or an earlier verdict) already settled.
            let settled = r
                .prepare_cached(
                    "SELECT 1 FROM conversation_pairs WHERE later = ?1 AND earlier = ?2",
                )?
                .exists(params![tid, u])?;
            let Some(up) = thread_people(&r, &u, &me)? else {
                continue;
            };
            if settled || up.people.is_disjoint(&tp.people) {
                continue;
            }
            seen.insert(cid.clone());
            let earlier = pair_item(&r, &u)?;
            let days = (tp.first_date - up.last_date) / 86_400_000;
            candidates.push(PairCandidate {
                earlier: u,
                conversation_id: cid,
                state: conv::pair_state(&earlier, &later, days),
            });
        }
        if candidates.is_empty() {
            return Ok(GroupingPlan::Alone);
        }
        let people: Vec<String> = tp.people.into_iter().collect();
        let mut splits = 0i64;
        let mut count = r.prepare_cached(
            "SELECT COUNT(*) FROM conversation_pairs WHERE source = 'user' AND decision = 'apart'
               AND EXISTS (SELECT 1 FROM json_each(people) j WHERE j.value = ?1)",
        )?;
        for p in &people {
            splits = splits.max(count.query_row([p], |r| r.get(0))?);
        }
        Ok(GroupingPlan::Judge {
            people,
            join_at: conv::join_threshold(splits),
            candidates,
        })
    }

    /// Settles a thread with nothing (left) to judge.
    pub fn finish_grouping(&self, tid: &str) -> Result<()> {
        self.w.lock().unwrap().execute(
            "UPDATE threads SET conv_state = 'done' WHERE id = ?1",
            [tid],
        )?;
        Ok(())
    }

    /// Records Jev's verdicts on a thread's candidates and acts on them: the most likely
    /// join above the bar wins; uncertain pairs wait for the AI; the rest stay apart.
    pub fn apply_jev_verdicts(
        &self,
        tid: &str,
        people: &[String],
        join_at: f64,
        verdicts: &[(PairCandidate, f64)],
    ) -> Result<Option<String>> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let best = verdicts
            .iter()
            .filter(|(_, p)| conv::decide(*p, join_at) == conv::Decision::Join)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let mut asked = false;
        for (c, p) in verdicts {
            let decision = match conv::decide(*p, join_at) {
                conv::Decision::Join if best.is_some_and(|(b, _)| b.earlier == c.earlier) => "join",
                conv::Decision::AskAi if best.is_none() => {
                    asked = true;
                    "ask_ai"
                }
                _ => "apart",
            };
            record_pair(
                &tx,
                tid,
                &c.earlier,
                people,
                (Some(*p), None),
                decision,
                "jev",
            )?;
        }
        let joined = match best {
            Some((c, _)) => {
                let into = conversation_of(&tx, &c.earlier)?;
                absorb(&tx, &conversation_of(&tx, tid)?, &into)?;
                Some(into)
            }
            None => None,
        };
        tx.execute(
            "UPDATE threads SET conv_state = ?2 WHERE id = ?1",
            params![tid, if asked { "ai" } else { "done" }],
        )?;
        tx.commit()?;
        Ok(joined)
    }

    /// The next pair waiting for the AI on a thread: its most likely candidate.
    pub fn next_ai_pair(&self, tid: &str) -> Result<Option<(String, String, serde_json::Value)>> {
        let r = self.bg.lock().unwrap();
        let pair: Option<String> = r
            .query_row(
                "SELECT earlier FROM conversation_pairs WHERE later = ?1 AND decision = 'ask_ai' ORDER BY jev DESC LIMIT 1",
                [tid],
                |r| r.get(0),
            )
            .optional()?;
        let Some(earlier) = pair else {
            return Ok(None);
        };
        let account: String =
            r.query_row("SELECT account_id FROM threads WHERE id = ?1", [tid], |r| {
                r.get(0)
            })?;
        let (first, last): (i64, i64) = (
            r.query_row("SELECT first_date FROM threads WHERE id = ?1", [tid], |r| {
                r.get(0)
            })?,
            r.query_row(
                "SELECT last_date FROM threads WHERE id = ?1",
                [&earlier],
                |r| r.get(0),
            )?,
        );
        let state = conv::pair_state(
            &pair_item(&r, &earlier)?,
            &pair_item(&r, tid)?,
            (first - last) / 86_400_000,
        );
        Ok(Some((account, earlier, state)))
    }

    /// Records the AI's verdict on one pair; a yes joins, a no moves to the next pair.
    pub fn apply_ai_verdict(&self, tid: &str, earlier: &str, same: bool) -> Result<Option<String>> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        tx.execute(
            "UPDATE conversation_pairs SET ai = ?3, decision = ?4, source = 'ai' WHERE later = ?1 AND earlier = ?2",
            params![tid, earlier, same as i64, if same { "join" } else { "apart" }],
        )?;
        let joined = if same {
            tx.execute(
                "UPDATE conversation_pairs SET decision = 'apart' WHERE later = ?1 AND decision = 'ask_ai'",
                [tid],
            )?;
            let into = conversation_of(&tx, earlier)?;
            absorb(&tx, &conversation_of(&tx, tid)?, &into)?;
            Some(into)
        } else {
            None
        };
        let left = tx
            .prepare_cached(
                "SELECT 1 FROM conversation_pairs WHERE later = ?1 AND decision = 'ask_ai'",
            )?
            .exists([tid])?;
        if !left {
            tx.execute(
                "UPDATE threads SET conv_state = 'done' WHERE id = ?1",
                [tid],
            )?;
        }
        tx.commit()?;
        Ok(joined)
    }

    /// Takes a thread out of its conversation for good. Returns the threads touched.
    pub fn split_thread(&self, tid: &str) -> Result<Vec<String>> {
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let cid = conversation_of(&tx, tid)?;
        let others: Vec<String> = tx
            .prepare_cached("SELECT id FROM threads WHERE conversation_id = ?1 AND id != ?2 ORDER BY last_date DESC")?
            .query_map(params![cid, tid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if others.is_empty() {
            return Ok(vec![tid.to_string()]);
        }
        let account: String =
            tx.query_row("SELECT account_id FROM threads WHERE id = ?1", [tid], |r| {
                r.get(0)
            })?;
        let me = my_addresses(&tx, &account)?;
        let people: Vec<String> = thread_people(&tx, tid, &me)?
            .map(|p| p.people.into_iter().collect())
            .unwrap_or_default();
        // The conversation keeps its id unless it was this thread's own: then the rest
        // takes the id of its latest thread.
        if cid == tid {
            tx.execute(
                "UPDATE threads SET conversation_id = ?2 WHERE conversation_id = ?1 AND id != ?1",
                params![tid, others[0]],
            )?;
        } else {
            tx.execute(
                "UPDATE threads SET conversation_id = ?1 WHERE id = ?1",
                [tid],
            )?;
        }
        tx.execute(
            "UPDATE threads SET conv_state = 'done', conv_kind = 'person' WHERE id = ?1",
            [tid],
        )?;
        for o in &others {
            let (later, earlier) = order_pair(&tx, tid, o)?;
            record_pair(
                &tx,
                &later,
                &earlier,
                &people,
                (None, None),
                "apart",
                "user",
            )?;
        }
        tx.commit()?;
        let mut touched = others;
        touched.push(tid.to_string());
        Ok(touched)
    }

    /// Joins the conversations of the given threads into the one of the latest thread.
    /// Returns the conversation id and every thread now in it.
    pub fn merge_threads(&self, ids: &[String]) -> Result<(String, Vec<String>)> {
        if ids.len() < 2 {
            return Err(Error::InvalidParams(
                "pick at least two conversations to merge".into(),
            ));
        }
        let mut w = self.w.lock().unwrap();
        let tx = w.transaction()?;
        let mut latest: Option<(i64, String)> = None;
        for id in ids {
            let d: i64 = tx
                .query_row("SELECT last_date FROM threads WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()?
                .ok_or_else(|| Error::NotFound(format!("thread {id}")))?;
            if latest.as_ref().is_none_or(|(ld, _)| d > *ld) {
                latest = Some((d, id.clone()));
            }
        }
        let (_, lead) = latest.expect("two ids");
        let into = conversation_of(&tx, &lead)?;
        for id in ids.iter().filter(|i| **i != lead) {
            absorb(&tx, &conversation_of(&tx, id)?, &into)?;
            let (later, earlier) = order_pair(&tx, id, &lead)?;
            record_pair(&tx, &later, &earlier, &[], (None, None), "join", "user")?;
        }
        tx.execute(
            "UPDATE threads SET conv_state = 'done' WHERE conversation_id = ?1 AND conv_state != 'done'",
            [&into],
        )?;
        let members: Vec<String> = tx
            .prepare_cached("SELECT id FROM threads WHERE conversation_id = ?1")?
            .query_map([&into], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        tx.commit()?;
        Ok((into, members))
    }

    /// Ids of a thread's conversation, oldest first.
    pub fn conversation_thread_ids(&self, tid: &str) -> Result<Vec<String>> {
        let r = self.r.lock().unwrap();
        let cid = conversation_of(&r, tid)?;
        Ok(r.prepare_cached(
            "SELECT id FROM threads WHERE conversation_id = ?1 ORDER BY first_date, id",
        )?
        .query_map([cid], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
    }

    /// The other open threads of a thread's conversation: they follow its status changes.
    pub fn open_siblings(&self, tid: &str) -> Result<Vec<String>> {
        let r = self.r.lock().unwrap();
        open_siblings(&r, tid)
    }

    /// Every message of a thread's conversation, oldest first.
    pub fn conversation_messages(&self, tid: &str) -> Result<Vec<Message>> {
        let mut out = vec![];
        for t in self.conversation_thread_ids(tid)? {
            out.extend(self.messages(&t)?);
        }
        out.sort_by_key(|m| m.date);
        Ok(out)
    }

    /// What the reading pane shows about a thread's conversation besides its messages.
    pub fn conversation(&self, tid: &str, messages: &[Message]) -> Result<Conversation> {
        let ids = self.conversation_thread_ids(tid)?;
        let mut threads = ids
            .iter()
            .map(|t| self.thread(t))
            .collect::<Result<Vec<_>>>()?;
        threads.sort_by_key(|t| t.last_date);
        let mut people: Vec<ConversationPerson> = vec![];
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut add = |a: &Address, wrote: bool| {
            let key = a.email.to_lowercase();
            if key.is_empty() {
                return;
            }
            let i = *seen.entry(key.clone()).or_insert_with(|| {
                people.push(ConversationPerson {
                    name: a.name.clone(),
                    email: key.clone(),
                    title: String::new(),
                    company: String::new(),
                    messages: 0,
                });
                people.len() - 1
            });
            if wrote {
                people[i].messages += 1;
            }
            if people[i].name.is_empty() {
                people[i].name = a.name.clone();
            }
        };
        let mut files = vec![];
        let mut links = vec![];
        for m in messages {
            if !m.is_from_me {
                add(&m.from, true);
            }
            for a in m.to.iter().chain(&m.cc) {
                add(a, false);
            }
            let from = if m.is_from_me {
                "You".to_string()
            } else if m.from.name.is_empty() {
                m.from.email.clone()
            } else {
                m.from.name.clone()
            };
            for a in &m.attachments {
                files.push(SharedFile {
                    id: a.id.clone(),
                    filename: a.filename.clone(),
                    mime: a.mime.clone(),
                    size: a.size,
                    message_id: m.id.clone(),
                    thread_id: m.thread_id.clone(),
                    from: from.clone(),
                    date: m.date,
                });
            }
            for url in conv::links(&m.body_new) {
                if !links.iter().any(|l: &SharedLink| l.url == url) {
                    links.push(SharedLink {
                        url,
                        message_id: m.id.clone(),
                        from: from.clone(),
                        date: m.date,
                    });
                }
            }
        }
        let me: BTreeSet<String> = messages
            .iter()
            .filter(|m| m.is_from_me)
            .map(|m| m.from.email.to_lowercase())
            .chain(
                threads
                    .first()
                    .and_then(|t| self.account(&t.account_id).ok())
                    .map(|a| a.email.to_lowercase()),
            )
            .collect();
        people.retain(|p| !me.contains(&p.email));
        for p in &mut people {
            if let Ok(c) = self.contact(&p.email) {
                p.title = c.card.title;
                p.company = c.card.company;
                if !c.card.name.is_empty() {
                    p.name = c.card.name;
                }
            }
        }
        people.sort_by(|a, b| b.messages.cmp(&a.messages));
        files.reverse();
        links.reverse();
        let lead = threads
            .iter()
            .max_by_key(|t| t.last_date)
            .map(|t| t.conversation_id.clone())
            .unwrap_or_else(|| tid.to_string());
        let kind = threads
            .iter()
            .map(|t| t.conversation_kind.clone())
            .find(|k| k != "person")
            .unwrap_or_else(|| "person".into());
        Ok(Conversation {
            id: lead,
            kind,
            threads,
            people,
            files,
            links,
        })
    }

    /// Files shared anywhere, newest first, optionally only those whose name matches.
    pub fn shared_files(&self, query: Option<&str>, limit: i64) -> Result<Vec<SharedFile>> {
        let r = self.r.lock().unwrap();
        let like = format!("%{}%", query.unwrap_or("").trim());
        let mut st = r.prepare_cached(
            "SELECT a.id, a.filename, a.mime, a.size, m.id, m.thread_id,
               CASE WHEN m.is_from_me = 1 THEN 'You' WHEN m.from_name != '' THEN m.from_name ELSE m.from_email END, m.date
             FROM attachments a JOIN messages m ON m.id = a.message_id JOIN threads t ON t.id = m.thread_id
             WHERE t.status != 'deleted' AND a.filename LIKE ?1
             ORDER BY m.date DESC LIMIT ?2",
        )?;
        Ok(st
            .query_map(params![like, limit], |r| {
                Ok(SharedFile {
                    id: r.get(0)?,
                    filename: r.get(1)?,
                    mime: r.get(2)?,
                    size: r.get(3)?,
                    message_id: r.get(4)?,
                    thread_id: r.get(5)?,
                    from: r.get(6)?,
                    date: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Grouping progress: threads still waiting per stage.
    pub fn grouping_backlog(&self) -> Result<HashMap<String, i64>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT conv_state, COUNT(*) FROM threads WHERE conv_state != 'done' AND status != 'deleted' GROUP BY conv_state",
        )?;
        Ok(st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }
}

pub(super) fn open_siblings(c: &Connection, tid: &str) -> Result<Vec<String>> {
    let cid = conversation_of(c, tid)?;
    Ok(c.prepare_cached(
        "SELECT id FROM threads WHERE conversation_id = ?1 AND id != ?2
               AND status IN (SELECT key FROM statuses WHERE archived = 0)",
    )?
    .query_map(params![cid, tid], |r| r.get(0))?
    .collect::<rusqlite::Result<_>>()?)
}

/// (later, earlier) by first message date.
fn order_pair(c: &Connection, a: &str, b: &str) -> Result<(String, String)> {
    let first = |t: &str| -> Result<i64> {
        Ok(
            c.query_row("SELECT first_date FROM threads WHERE id = ?1", [t], |r| {
                r.get(0)
            })?,
        )
    };
    Ok(if first(a)? >= first(b)? {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::{ParsedAttachment, ParsedMessage};
    use crate::store::{IncomingMessage, SyncMode, ThreadFilter, ThreadPatch};
    use rstest::*;

    const DAY: i64 = 86_400_000;
    const ME: &str = "me@example.com";

    struct Env {
        _dir: tempfile::TempDir,
        store: Store,
        acc: String,
        now: i64,
    }

    #[fixture]
    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        let acc = store.add_account(ME, "gmail").unwrap().id;
        Env {
            _dir: dir,
            store,
            acc,
            now: now_ms(),
        }
    }

    /// One message in Gmail thread `gthread`; `from` "me" is the user writing to `peer`.
    struct Mail<'a> {
        gthread: &'a str,
        from: &'a str,
        peer: &'a str,
        subject: &'a str,
        body: &'a str,
        days_ago: i64,
        list_id: Option<&'a str>,
        file: Option<&'a str>,
    }

    impl Default for Mail<'_> {
        fn default() -> Self {
            Mail {
                gthread: "g1",
                from: "nora@agency.example",
                peer: "nora@agency.example",
                subject: "Flat sale",
                body: "Hello",
                days_ago: 1,
                list_id: None,
                file: None,
            }
        }
    }

    impl Env {
        fn mail(&self, m: Mail) -> String {
            let mine = m.from == "me";
            let addr = |e: &str| Address {
                name: String::new(),
                email: e.into(),
            };
            let (from, to) = if mine {
                (addr(ME), addr(m.peer))
            } else {
                (addr(m.from), addr(ME))
            };
            let n: u64 = rand::random();
            let msg = IncomingMessage {
                provider_id: format!("p{n}"),
                provider_thread_id: m.gthread.into(),
                label_ids: if mine {
                    vec!["SENT".into()]
                } else {
                    vec!["INBOX".into()]
                },
                internal_date: self.now - m.days_ago * DAY,
                parsed: ParsedMessage {
                    from: Some(from),
                    to: vec![to],
                    subject: m.subject.into(),
                    body_text: m.body.into(),
                    list_id: m.list_id.map(String::from),
                    attachments: m
                        .file
                        .map(|f| {
                            vec![ParsedAttachment {
                                filename: f.into(),
                                mime: "application/pdf".into(),
                                data: b"%PDF".to_vec(),
                            }]
                        })
                        .unwrap_or_default(),
                    ..Default::default()
                },
            };
            self.store
                .ingest(&self.acc, ME, &msg, SyncMode::Initial)
                .unwrap()
        }

        fn conv(&self, tid: &str) -> String {
            self.store.thread(tid).unwrap().conversation_id
        }

        fn set_status(&self, tid: &str, status: &str) {
            self.store
                .w
                .lock()
                .unwrap()
                .execute(
                    "UPDATE threads SET status = ?2 WHERE id = ?1",
                    [tid, status],
                )
                .unwrap();
        }

        /// Earlier thread with Nora (the user wrote to her), then her new thread.
        fn pair(&self, gap_days: i64) -> (String, String) {
            let a = self.mail(Mail {
                gthread: "ga",
                from: "me",
                subject: "Flat sale, next steps",
                body: "Here is the cellar plan.",
                days_ago: gap_days + 2,
                ..Default::default()
            });
            let b = self.mail(Mail {
                gthread: "gb",
                subject: "Signing postponed",
                body: "The signing moves to the 20th.",
                days_ago: 1,
                ..Default::default()
            });
            (a, b)
        }

        fn candidates(&self, tid: &str) -> Vec<PairCandidate> {
            match self.store.grouping_candidates(tid).unwrap() {
                GroupingPlan::Judge { candidates, .. } => candidates,
                _ => vec![],
            }
        }

        fn jev(&self, tid: &str, p: f64) -> Option<String> {
            let verdicts: Vec<(PairCandidate, f64)> =
                self.candidates(tid).into_iter().map(|c| (c, p)).collect();
            self.store
                .apply_jev_verdicts(tid, &["nora@agency.example".into()], conv::JOIN, &verdicts)
                .unwrap()
        }

        fn state(&self, tid: &str) -> String {
            self.store
                .r
                .lock()
                .unwrap()
                .query_row("SELECT conv_state FROM threads WHERE id = ?1", [tid], |r| {
                    r.get(0)
                })
                .unwrap()
        }

        fn joined_pair(&self) -> (String, String) {
            let (a, b) = self.pair(3);
            self.jev(&b, 0.9);
            (a, b)
        }

        fn leads(&self) -> Vec<String> {
            self.store
                .threads(&ThreadFilter {
                    conversations: true,
                    ..Default::default()
                })
                .unwrap()
                .0
                .into_iter()
                .map(|t| t.id)
                .collect()
        }
    }

    #[rstest]
    fn a_new_thread_is_its_own_conversation(env: Env) {
        let t = env.mail(Mail::default());
        assert_eq!(env.conv(&t), t);
    }

    #[rstest]
    fn a_feed_joins_the_same_senders_stream(env: Env) {
        let a = env.mail(Mail {
            gthread: "s1",
            from: "noreply@bank.example",
            subject: "Statement",
            ..Default::default()
        });
        let b = env.mail(Mail {
            gthread: "s2",
            from: "noreply@bank.example",
            subject: "Card payment",
            days_ago: 0,
            ..Default::default()
        });
        env.store.classify_for_grouping(&a).unwrap();
        env.store.classify_for_grouping(&b).unwrap();
        assert_eq!(env.conv(&b), env.conv(&a));
    }

    #[rstest]
    fn a_feed_is_marked_as_a_feed(env: Env) {
        let a = env.mail(Mail {
            gthread: "s1",
            from: "noreply@bank.example",
            ..Default::default()
        });
        env.store.classify_for_grouping(&a).unwrap();
        assert_eq!(env.store.thread(&a).unwrap().conversation_kind, "feed");
    }

    #[rstest]
    fn a_mailing_list_joins_its_list(env: Env) {
        let a = env.mail(Mail {
            gthread: "l1",
            from: "ada@club.example",
            list_id: Some("runners.club.example"),
            ..Default::default()
        });
        let b = env.mail(Mail {
            gthread: "l2",
            from: "sam@club.example",
            list_id: Some("runners.club.example"),
            days_ago: 0,
            ..Default::default()
        });
        env.store.classify_for_grouping(&a).unwrap();
        env.store.classify_for_grouping(&b).unwrap();
        assert_eq!(env.conv(&b), env.conv(&a));
    }

    #[rstest]
    fn a_stranger_is_never_judged(env: Env) {
        let t = env.mail(Mail {
            from: "someone@else.example",
            ..Default::default()
        });
        assert_eq!(
            env.store.classify_for_grouping(&t).unwrap(),
            GroupingPlan::Alone
        );
    }

    #[rstest]
    fn someone_the_user_wrote_to_goes_to_judgment(env: Env) {
        let (_, b) = env.pair(3);
        env.store.classify_for_grouping(&b).unwrap();
        assert_eq!(env.state(&b), "judge");
    }

    #[rstest]
    fn the_earlier_thread_is_a_candidate(env: Env) {
        let (a, b) = env.pair(3);
        assert_eq!(env.candidates(&b)[0].earlier, a);
    }

    #[rstest]
    fn the_pair_state_shows_the_later_subject(env: Env) {
        let (_, b) = env.pair(3);
        assert_eq!(
            env.candidates(&b)[0].state["later"]["subject"],
            "Signing postponed"
        );
    }

    #[rstest]
    fn threads_older_than_the_window_are_not_candidates(env: Env) {
        let (_, b) = env.pair(45);
        assert!(env.candidates(&b).is_empty());
    }

    #[rstest]
    fn other_peoples_threads_are_not_candidates(env: Env) {
        env.mail(Mail {
            gthread: "gx",
            from: "me",
            peer: "tom@garden.example",
            days_ago: 3,
            ..Default::default()
        });
        let b = env.mail(Mail {
            gthread: "gb",
            ..Default::default()
        });
        assert!(env.candidates(&b).is_empty());
    }

    #[rstest]
    fn a_confident_verdict_joins(env: Env) {
        let (a, b) = env.joined_pair();
        assert_eq!(env.conv(&b), env.conv(&a));
    }

    #[rstest]
    fn an_uncertain_verdict_waits_for_the_ai(env: Env) {
        let (_, b) = env.pair(3);
        env.jev(&b, 0.6);
        assert_eq!(env.state(&b), "ai");
    }

    #[rstest]
    fn an_unlikely_verdict_stays_apart(env: Env) {
        let (a, b) = env.pair(3);
        env.jev(&b, 0.2);
        assert_ne!(env.conv(&b), env.conv(&a));
    }

    #[rstest]
    fn the_ai_gets_the_pair_to_judge(env: Env) {
        let (a, b) = env.pair(3);
        env.jev(&b, 0.6);
        assert_eq!(env.store.next_ai_pair(&b).unwrap().unwrap().1, a);
    }

    #[rstest]
    fn the_ai_saying_yes_joins(env: Env) {
        let (a, b) = env.pair(3);
        env.jev(&b, 0.6);
        env.store.apply_ai_verdict(&b, &a, true).unwrap();
        assert_eq!(env.conv(&b), env.conv(&a));
    }

    #[rstest]
    fn the_ai_saying_no_settles_the_thread(env: Env) {
        let (a, b) = env.pair(3);
        env.jev(&b, 0.6);
        env.store.apply_ai_verdict(&b, &a, false).unwrap();
        assert_eq!(env.state(&b), "done");
    }

    #[rstest]
    fn a_judged_pair_is_not_judged_again(env: Env) {
        let (_, b) = env.pair(3);
        env.jev(&b, 0.2);
        assert!(env.candidates(&b).is_empty());
    }

    #[rstest]
    fn lists_show_one_row_per_conversation(env: Env) {
        let (_, b) = env.joined_pair();
        assert_eq!(env.leads(), vec![b]);
    }

    #[rstest]
    fn the_lead_counts_its_threads(env: Env) {
        let (_, b) = env.joined_pair();
        assert_eq!(env.store.thread(&b).unwrap().conversation_threads, 2);
    }

    #[rstest]
    fn an_open_thread_leads_over_a_newer_closed_one(env: Env) {
        let (a, b) = env.joined_pair();
        env.set_status(&b, "archive");
        env.set_status(&a, "todo");
        assert_eq!(env.leads(), vec![a]);
    }

    #[rstest]
    fn a_status_change_moves_the_whole_conversation(env: Env) {
        let (a, b) = env.joined_pair();
        env.set_status(&a, "todo");
        env.set_status(&b, "todo");
        env.store
            .update_threads(
                std::slice::from_ref(&b),
                &ThreadPatch {
                    status: Some("done".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(env.store.thread(&a).unwrap().status, "done");
    }

    #[rstest]
    fn counts_count_conversations(env: Env) {
        let (a, b) = env.joined_pair();
        env.set_status(&a, "todo");
        env.set_status(&b, "todo");
        assert_eq!(env.store.counts(true).unwrap()["status"]["todo"], 1);
    }

    #[rstest]
    fn the_conversation_reads_every_message(env: Env) {
        let (_, b) = env.joined_pair();
        assert_eq!(env.store.conversation_messages(&b).unwrap().len(), 2);
    }

    #[rstest]
    fn messages_say_which_thread_they_come_from(env: Env) {
        let (a, b) = env.joined_pair();
        assert_eq!(env.store.conversation_messages(&b).unwrap()[0].thread_id, a);
    }

    #[rstest]
    fn splitting_takes_the_thread_out(env: Env) {
        let (a, b) = env.joined_pair();
        env.store.split_thread(&b).unwrap();
        assert_ne!(env.conv(&b), env.conv(&a));
    }

    #[rstest]
    fn splitting_the_thread_naming_the_conversation_keeps_the_rest_together(env: Env) {
        let (a, b) = env.joined_pair();
        let c = env.mail(Mail {
            gthread: "gc",
            subject: "Keys",
            days_ago: 0,
            ..Default::default()
        });
        env.store.merge_threads(&[a.clone(), c.clone()]).unwrap();
        env.store.split_thread(&a).unwrap();
        assert_eq!(env.conv(&c), env.conv(&b));
    }

    #[rstest]
    fn a_split_raises_the_bar_for_that_person(env: Env) {
        let (_, b) = env.joined_pair();
        env.store.split_thread(&b).unwrap();
        let c = env.mail(Mail {
            gthread: "gc",
            subject: "Keys",
            days_ago: 0,
            ..Default::default()
        });
        let GroupingPlan::Judge { join_at, .. } = env.store.grouping_candidates(&c).unwrap() else {
            panic!("expected a judgment");
        };
        assert!((join_at - 0.9).abs() < 1e-9);
    }

    #[rstest]
    fn merging_joins_two_conversations(env: Env) {
        let (a, b) = env.pair(3);
        env.store.merge_threads(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(env.conv(&a), env.conv(&b));
    }

    #[rstest]
    fn merging_one_conversation_is_refused(env: Env) {
        let (a, _) = env.pair(3);
        assert!(env.store.merge_threads(&[a]).is_err());
    }

    #[rstest]
    fn shared_links_are_listed(env: Env) {
        let t = env.mail(Mail {
            body: "Book here https://notary.example/rdv",
            ..Default::default()
        });
        let msgs = env.store.conversation_messages(&t).unwrap();
        assert_eq!(
            env.store.conversation(&t, &msgs).unwrap().links[0].url,
            "https://notary.example/rdv"
        );
    }

    #[rstest]
    fn shared_files_are_listed(env: Env) {
        let t = env.mail(Mail {
            file: Some("cellar-plan.pdf"),
            ..Default::default()
        });
        let msgs = env.store.conversation_messages(&t).unwrap();
        assert_eq!(
            env.store.conversation(&t, &msgs).unwrap().files[0].filename,
            "cellar-plan.pdf"
        );
    }

    #[rstest]
    fn people_leave_the_user_out(env: Env) {
        let (_, b) = env.joined_pair();
        let msgs = env.store.conversation_messages(&b).unwrap();
        let people: Vec<String> = env
            .store
            .conversation(&b, &msgs)
            .unwrap()
            .people
            .into_iter()
            .map(|p| p.email)
            .collect();
        assert_eq!(people, vec!["nora@agency.example"]);
    }

    #[rstest]
    fn the_docs_list_finds_files_by_name(env: Env) {
        env.mail(Mail {
            file: Some("cellar-plan.pdf"),
            ..Default::default()
        });
        assert_eq!(env.store.shared_files(Some("cellar"), 10).unwrap().len(), 1);
    }

    #[rstest]
    fn unrouted_unread_mail_waits_for_the_router(env: Env) {
        let t = env.mail(Mail::default());
        env.set_status(&t, "unread");
        assert!(
            env.store
                .threads_to_group("pending", 10, true)
                .unwrap()
                .is_empty()
        );
    }
}
