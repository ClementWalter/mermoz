//! The write-back queue pushed to Gmail: label, read and trash changes that come to the
//! same edit are sent together, up to 1,000 messages per `messages.batchModify` call, so
//! a bulk run of 25,000 threads reaches Gmail in a few dozen calls. Trash is the `TRASH`
//! system label (Gmail lists it as one a client may apply), so a delete and its undo
//! batch like an archive; nothing here ever deletes a message for good. The queue is
//! read a chunk at a time, the user's own edits first, so they never wait behind a bulk
//! run.

use super::AccountSync;
use crate::error::{Error, Result};
use crate::gmail::BATCH_MODIFY_MAX;
use crate::store::{LABEL_PREFIX, Op};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use tracing::warn;

/// Ops read per chunk; the queue is read again after each, so an edit made during a
/// bulk push goes out with the next chunk.
const PUSH_CHUNK: i64 = 1000;

/// Label names a group of ops adds and removes.
type Delta = (BTreeSet<String>, BTreeSet<String>);

/// What one op needs from Gmail.
enum Plan {
    /// Nothing: Gmail already matches.
    Done,
    /// A label change shared with other ops, sent in batches.
    Batch(Delta),
    /// Its own calls (label renames and deletes).
    Single,
}

/// How a chunk went: ops pushed, ops that failed, and whether Gmail is unreachable.
#[derive(Default)]
struct Outcome {
    pushed: usize,
    failed: Vec<i64>,
    offline: bool,
}

impl AccountSync {
    /// Pushes queued local edits to Gmail until the queue holds only failures; those
    /// stay queued for the next round.
    pub async fn push_ops(&self) -> Result<usize> {
        let mut pushed = 0;
        let mut failed: HashSet<i64> = HashSet::new();
        loop {
            let ops: Vec<Op> = self
                .store
                .pending_ops_for(&self.account_id, PUSH_CHUNK + failed.len() as i64)?
                .into_iter()
                .filter(|o| !failed.contains(&o.id))
                .collect();
            if ops.is_empty() {
                break;
            }
            let o = self.push_chunk(&ops).await?;
            pushed += o.pushed;
            failed.extend(o.failed);
            if o.offline {
                break;
            }
        }
        Ok(pushed)
    }

    async fn push_chunk(&self, ops: &[Op]) -> Result<Outcome> {
        let mut out = Outcome::default();
        let mut batched: Vec<(bool, Delta, &Op)> = vec![];
        let mut done: Vec<&Op> = vec![];
        let mut singles: Vec<&Op> = vec![];
        for op in ops {
            match self.plan(op) {
                Ok(Plan::Done) => done.push(op),
                Ok(Plan::Batch(d)) => batched.push((op.bulk, d, op)),
                Ok(Plan::Single) => singles.push(op),
                Err(Error::NotFound(_)) => done.push(op),
                Err(e) => self.failed(op, &e, &mut out)?,
            }
        }
        self.store.ops_done(&done)?;
        out.pushed += done.len();
        for ((add, remove), group) in group(batched) {
            if out.offline {
                break;
            }
            self.push_group(&add, &remove, &group, &mut out).await?;
        }
        for op in singles {
            if out.offline {
                break;
            }
            match self.push_one(op).await {
                Ok(()) | Err(Error::NotFound(_)) => {
                    self.store.op_done(op)?;
                    out.pushed += 1;
                }
                Err(e) => self.failed(op, &e, &mut out)?,
            }
        }
        Ok(out)
    }

    fn failed(&self, op: &Op, e: &Error, out: &mut Outcome) -> Result<()> {
        warn!(op = op.id, "push failed: {e}");
        self.store.op_failed(op, &e.to_string())?;
        out.failed.push(op.id);
        // Offline: stop this round, everything stays queued.
        out.offline |= matches!(e, Error::Http(_));
        Ok(())
    }

