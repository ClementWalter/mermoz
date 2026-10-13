//! The feed's storage: Jev's judgments, the AI's card words and snoozes, all kept per
//! thread until new mail arrives in it, and the open work read back as feed units.

use super::{ColumnScope, Store, THREAD_COLS, attach, is_automated, thread_row};
use crate::error::Result;
use crate::feed::{self, FeedUnit, Judgment, Kind, Texts};
use crate::model::{ThreadSummary, now_ms};
use rusqlite::{OptionalExtension, params, params_from_iter};

/// Later remembers what the user picked ("Friday evening") and whether the mail came back.
pub const LATER_MIGRATION: &str = "ALTER TABLE feed_snoozes ADD COLUMN label TEXT;
ALTER TABLE feed_snoozes ADD COLUMN woke INTEGER NOT NULL DEFAULT 0;";

/// A mail that came back from Later stays in Now this long, with its hint.
const BACK_FOR_MS: i64 = 2 * 86_400_000;

/// Statuses whose threads are always in the feed.
pub const FEED_STATUSES: &[&str] = &["unread", "todo", "in_progress", "waiting"];

/// Backlog mail older than this is never scanned for close dates.
const BACKLOG_WINDOW_MS: i64 = 60 * 86_400_000;

/// Peer threads looked at for how the user and these people usually write.
const PEER_THREADS: i64 = 60;

const DAY_MS: i64 = 86_400_000;

/// Columns read for each unit after the thread's own (see `feed_units`). The two message
/// lookups run only where they matter (waiting threads, threads Jev has not judged), so a
/// feed of thousands of open threads stays within a frame.
const UNIT_COLS: &str = "j.judged, j.kind, j.needs_reply, j.urgency, j.due, j.due_time, j.amount, j.automated, \
    j.sender_weight, j.typical_reply_days, x.json, \
    CASE WHEN t.status = 'waiting' THEN (SELECT MAX(m.date) FROM messages m WHERE m.thread_id = t.id AND m.is_from_me = 1) END, \
    CASE WHEN j.judged IS NOT 1 OR j.kind IN ('cleanup', 'reading') OR j.automated >= 0.7 \
      THEN (SELECT m.from_email FROM messages m WHERE m.thread_id = t.id ORDER BY m.date DESC LIMIT 1) END, \
    z.until, z.label, z.woke";

/// How many columns `UNIT_COLS` has.
const UNIT_COL_COUNT: usize = 16;

/// A unit as read from the store: thread, judgment, card words, the user's last
/// message date and the last sender's address.
type UnitRow = (
    ThreadSummary,
    Option<Judgment>,
    Option<Texts>,
    Option<i64>,
    String,
    Option<String>,
);

/// A thread to judge: id, the date of its latest message, and whether it is Backlog.
pub type ToJudge = (String, i64, bool);

