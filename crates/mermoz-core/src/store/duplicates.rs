//! The same email received twice (a forwarding address delivering into Gmail, a double
//! import) is two Gmail messages with one content. Later copies point at the kept one
//! with `duplicate_of`; a thread shows the kept one with `copies`, and counts it once.
//! Gmail keeps every copy: labels, archive and delete still apply to all of them, and
//! no copy is ever deleted on its own.
//!
//! Copies share the sender, the minute (±2 min), the subject (Re:/Fwd: aside), the
//! recipients and the text (whitespace aside). The kept copy carries the sender's own
//! Message-ID rather than one Google made up on receipt (`…SMTPIN_ADDED@mx.google.com`).

use crate::error::Result;
use rusqlite::{Connection, params};
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

pub(super) const MIGRATION: &str = "ALTER TABLE messages ADD COLUMN duplicate_of TEXT;";

/// Whether a message has HTML and how many parts it carries (`m` is the message row).
const PARTS: &str =
    "(m.body_html IS NOT NULL OR EXISTS (SELECT 1 FROM message_html h WHERE h.message_id = m.id)),
   (SELECT COUNT(*) FROM attachments a WHERE a.message_id = m.id)";

/// How far apart two deliveries of one email can be stamped.
const WINDOW_MS: i64 = 2 * 60 * 1000;

/// A subject without its reply and forward prefixes, lowercased.
fn subject_key(subject: &str) -> String {
    crate::conversation::bare_subject(subject).to_lowercase()
}