    /// What an op asks of Gmail, from the thread's state now.
    fn plan(&self, op: &Op) -> Result<Plan> {
        let names = |k: &str| -> BTreeSet<String> {
            op.payload[k]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        match op.kind.as_str() {
            "labels" => self.labels_plan(&op.key),
            "projects" => {
                self.store.provider_thread_id(&op.key)?;
                Ok(Plan::Batch((names("add"), names("remove"))))
            }
            "read" => {
                self.store.provider_thread_id(&op.key)?;
                let unread = BTreeSet::from(["UNREAD".to_string()]);
                Ok(if op.payload["read"].as_bool().unwrap_or(true) {
                    Plan::Batch((BTreeSet::new(), unread))
                } else {
                    Plan::Batch((unread, BTreeSet::new()))
                })
            }
            _ => Ok(Plan::Single),
        }
    }

    /// Managed labels (TRASH, INBOX, Mermoz/*) to add and remove so Gmail shows the
    /// thread's status.
    fn labels_plan(&self, thread_id: &str) -> Result<Plan> {
        self.store.provider_thread_id(thread_id)?;
        let trashed = self.store.thread_trashed(&self.account_id, thread_id)?;
        let deleted = self.store.thread_status(thread_id)? == "deleted";
        let (desired, current) = if deleted {
            (BTreeSet::new(), BTreeSet::new())
        } else {
            (
                self.store.thread_desired_labels(thread_id)?,
                self.store
                    .thread_current_labels(&self.account_id, thread_id)?,
            )
        };
        Ok(match label_delta(deleted, trashed, &desired, &current) {
            Some(d) => Plan::Batch(d),
            None => Plan::Done,
        })
    }

    /// One label change for many threads: their messages in calls of up to 1,000.
    async fn push_group(
        &self,
        add: &BTreeSet<String>,
        remove: &BTreeSet<String>,
        ops: &[&Op],
        out: &mut Outcome,
    ) -> Result<()> {
        let add_ids = match self.label_ids(add).await {
            Ok(ids) => ids,
            Err(e) => {
                for op in ops {
                    self.failed(op, &e, out)?;
                }
                return Ok(());
            }
        };
        // A label that no longer exists is already off every message.
        let mut remove_ids = vec![];
        for n in remove {
            if SYSTEM_LABELS.contains(&n.as_str()) {
                remove_ids.push(n.clone());
            } else if let Some(id) = self.store.label_id(&self.account_id, n)? {
                remove_ids.push(id);
            }
        }
        if add_ids.is_empty() && remove_ids.is_empty() {
            self.store.ops_done(ops)?;
            out.pushed += ops.len();
            return Ok(());
        }
        let threads: Vec<String> = ops.iter().map(|o| o.key.clone()).collect();
        let messages = self.store.provider_message_ids(&threads)?;
        let sizes: Vec<usize> = ops.iter().map(|o| messages[&o.key].len()).collect();
        let (calls, alone) = pack(&sizes, BATCH_MODIFY_MAX);
        for call in calls {
            if out.offline {
                return Ok(());
            }
            let batch: Vec<&Op> = call.iter().map(|&i| ops[i]).collect();
            let ids: Vec<String> = batch
                .iter()
                .flat_map(|o| messages[&o.key].iter().cloned())
                .collect();
            self.send_batch(&batch, &ids, &add_ids, &remove_ids, out)
                .await?;
        }
        for i in alone {
            if out.offline {
                break;
            }
            self.send_thread(ops[i], &add_ids, &remove_ids, out).await?;
        }
        Ok(())
    }

    async fn send_batch(
        &self,
        ops: &[&Op],
        ids: &[String],
        add: &[String],
        remove: &[String],
        out: &mut Outcome,
    ) -> Result<()> {
        match self.gmail()?.batch_modify(ids, add, remove).await {
            Ok(()) => {
                let threads: Vec<String> = ops.iter().map(|o| o.key.clone()).collect();
                self.store.apply_label_delta_many(&threads, add, remove)?;
                self.store.ops_done(ops)?;
                out.pushed += ops.len();
            }
            Err(e @ Error::Http(_)) => {
                for op in ops {
                    self.failed(op, &e, out)?;
                }
            }
            // One bad id fails the whole call: each thread on its own instead.
            Err(e) => {
                warn!(
                    threads = ops.len(),
                    "batch label change refused, sending per thread: {e}"
                );
                for op in ops {
                    if out.offline {
                        break;
                    }
                    self.send_thread(op, add, remove, out).await?;
                }
            }
        }
        Ok(())
    }

    async fn send_thread(
        &self,
        op: &Op,
        add: &[String],
        remove: &[String],
        out: &mut Outcome,
    ) -> Result<()> {
        let res = match self.store.provider_thread_id(&op.key) {
            Ok((_, pt)) => self.modify_thread(&pt, add, remove).await,
            Err(e) => Err(e),
        };
        match res {
            Ok(()) => {
                self.store.apply_label_delta(&op.key, add, remove)?;
                self.store.op_done(op)?;
                out.pushed += 1;
            }
            Err(Error::NotFound(_)) | Err(Error::Upstream { status: 404, .. }) => {
                self.store.op_done(op)?;
                out.pushed += 1;
            }
            Err(e) => self.failed(op, &e, out)?,
        }
        Ok(())
    }

    /// One thread's label change; moving in or out of Trash uses Gmail's own
    /// `threads.trash` / `threads.untrash`, the rest `threads.modify`.
    async fn modify_thread(&self, pt: &str, add: &[String], remove: &[String]) -> Result<()> {
        let (mut add, mut remove) = (add.to_vec(), remove.to_vec());
        if add.iter().any(|l| l == TRASH) {
            self.gmail()?.trash_thread(pt).await?;
            // threads.trash takes the thread out of the inbox as well.
            add.retain(|l| l != TRASH);
            remove.retain(|l| l != TRASH && l != "INBOX");
        } else if remove.iter().any(|l| l == TRASH) {
            self.gmail()?.untrash_thread(pt).await?;
            remove.retain(|l| l != TRASH);
        }
        if add.is_empty() && remove.is_empty() {
            return Ok(());
        }
        self.gmail()?.modify_thread(pt, &add, &remove).await
    }

    /// An op with calls of its own.
    async fn push_one(&self, op: &Op) -> Result<()> {
        match op.kind.as_str() {
            "label_rename" => self.push_label_rename(&op.payload).await,
            "label_delete" => self.push_label_delete(&op.payload).await,
            other => Err(Error::other(format!("unknown op {other}"))),
        }
    }
}

const TRASH: &str = "TRASH";

/// Gmail system labels sent by id, which is their name.
pub(super) const SYSTEM_LABELS: &[&str] = &["INBOX", "UNREAD", TRASH];

/// The change that makes Gmail show a thread as it is here: `deleted` here, `trashed`
/// in Gmail, and its managed labels (INBOX, Mermoz/*) `desired` and `current`. A
/// deleted thread goes to Trash and leaves the inbox, as Gmail's own trash does; any
/// other thread leaves Trash. `None` when Gmail already matches.
fn label_delta(
    deleted: bool,
    trashed: bool,
    desired: &BTreeSet<String>,
    current: &BTreeSet<String>,
) -> Option<Delta> {
    if deleted {
        return (!trashed).then(|| {
            (
                BTreeSet::from([TRASH.to_string()]),
                BTreeSet::from(["INBOX".to_string()]),
            )
        });
    }
    let add: BTreeSet<String> = desired.difference(current).cloned().collect();
    let mut remove: BTreeSet<String> = current
        .iter()
        .filter(|n| *n == "INBOX" || n.starts_with(LABEL_PREFIX))
        .filter(|n| !desired.contains(*n))
        .cloned()
        .collect();
    if trashed {
        remove.insert(TRASH.to_string());
    }
    (!add.is_empty() || !remove.is_empty()).then_some((add, remove))
}

/// Groups ops by (bulk, change), in the order of each group's first op. The user's
/// edits come first in the queue, so their calls go out first and carry no bulk ops.
fn group<T>(planned: Vec<(bool, Delta, T)>) -> Vec<(Delta, Vec<T>)> {
    let mut groups: Vec<(Delta, Vec<T>)> = vec![];
    let mut index: BTreeMap<(bool, Delta), usize> = BTreeMap::new();
    for (bulk, d, op) in planned {
        let i = *index.entry((bulk, d.clone())).or_insert_with(|| {
            groups.push((d, vec![]));
            groups.len() - 1
        });
        groups[i].1.push(op);
    }
    groups
}

/// Packs threads, by message count in queue order, into calls of at most `max`
/// messages. Whole threads per call, so each op is done or not as one. Threads that fit
/// no call (no stored message, or more than `max`) come back apart, by index.
fn pack(sizes: &[usize], max: usize) -> (Vec<Vec<usize>>, Vec<usize>) {
    let (mut calls, mut alone): (Vec<Vec<usize>>, Vec<usize>) = (vec![], vec![]);
    let (mut call, mut n) = (vec![], 0);
    for (i, &size) in sizes.iter().enumerate() {
        if size == 0 || size > max {
            alone.push(i);
            continue;
        }
        if n + size > max {
            calls.push(std::mem::take(&mut call));
            n = 0;
        }
        call.push(i);
        n += size;
    }
    if !call.is_empty() {
        calls.push(call);
    }
    (calls, alone)
}

#[cfg(test)]
mod tests;