fn kind_key(k: Kind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn kind_of(s: Option<String>) -> Option<Kind> {
    s.and_then(|s| serde_json::from_value(serde_json::Value::String(s)).ok())
}

/// The middle of a list of delays, in days.
pub fn median_days(mut gaps_ms: Vec<i64>) -> Option<f64> {
    if gaps_ms.is_empty() {
        return None;
    }
    gaps_ms.sort_unstable();
    let mid = gaps_ms.len() / 2;
    let ms = if gaps_ms.len().is_multiple_of(2) {
        (gaps_ms[mid - 1] + gaps_ms[mid]) as f64 / 2.0
    } else {
        gaps_ms[mid] as f64
    };
    Some(ms / DAY_MS as f64)
}

impl Store {
    /// Open threads without a judgment for their latest message, newest first, then
    /// recent Backlog threads not checked today. Backlog threads are only judged when
    /// they mention a close date (see [`feed::backlog_worth_judging`]).
    pub fn threads_to_judge(&self, today: &str, limit: i64) -> Result<Vec<ToJudge>> {
        let r = self.bg.lock().unwrap();
        let marks = vec!["?"; FEED_STATUSES.len()].join(",");
        let mut st = r.prepare_cached(&format!(
            "SELECT t.id, t.last_date FROM threads t LEFT JOIN feed_judgments j ON j.thread_id = t.id
             WHERE t.status IN ({marks}) AND (j.thread_id IS NULL OR j.for_date < t.last_date OR j.judged = 0)
             ORDER BY t.last_date DESC LIMIT {limit}"
        ))?;
        let mut out: Vec<ToJudge> = st
            .query_map(params_from_iter(FEED_STATUSES.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?, false))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let left = limit - out.len() as i64;
        if left > 0 {
            let mut st = r.prepare_cached(
                "SELECT t.id, t.last_date FROM threads t LEFT JOIN feed_judgments j ON j.thread_id = t.id
                 WHERE t.status = 'backlog' AND t.last_date >= ?1
                   AND (j.thread_id IS NULL OR j.for_date < t.last_date OR (j.judged = 0 AND j.checked_day < ?2))
                 ORDER BY t.last_date DESC LIMIT ?3",
            )?;
            out.extend(
                st.query_map(params![now_ms() - BACKLOG_WINDOW_MS, today, left], |r| {
                    Ok((r.get(0)?, r.get(1)?, true))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        }
        Ok(out)
    }

    /// The text the feed's date and amount finders read: subject and latest messages, as
    /// Jev sees them.
    pub fn feed_text(input: &crate::jev::RouteInput) -> String {
        let mut text = input.subject.clone();
        for m in &input.recent_messages {
            text.push('\n');
            text.push_str(&m.text);
        }
        text
    }

    /// How much the user answers these people (share of their threads the user wrote in)
    /// and how many days they usually take to answer the user (median), from past threads.
    pub fn sender_stats(&self, thread_id: &str) -> Result<(f64, Option<f64>)> {
        let r = self.bg.lock().unwrap();
        let mut st = r.prepare_cached(
            "WITH people AS (SELECT DISTINCT lower(from_email) e FROM messages WHERE thread_id = ?1 AND is_from_me = 0),
             peers AS (SELECT DISTINCT m.thread_id FROM people p JOIN messages m ON lower(m.from_email) = p.e
                       ORDER BY m.date DESC LIMIT ?2)
             SELECT m.thread_id, m.date, m.is_from_me, lower(m.from_email) IN (SELECT e FROM people)
             FROM messages m JOIN peers ON peers.thread_id = m.thread_id ORDER BY m.thread_id, m.date",
        )?;
        let rows: Vec<(String, i64, bool, bool)> = st
            .query_map(params![thread_id, PEER_THREADS], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)? != 0,
                    r.get::<_, i64>(3)? != 0,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let (mut threads, mut answered) = (0usize, 0usize);
        let mut gaps = vec![];
        let mut i = 0;
        while i < rows.len() {
            let tid = &rows[i].0;
            let end = rows[i..]
                .iter()
                .position(|x| &x.0 != tid)
                .map_or(rows.len(), |p| i + p);
            let msgs = &rows[i..end];
            if tid != thread_id {
                threads += 1;
                answered += msgs.iter().any(|m| m.2) as usize;
            }
            // Each of the user's messages followed by one of theirs is a reply delay.
            for (k, m) in msgs.iter().enumerate() {
                if m.2
                    && let Some(next) = msgs[k + 1..].iter().find(|n| !n.2)
                    && next.3
                {
                    gaps.push(next.1 - m.1);
                }
            }
            i = end;
        }
        let weight = if threads == 0 {
            0.0
        } else {
            answered as f64 / threads as f64
        };
        Ok((weight, median_days(gaps)))
    }

    /// Stores Jev's verdict on a thread (`judged`), or that a Backlog thread mentions no
    /// close date (`!judged`, rechecked tomorrow).
    pub fn save_judgment(
        &self,
        thread_id: &str,
        j: &Judgment,
        for_date: i64,
        judged: bool,
        today: &str,
    ) -> Result<()> {
        let w = self.w.lock().unwrap();
        w.execute(
            "INSERT INTO feed_judgments (thread_id, judged, kind, needs_reply, urgency, due, due_time, amount, automated,
               sender_weight, typical_reply_days, for_date, checked_day, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(thread_id) DO UPDATE SET judged = excluded.judged, kind = excluded.kind,
               needs_reply = excluded.needs_reply, urgency = excluded.urgency, due = excluded.due,
               due_time = excluded.due_time, amount = excluded.amount, automated = excluded.automated,
               sender_weight = excluded.sender_weight, typical_reply_days = excluded.typical_reply_days,
               for_date = excluded.for_date, checked_day = excluded.checked_day, created = excluded.created",
            params![
                thread_id,
                judged as i64,
                j.kind.map(kind_key),
                j.needs_reply,
                j.urgency,
                j.due,
                j.due_time,
                j.amount,
                j.automated,
                j.sender_weight,
                j.typical_reply_days,
                for_date,
                today,
                now_ms(),
            ],
        )?;
        Ok(())
    }

    /// The open work, as feed units: every thread in a feed status, plus Backlog threads
    /// whose date is close; snoozed threads stay out until their time or new mail.
    pub fn feed_units(
        &self,
        scope: &ColumnScope,
        today: chrono::NaiveDate,
        now: i64,
    ) -> Result<Vec<FeedUnit>> {
        let (scope_sql, scope_args) = scope.clause();
        let marks = vec!["?"; FEED_STATUSES.len()].join(",");
        let horizon = (today + chrono::Duration::days(feed::BACKLOG_HORIZON_DAYS)).to_string();
        let base = format!(
            "SELECT {THREAD_COLS}, {UNIT_COLS} FROM threads t
             LEFT JOIN feed_judgments j ON j.thread_id = t.id
             LEFT JOIN feed_texts x ON x.thread_id = t.id AND x.for_date >= t.last_date AND x.json IS NOT NULL
             LEFT JOIN feed_snoozes z ON z.thread_id = t.id"
        );
        let not_snoozed = "(z.thread_id IS NULL OR z.until <= ? OR z.for_date < t.last_date)";
        // Two queries, each on its own index: the open statuses, then the few Backlog
        // threads whose judged date is close (Backlog itself is never scanned).
        let open_sql = format!(
            "{base} WHERE t.status IN ({marks}) AND {not_snoozed}{scope_sql} ORDER BY t.last_date DESC"
        );
        let mut open_args: Vec<rusqlite::types::Value> =
            FEED_STATUSES.iter().map(|s| s.to_string().into()).collect();
        open_args.push(now.into());
        open_args.extend(scope_args.iter().cloned());
        // CROSS JOIN keeps the judgments (indexed by date) as the outer loop.
        let backlog_sql = format!(
            "SELECT {THREAD_COLS}, {UNIT_COLS} FROM feed_judgments j CROSS JOIN threads t ON t.id = j.thread_id
             LEFT JOIN feed_texts x ON x.thread_id = t.id AND x.for_date >= t.last_date AND x.json IS NOT NULL
             LEFT JOIN feed_snoozes z ON z.thread_id = t.id
             WHERE j.judged = 1 AND j.due BETWEEN ? AND ? AND t.status = 'backlog' AND j.for_date >= t.last_date
               AND {not_snoozed}{scope_sql}
             ORDER BY t.last_date DESC"
        );
        let mut backlog_args: Vec<rusqlite::types::Value> =
            vec![today.to_string().into(), horizon.into(), now.into()];
        backlog_args.extend(scope_args);
        let read_row = |row: &rusqlite::Row| -> rusqlite::Result<UnitRow> {
            let t = thread_row(row)?;
            // The feed's columns follow the thread's own, however many those are.
            let b = row.as_ref().column_count() - UNIT_COL_COUNT;
            let judged: Option<i64> = row.get(b)?;
            let j = if judged == Some(1) {
                Some(Judgment {
                    kind: kind_of(row.get(b + 1)?),
                    needs_reply: row.get(b + 2)?,
                    urgency: row.get(b + 3)?,
                    due: row.get(b + 4)?,
                    due_time: row.get(b + 5)?,
                    amount: row.get(b + 6)?,
                    automated: row.get(b + 7)?,
                    sender_weight: row.get::<_, Option<f64>>(b + 8)?.unwrap_or(0.0),
                    typical_reply_days: row.get(b + 9)?,
                })
            } else {
                None
            };
            let texts = row
                .get::<_, Option<String>>(b + 10)?
                .and_then(|s| serde_json::from_str(&s).ok());
            let my_last: Option<i64> = row.get(b + 11)?;
            let last_sender: Option<String> = row.get(b + 12)?;
            let until: Option<i64> = row.get(b + 13)?;
            let label: Option<String> = row.get(b + 14)?;
            let woke: Option<i64> = row.get(b + 15)?;
            // Back from Later: woken recently, and nothing happened since.
            let back = match (until, woke) {
                (Some(u), Some(1)) if now - u < BACK_FOR_MS => {
                    Some(label.unwrap_or_else(|| "this time".into()))
                }
                _ => None,
            };
            Ok((t, j, texts, my_last, last_sender.unwrap_or_default(), back))
        };
        let mut rows: Vec<UnitRow> = vec![];
        {
            let r = self.r.lock().unwrap();
            for (sql, args) in [(&open_sql, &open_args), (&backlog_sql, &backlog_args)] {
                let mut st = r.prepare_cached(sql)?;
                rows.extend(
                    st.query_map(params_from_iter(args.iter()), read_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?,
                );
            }
        }
        // Only mail that looks automated is checked against the user's learned rules.
        let automated: Vec<String> = rows
            .iter()
            .filter(|r| {
                is_automated(&r.4)
                    || r.1.as_ref().is_some_and(|j| {
                        j.automated.unwrap_or(0.0) >= 0.7
                            || matches!(j.kind, Some(Kind::Cleanup | Kind::Reading))
                    })
            })
            .map(|r| r.0.id.clone())
            .collect();
        let noise = self.learned_noise(&automated)?;
        Ok(rows
            .drain(..)
            .map(|(t, judgment, texts, my_last, sender, back)| {
                let waiting_days = (t.status == "waiting")
                    .then(|| (now - my_last.unwrap_or(t.last_date)).max(0) / DAY_MS);
                FeedUnit {
                    id: t.id.clone(),
                    from_backlog: t.status == "backlog",
                    automated_sender: is_automated(&sender),
                    learned_noise: noise.contains(&t.id),
                    back_from_later: back,
                    threads: vec![t],
                    judgment,
                    texts,
                    waiting_days,
                }
            })
            .collect())
    }

    /// Threads whose sender the user taught to archive: the Sort queue's learned rule
    /// for that sender (three consistent yes, never a no) is "Archive". No AI guess counts.
    pub fn learned_noise(
        &self,
        thread_ids: &[String],
    ) -> Result<std::collections::HashSet<String>> {
        let mut rules: std::collections::HashMap<String, bool> = Default::default();
        let mut out = std::collections::HashSet::new();
        for id in thread_ids {
            let sender = self.thread_sender(id)?;
            let noise = match rules.get(&sender) {
                Some(n) => *n,
                None => {
                    let n = self.sender_rule(&sender)?.is_some_and(|r| {
                        r.projects.is_empty()
                            && matches!(r.status.as_deref(), Some("archive" | "deleted"))
                    });
                    rules.insert(sender, n);
                    n
                }
            };
            if noise {
                out.insert(id.clone());
            }
        }
        Ok(out)
    }

    /// The user archived these automated mails from the Feed: each counts as a yes to
    /// "Archive" for its sender, the same answer the Sort queue records.
    pub fn learn_archive(&self, thread_ids: &[String]) -> Result<()> {
        let target = super::SuggestionGroup {
            status: Some("archive".into()),
            projects: vec![],
            count: 0,
        };
        let w = self.w.lock().unwrap();
        for id in thread_ids {
            let sender = super::thread_sender_tx(&w, id)?;
            super::record_feedback_tx(&w, id, &sender, &target, "yes")?;
        }
        Ok(())
    }

    /// Fills in projects and pending suggestions on the threads a feed shows; done after
    /// assembly so a feed of thousands of open threads only pays for its visible cards.
    pub fn complete_feed(&self, f: &mut feed::Feed) -> Result<()> {
        let mut threads: Vec<ThreadSummary> = f
            .sections
            .iter()
            .flat_map(|s| s.cards.iter())
            .flat_map(|c| std::iter::once(c.thread.clone()).chain(c.members.iter().cloned()))
            .collect();
        {
            let r = self.r.lock().unwrap();
            attach(&r, &mut threads)?;
        }
        let full: std::collections::HashMap<String, ThreadSummary> =
            threads.into_iter().map(|t| (t.id.clone(), t)).collect();
        for c in f.sections.iter_mut().flat_map(|s| s.cards.iter_mut()) {
            for t in std::iter::once(&mut c.thread).chain(c.members.iter_mut()) {
                if let Some(x) = full.get(&t.id) {
                    t.projects = x.projects.clone();
                    t.suggestion = x.suggestion.clone();
                }
            }
        }
        Ok(())
    }

    /// Threads among `ids` whose card words are missing or older than their latest
    /// message, with that message's date. A failed attempt is not retried until new mail.
    pub fn feed_texts_needed(&self, ids: &[String]) -> Result<Vec<(String, i64)>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let marks = vec!["?"; ids.len()].join(",");
        let r = self.r.lock().unwrap();
        let mut st = r.prepare(&format!(
            "SELECT t.id, t.last_date FROM threads t LEFT JOIN feed_texts x ON x.thread_id = t.id
             WHERE t.id IN ({marks}) AND (x.thread_id IS NULL OR x.for_date < t.last_date)"
        ))?;
        Ok(st
            .query_map(params_from_iter(ids.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Stores a card's words, or why the AI could not write them.
    pub fn save_feed_texts(
        &self,
        thread_id: &str,
        for_date: i64,
        provider_id: &str,
        outcome: &Result<Texts>,
    ) -> Result<()> {
        let (json, error) = match outcome {
            Ok(t) => (Some(serde_json::to_string(t)?), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let w = self.w.lock().unwrap();
        w.execute(
            "INSERT INTO feed_texts (thread_id, for_date, json, error, provider_id, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(thread_id) DO UPDATE SET for_date = excluded.for_date, json = excluded.json,
               error = excluded.error, provider_id = excluded.provider_id, created = excluded.created",
            params![thread_id, for_date, json, error, provider_id, now_ms()],
        )?;
        Ok(())
    }

    /// The kind a thread's card shows as, for the AI prompt.
    pub fn feed_kind(&self, thread_id: &str) -> Result<Kind> {
        let r = self.r.lock().unwrap();
        let row: Option<(Option<String>, String, Option<f64>)> = r
            .query_row(
                "SELECT j.kind, t.status, j.automated FROM threads t LEFT JOIN feed_judgments j ON j.thread_id = t.id AND j.judged = 1
                 WHERE t.id = ?1",
                [thread_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            Some((_, status, _)) if status == "waiting" => Kind::Waiting,
            Some((kind, ..)) => kind_of(kind).unwrap_or(Kind::Other),
            None => Kind::Other,
        })
    }

    /// Hides threads from the feed until `until`, or until new mail arrives in them.
    /// `label` is how the user put it ("Friday evening"), for the hint when it comes back.
    pub fn snooze(&self, ids: &[String], until: i64, label: Option<&str>) -> Result<usize> {
        let w = self.w.lock().unwrap();
        let mut n = 0;
        for id in ids {
            n += w.execute(
                "INSERT INTO feed_snoozes (thread_id, until, for_date, label, woke)
                 SELECT id, ?2, last_date, ?3, 0 FROM threads WHERE id = ?1
                 ON CONFLICT(thread_id) DO UPDATE SET until = excluded.until, for_date = excluded.for_date,
                   label = excluded.label, woke = 0",
                params![id, until, label],
            )?;
        }
        Ok(n)
    }

    /// Put-off mail whose time has come and that has not come back yet.
    pub fn due_snoozes(&self, now: i64) -> Result<Vec<String>> {
        let r = self.r.lock().unwrap();
        let mut st = r.prepare_cached(
            "SELECT z.thread_id FROM feed_snoozes z JOIN threads t ON t.id = z.thread_id
             WHERE z.woke = 0 AND z.until <= ?1",
        )?;
        Ok(st
            .query_map([now], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Brings put-off mail back: Unread again (it resumes its status once read), and
    /// shown in the feed's Now with what the user had picked. Closed mail stays closed.
    pub fn wake_snoozed(&self, thread_id: &str) -> Result<bool> {
        let status = self.thread_status(thread_id)?;
        let open = !matches!(status.as_str(), "done" | "archive" | "deleted");
        if open {
            self.mark_read(thread_id, false)?;
        }
        let w = self.w.lock().unwrap();
        w.execute(
            "UPDATE feed_snoozes SET woke = 1 WHERE thread_id = ?1",
            [thread_id],
        )?;
        Ok(open)
    }

    pub fn unsnooze(&self, ids: &[String]) -> Result<usize> {
        let w = self.w.lock().unwrap();
        let mut n = 0;
        for id in ids {
            n += w.execute("DELETE FROM feed_snoozes WHERE thread_id = ?1", [id])?;
        }
        Ok(n)
    }

    /// Accounts whose mail may go to an AI, for "the AI can write this card".
    pub fn ai_accounts(&self) -> Result<std::collections::HashSet<String>> {
        let mut out = std::collections::HashSet::new();
        for a in self.accounts()? {
            if self.provider_for_account(&a.id)?.is_some() {
                out.insert(a.id);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