/// Everyone a message went to, as a sorted set of addresses.
fn recipients_key(to_json: &str, cc_json: &str) -> BTreeSet<String> {
    let parse =
        |j: &str| -> Vec<crate::model::Address> { serde_json::from_str(j).unwrap_or_default() };
    parse(to_json)
        .into_iter()
        .chain(parse(cc_json))
        .map(|a| a.email.to_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
}

/// The text with its whitespace and line endings folded.
fn body_key(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for w in text.split_whitespace() {
        w.hash(&mut h);
    }
    h.finish()
}

/// A Message-ID Google wrote on receipt instead of the sender's own.
fn made_up_by_google(message_id: &str) -> bool {
    message_id.contains("SMTPIN_ADDED")
}

/// One message as compared.
struct Copy {
    id: String,
    rowid: i64,
    message_id: String,
    subject: String,
    recipients: BTreeSet<String>,
    body: u64,
    /// (has HTML, attachment count): a second delivery keeps the email's parts.
    parts: (bool, i64),
}

/// The duplicate group of `message_id` (itself included) and its `duplicate_of` for
/// each member: None for the kept one. A message alone gets `[(id, None)]`.
pub(super) fn regroup(
    conn: &Connection,
    message_id: &str,
) -> Result<Vec<(String, Option<String>)>> {
    let row = |r: &rusqlite::Row| -> rusqlite::Result<Copy> {
        let subject: String = r.get(3)?;
        let (to, cc, body): (String, String, String) = (r.get(4)?, r.get(5)?, r.get(6)?);
        Ok(Copy {
            id: r.get(0)?,
            rowid: r.get(1)?,
            message_id: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            subject: subject_key(&subject),
            recipients: recipients_key(&to, &cc),
            body: body_key(&body),
            parts: (r.get::<_, i64>(7)? != 0, r.get(8)?),
        })
    };
    let me: Option<(Copy, String, String, i64)> = conn
        .prepare_cached(&format!(
            "SELECT id, rowid, message_id_header, subject, to_json, cc_json, body_text, {PARTS},
               thread_id, from_email, date FROM messages m WHERE id = ?1"
        ))?
        .query_map([message_id], |r| {
            Ok((row(r)?, r.get(9)?, r.get(10)?, r.get(11)?))
        })?
        .next()
        .transpose()?;
    let Some((me, thread, from, date)) = me else {
        return Ok(vec![]);
    };
    // Candidates: same thread, same sender, close in time (the thread index serves it).
    let candidates: Vec<Copy> = conn
        .prepare_cached(&format!(
            "SELECT id, rowid, message_id_header, subject, to_json, cc_json, body_text, {PARTS}
             FROM messages m WHERE thread_id = ?1 AND date BETWEEN ?2 AND ?3
               AND lower(from_email) = lower(?4)"
        ))?
        .query_map(
            params![thread, date - WINDOW_MS, date + WINDOW_MS, from],
            row,
        )?
        .collect::<rusqlite::Result<_>>()?;
    let mut group: Vec<&Copy> = candidates
        .iter()
        .filter(|c| {
            c.subject == me.subject
                && c.recipients == me.recipients
                && c.body == me.body
                && c.parts == me.parts
        })
        .collect();
    if group.len() < 2 {
        return Ok(vec![(me.id, None)]);
    }
    group.sort_by_key(|c| (made_up_by_google(&c.message_id), c.rowid));
    let kept = group[0].id.clone();
    Ok(group
        .iter()
        .map(|c| (c.id.clone(), (c.id != kept).then(|| kept.clone())))
        .collect())
}

/// Writes a group's `duplicate_of` values.
pub(super) fn apply(conn: &Connection, updates: &[(String, Option<String>)]) -> Result<()> {
    for (id, of) in updates {
        conn.prepare_cached(
            "UPDATE messages SET duplicate_of = ?2 WHERE id = ?1 AND duplicate_of IS NOT ?2",
        )?
        .execute(params![id, of])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::ParsedMessage;
    use crate::model::Address;
    use crate::store::{IncomingMessage, Store, SyncMode};
    use rstest::*;

    const T0: i64 = 1_333_000_000_000;
    const TEXT: &str = "Joyeuses Pâques à toute la famille !\r\n\r\nBises,\r\nNina";

    fn addr(email: &str) -> Address {
        Address {
            name: String::new(),
            email: email.into(),
        }
    }

    /// One delivery of a message: provider id, Message-ID, date, text, recipient.
    fn delivery(
        pid: &str,
        mid: &str,
        date: i64,
        text: &str,
        to: &str,
        subject: &str,
    ) -> IncomingMessage {
        IncomingMessage {
            provider_id: pid.into(),
            provider_thread_id: "t1".into(),
            label_ids: vec!["INBOX".into(), "Label_17".into()],
            internal_date: date,
            parsed: ParsedMessage {
                from: Some(Address {
                    name: "Nina Roy".into(),
                    email: "Nina.Roy@mail.example".into(),
                }),
                to: vec![addr(to)],
                subject: subject.into(),
                body_text: text.into(),
                message_id: Some(mid.into()),
                ..Default::default()
            },
        }
    }

    fn store_with(msgs: &[IncomingMessage]) -> (tempfile::TempDir, Store, String) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        let acc = store.add_account("teo@mail.example", "gmail").unwrap().id;
        let thread = store
            .ingest_many(&acc, "teo@mail.example", msgs, SyncMode::Initial)
            .unwrap()
            .remove(0);
        (dir, store, thread)
    }

    /// The forwarded copy (Google's Message-ID) arrives first, the original second.
    fn twice() -> Vec<IncomingMessage> {
        vec![
            delivery(
                "g1",
                "CAF1.SMTPIN_ADDED@mx.google.com",
                T0,
                TEXT,
                "teo@school.example",
                "Joyeuses Pâques",
            ),
            delivery(
                "g2",
                "1333000.42@mail.example",
                T0 + 30_000,
                &TEXT.replace("\r\n", "\n"),
                "teo@school.example",
                "Joyeuses Pâques",
            ),
        ]
    }

    #[rstest]
    fn an_exact_copy_shows_once() {
        let (_d, store, t) = store_with(&twice());
        assert_eq!(store.messages(&t).unwrap().len(), 1);
    }

    #[rstest]
    fn the_kept_copy_says_it_came_twice() {
        let (_d, store, t) = store_with(&twice());
        assert_eq!(store.messages(&t).unwrap()[0].copies, 2);
    }

    #[rstest]
    fn the_copy_with_the_senders_own_message_id_is_kept() {
        let (_d, store, t) = store_with(&twice());
        let kept = &store.messages(&t).unwrap()[0];
        let header: String = store
            .w
            .lock()
            .unwrap()
            .query_row(
                "SELECT message_id_header FROM messages WHERE id = ?1",
                [&kept.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(header, "1333000.42@mail.example");
    }

    #[rstest]
    fn the_thread_counts_the_email_once() {
        let (_d, store, t) = store_with(&twice());
        assert_eq!(store.thread(&t).unwrap().message_count, 1);
    }

    #[rstest]
    fn gmail_keeps_both_copies() {
        let (_d, store, _t) = store_with(&twice());
        let n: i64 = store
            .w
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[rstest]
    #[case::different_text(delivery("g2", "b@mail.example", T0 + 30_000, "Joyeuses Pâques à tous !", "teo@school.example", "Joyeuses Pâques"))]
    #[case::sent_to_someone_else(delivery("g2", "b@mail.example", T0 + 30_000, TEXT, "ana@mail.example", "Joyeuses Pâques"))]
    #[case::hours_later(delivery("g2", "b@mail.example", T0 + 3 * 3_600_000, TEXT, "teo@school.example", "Joyeuses Pâques"))]
    #[case::other_subject(delivery("g2", "b@mail.example", T0 + 30_000, TEXT, "teo@school.example", "Pâques 2"))]
    fn a_similar_message_is_not_a_copy(#[case] other: IncomingMessage) {
        let first = delivery(
            "g1",
            "a@mail.example",
            T0,
            TEXT,
            "teo@school.example",
            "Joyeuses Pâques",
        );
        let (_d, store, t) = store_with(&[first, other]);
        assert_eq!(store.messages(&t).unwrap().len(), 2);
    }

    #[rstest]
    fn a_forwarded_delivery_with_a_forward_prefix_is_a_copy() {
        let first = delivery(
            "g1",
            "a@mail.example",
            T0,
            TEXT,
            "teo@school.example",
            "Joyeuses Pâques",
        );
        let fwd = delivery(
            "g2",
            "F9.SMTPIN_ADDED@mx.google.com",
            T0 + 60_000,
            TEXT,
            "teo@school.example",
            "Fwd: Joyeuses Pâques",
        );
        let (_d, store, t) = store_with(&[first, fwd]);
        assert_eq!(store.messages(&t).unwrap().len(), 1);
    }

    #[rstest]
    fn the_background_pass_flags_older_copies() {
        let (_d, store, t) = store_with(&twice());
        store
            .w
            .lock()
            .unwrap()
            .execute_batch("UPDATE messages SET duplicate_of = NULL; DELETE FROM message_reading; DELETE FROM meta WHERE key = 'reading_version'")
            .unwrap();
        while store.read_bodies_batch(0, 10).unwrap().is_some() {}
        assert_eq!(store.messages(&t).unwrap().len(), 1);
    }

    #[rstest]
    #[case::reply("Re: Joyeuses Pâques", "joyeuses pâques")]
    #[case::stacked("RE: TR: Fwd: Joyeuses Pâques", "joyeuses pâques")]
    #[case::plain("Joyeuses Pâques", "joyeuses pâques")]
    fn subjects_compare_without_prefixes(#[case] subject: &str, #[case] want: &str) {
        assert_eq!(subject_key(subject), want);
    }
}
