//! What the account knows of each sending organisation: the first time mail came from
//! its registered domain, whether the user ever wrote to it, and the web research on
//! who a sender's name claims to be. A domain never seen before vouches for nobody.

use super::Store;
use crate::error::Result;
use crate::model::{Address, SenderClaim};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

pub(super) const MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS sender_domains (
  domain TEXT PRIMARY KEY, first_received INTEGER, written INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS sender_claims (
  name TEXT NOT NULL, domain TEXT NOT NULL, claim_json TEXT NOT NULL, checked INTEGER NOT NULL,
  PRIMARY KEY (name, domain)) WITHOUT ROWID;
"#;

/// Web research on senders per day, at most.
pub const DAILY_SENDER_CHECKS: i64 = 100;

/// Notes mail received from `email` at `date`.
pub(super) fn note_received(tx: &Connection, email: &str, date: i64) -> Result<()> {
    let domain = crate::auth::sender_domain(email);
    if domain.is_empty() {
        return Ok(());
    }
    tx.prepare_cached(
        "INSERT INTO sender_domains (domain, first_received) VALUES (?1, ?2)
         ON CONFLICT(domain) DO UPDATE SET first_received =
           min(coalesce(first_received, excluded.first_received), excluded.first_received)",
    )?
    .execute(params![domain, date])?;
    Ok(())
}

/// Notes that the user wrote to these addresses.
pub(super) fn note_written<'a>(
    tx: &Connection,
    addrs: impl Iterator<Item = &'a Address>,
) -> Result<()> {
    let mut st = tx.prepare_cached(
        "INSERT INTO sender_domains (domain, written) VALUES (?1, 1)
         ON CONFLICT(domain) DO UPDATE SET written = 1",
    )?;
    for a in addrs {
        let domain = crate::auth::sender_domain(&a.email);
        if !domain.is_empty() {
            st.execute([domain])?;
        }
    }
    Ok(())
}

/// Whether nothing came from `email`'s organisation before `date` and the user never
/// wrote to it.
pub(super) fn is_new(conn: &Connection, email: &str, date: i64) -> Result<bool> {
    let known: Option<(Option<i64>, bool)> = conn
        .prepare_cached("SELECT first_received, written FROM sender_domains WHERE domain = ?1")?
        .query_row([crate::auth::sender_domain(email)], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? != 0))
        })
        .optional()?;
    Ok(match known {
        Some((_, true)) => false,
        Some((Some(first), false)) => first >= date,
        _ => true,
    })
}

fn claim_key(name: &str, email: &str) -> (String, String) {
    (
        name.trim().to_lowercase(),
        crate::auth::sender_domain(email),
    )
}

/// The research stored for this sender, if any.
pub(super) fn claim(conn: &Connection, from: &Address) -> Result<Option<SenderClaim>> {
    let (name, domain) = claim_key(&from.name, &from.email);
    let json: Option<String> = conn
        .prepare_cached("SELECT claim_json FROM sender_claims WHERE name = ?1 AND domain = ?2")?
        .query_row([name, domain], |r| r.get(0))
        .optional()?;
    Ok(json
        .and_then(|j| serde_json::from_str::<SenderClaim>(&j).ok())
        .map(|mut c| {
            c.matches = crate::auth::sends_from(&c.domains, &from.email);
            c
        }))
}

impl Store {
    /// Keeps the web research on a sender (by name and organisation domain).
    pub fn save_sender_claim(&self, from: &Address, claim: &SenderClaim) -> Result<()> {
        let (name, domain) = claim_key(&from.name, &from.email);
        self.w.lock().unwrap().execute(
            "INSERT INTO sender_claims (name, domain, claim_json, checked) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name, domain) DO UPDATE SET claim_json = excluded.claim_json, checked = excluded.checked",
            params![name, domain, serde_json::to_string(claim)?, crate::model::now_ms()],
        )?;
        Ok(())
    }

    /// Whether this sender was researched already.
    pub fn has_sender_claim(&self, from: &Address) -> Result<bool> {
        let (name, domain) = claim_key(&from.name, &from.email);
        Ok(self
            .r
            .lock()
            .unwrap()
            .prepare_cached("SELECT 1 FROM sender_claims WHERE name = ?1 AND domain = ?2")?
            .exists([name, domain])?)
    }

    /// Takes one of today's sender checks; `false` once the daily allowance is used.
    pub fn take_sender_check(&self) -> Result<bool> {
        let key = format!("sender_checks:{}", chrono::Utc::now().format("%Y-%m-%d"));
        let w = self.w.lock().unwrap();
        let used: i64 = w
            .query_row("SELECT value FROM settings WHERE key = ?1", [&key], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if used >= DAILY_SENDER_CHECKS {
            return Ok(false);
        }
        w.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, (used + 1).to_string()],
        )?;
        Ok(true)
    }

    /// Fills `sender_domains` from the mail stored before it existed.
    pub(super) fn backfill_sender_domains(&self) -> Result<()> {
        const VERSION: &str = "1";
        let mut w = self.w.lock().unwrap();
        let done: Option<String> = w
            .query_row(
                "SELECT value FROM meta WHERE key = 'sender_domains_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if done.as_deref() == Some(VERSION) {
            return Ok(());
        }
        let tx = w.transaction()?;
        let mut first: HashMap<String, i64> = HashMap::new();
        {
            let mut st = tx.prepare(
                "SELECT lower(from_email), min(date) FROM messages WHERE is_from_me = 0 GROUP BY 1",
            )?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (email, date) = row?;
                let d = crate::auth::sender_domain(&email);
                if !d.is_empty() {
                    let e = first.entry(d).or_insert(date);
                    *e = (*e).min(date);
                }
            }
        }
        for (domain, date) in first {
            tx.execute(
                "INSERT INTO sender_domains (domain, first_received) VALUES (?1, ?2)
                 ON CONFLICT(domain) DO UPDATE SET first_received =
                   min(coalesce(first_received, excluded.first_received), excluded.first_received)",
                params![domain, date],
            )?;
        }
        let written: Vec<String> = tx
            .prepare("SELECT email FROM correspondents")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let addrs: Vec<Address> = written
            .into_iter()
            .map(|email| Address {
                name: String::new(),
                email,
            })
            .collect();
        note_written(&tx, addrs.iter())?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('sender_domains_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [VERSION],
        )?;
        tx.commit()?;
        Ok(())
    }
}
