//! Built-in agent: reads a message through the provider allowed for its account and
//! returns a clean reading version plus the sender's contact card from the signature.
//! Anthropic (Messages API) and OpenAI-compatible endpoints are both supported.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// "anthropic" or "openai" (any OpenAI-compatible endpoint).
    pub kind: String,
    pub base_url: Option<String>,
    pub model: String,
    /// "key" (pasted API key) or "oauth" (signed in through Anthropic's login).
    #[serde(default = "default_auth")]
    pub auth: String,
    /// Login profile for "oauth" providers.
    #[serde(default)]
    pub profile: Option<String>,
}

fn default_auth() -> String {
    "key".into()
}

/// Contact details found in a signature; every field optional, the model leaves unknowns empty.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Card {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub phones: Vec<String>,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub website: String,
    #[serde(default)]
    pub socials: Vec<String>,
}

impl Card {
    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
            && self.company.is_empty()
            && self.phones.is_empty()
            && self.address.is_empty()
            && self.website.is_empty()
            && self.socials.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Digest {
    pub clean_body: String,
    pub card: Option<Card>,
}

/// Bodies above this are left alone rather than truncated (newsletters, logs).
pub const MAX_INPUT_CHARS: usize = 40_000;

const SYSTEM: &str = "You prepare emails for reading in a mail client.\n\n\
Given one email, return:\n\
1. clean_body: the message as its author meant it, in its original language, with every word of substance kept verbatim. \
A forwarded email is content, not quoted history: keep it. Remove: quoted earlier messages of a reply, signatures, legal disclaimers, confidentiality notices, \"sent from my phone\" footers, unsubscribe and tracking boilerplate, \
and leftover formatting debris (stray symbols, repeated blank lines, broken line wraps). Keep greetings and sign-off names. \
Format it with light Markdown so it reads well: paragraphs separated by one blank line, lists as \"- \" lines, \
**bold** only for headings or labels the author used, links as [label](url) or the bare URL. Never summarize, translate, rephrase or add anything.\n\
2. card: contact details for the SENDER found in their signature (name, job title, company, phone numbers, postal address, website, social profile URLs). \
Only include what is written in this email; leave a field empty when absent. Return null when the email has no signature details.";

fn schema() -> Value {
    let s = json!({ "type": "string" });
    let arr = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["clean_body", "card"],
        "properties": {
            "clean_body": s,
            "card": { "anyOf": [ { "type": "null" }, {
                "type": "object",
                "additionalProperties": false,
                "required": ["name", "title", "company", "phones", "address", "website", "socials"],
                "properties": { "name": s, "title": s, "company": s, "phones": arr, "address": s, "website": s, "socials": arr }
            } ] }
        }
    })
}

fn user_prompt(from: &str, subject: &str, body: &str) -> String {
    // A forward's header block looks like a quoted reply; say which it is.
    let note = if crate::quote::is_forward(subject) {
        "\nThis email FORWARDS another email: the forwarded email is the content. Keep it in full \
         (after the sender's own note), introduced by one line \"Forwarded from <sender>, <date>:\".\n"
    } else {
        ""
    };
    format!("From: {from}\nSubject: {subject}\n{note}\n<email>\n{body}\n</email>")
}

pub struct Client {
    http: reqwest::Client,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .expect("http client");
        Self { http }
    }

    /// One structured call on any provider kind; returns the raw JSON text.
    pub async fn complete(
        &self,
        p: &Provider,
        key: &str,
        system: &str,
        user: &str,
        schema: &Value,
    ) -> Result<String> {
        match p.kind.as_str() {
            "anthropic" => self.anthropic(p, key, system, user, schema).await,
            "chatgpt" => crate::chatgpt::respond(key, &p.model, system, user, schema).await,
            "openai" => self.openai(p, key, system, user, schema).await,
            other => Err(Error::InvalidParams(format!(
                "unknown provider kind '{other}'"
            ))),
        }
    }

    /// Groups and renames a batch of directory entries.
    pub async fn people_groups(
        &self,
        p: &Provider,
        key: &str,
        entries: &[PersonEntry],
    ) -> Result<Vec<PersonGroup>> {
        let schema = json!({
            "type": "object", "additionalProperties": false, "required": ["groups"],
            "properties": { "groups": { "type": "array", "items": {
                "type": "object", "additionalProperties": false, "required": ["ids", "name", "kind"],
                "properties": {
                    "ids": { "type": "array", "items": { "type": "string" } },
                    "name": { "type": "string" },
                    "kind": { "type": "string", "enum": ["person", "organization", "list"] }
                } } } }
        });
        let user = serde_json::to_string(&json!({ "entries": entries }))?;
        let raw = self.complete(p, key, PEOPLE_SYSTEM, &user, &schema).await?;
        let v: Value = serde_json::from_str(&raw)
            .map_err(|_| Error::other("provider returned malformed JSON"))?;
        Ok(serde_json::from_value(v["groups"].clone()).unwrap_or_default())
    }

    /// One call per message; the stable system prompt is cached across calls.
    pub async fn digest(
        &self,
        p: &Provider,
        api_key: &str,
        from: &str,
        subject: &str,
        body: &str,
    ) -> Result<Digest> {
        if body.chars().count() > MAX_INPUT_CHARS {
            return Err(Error::InvalidParams("message too long to clean".into()));
        }
        let raw = self
            .complete(
                p,
                api_key,
                SYSTEM,
                &user_prompt(from, subject, body),
                &schema(),
            )
            .await?;
        let mut d: Digest = serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| {
                Some(Digest {
                    clean_body: v.get("clean_body")?.as_str()?.to_string(),
                    card: v
                        .get("card")
                        .and_then(|c| serde_json::from_value(c.clone()).ok()),
                })
            })
            .ok_or_else(|| Error::other("provider returned malformed JSON"))?;
        if d.card.as_ref().is_some_and(|c| c.is_empty()) {
            d.card = None;
        }
        Ok(d)
    }

    async fn anthropic(
        &self,
        p: &Provider,
        key: &str,
        system: &str,
        user: &str,
        schema: &Value,
    ) -> Result<String> {
        let base = p
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.anthropic.com".into());
        let mut body = json!({
            "model": p.model,
            "max_tokens": 16000,
            "system": [{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }],
            "messages": [{ "role": "user", "content": user }],
            // Mechanical rewrite + extraction: low effort keeps it fast and cheap.
            "output_config": { "effort": "low", "format": { "type": "json_schema", "schema": schema } }
        });
        let mut req = self
            .http
            .post(format!("{}/v1/messages", base.trim_end_matches('/')))
            .header("anthropic-version", "2023-06-01");
        let mut betas: Vec<&str> = vec![];
        // OAuth logins send a bearer token plus the oauth beta; pasted keys use x-api-key.
        if p.auth == "oauth" {
            req = req.bearer_auth(key);
            betas.push("oauth-2025-04-20");
        } else {
            req = req.header("x-api-key", key);
        }
        // Server-side refusal fallback where the model family supports it (not Haiku).
        if !p.model.contains("haiku") {
            body["fallbacks"] = json!("default");
            betas.push("server-side-fallback-2026-07-01");
        }
        if !betas.is_empty() {
            req = req.header("anthropic-beta", betas.join(","));
        }
        let resp = req.json(&body).send().await?;
        let status = resp.status();
        let v: Value = resp.json().await?;
        if !status.is_success() {
            return Err(Error::Upstream {
                status: status.as_u16(),
                body: v.to_string(),
            });
        }
        match v["stop_reason"].as_str() {
            Some("refusal") => return Err(Error::other("provider declined this message")),
            Some("max_tokens") => return Err(Error::other("response cut off (max_tokens)")),
            _ => {}
        }
        v["content"]
            .as_array()
            .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
            .and_then(|b| b["text"].as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| Error::other("no text in provider response"))
    }

    async fn openai(
        &self,
        p: &Provider,
        key: &str,
        system: &str,
        user: &str,
        schema: &Value,
    ) -> Result<String> {
        let base = p.base_url.clone().ok_or_else(|| {
            Error::InvalidParams("OpenAI-compatible provider needs a base URL".into())
        })?;
        let body = json!({
            "model": p.model,
            "messages": [{ "role": "system", "content": system }, { "role": "user", "content": user }],
            "response_format": { "type": "json_schema", "json_schema": { "name": "result", "strict": true, "schema": schema } }
        });
        let resp = self
            .http
            .post(format!("{}/chat/completions", base.trim_end_matches('/')))
            .bearer_auth(key)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let v: Value = resp.json().await?;
        if !status.is_success() {
            return Err(Error::Upstream {
                status: status.as_u16(),
                body: v.to_string(),
            });
        }
        v["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| Error::other("no content in provider response"))
    }
}

/// One directory entry as the people clean-up sees it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersonEntry {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
    pub company: String,
}

/// Entries the AI says are one person (or organization/list), with a clean display name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersonGroup {
    pub ids: Vec<String>,
    pub name: String,
    pub kind: String,
}

const PEOPLE_SYSTEM: &str = "You clean up an address book built from email headers.\n\
Each entry has an id, a raw display name, its email addresses and maybe a company.\n\
Return every entry exactly once inside a group:\n\
- Put entries in the same group only when they are clearly the same human (same person writing from several addresses, \
or the same name written differently: order, case, accents, initials, nicknames). When unsure, keep them apart.\n\
- name: how the address book should show them. People: \"First Last\" with proper capitalization and accents \
(\"NAGEOTTE Pierre-Etienne (75)\" -> \"Pierre-Etienne Nageotte\", \"lieber adele\" -> \"Adèle Lieber\", \"Delobelle, Anne-Charlotte\" -> \"Anne-Charlotte Delobelle\"). \
Drop noise like \"(Airbnb)\" unless it is the only identifying part. If only an address is known, infer the name from it only when obvious (jean.dupont@ -> Jean Dupont), else keep the address.\n\
- kind: person, organization (companies, services, shops) or list (mailing lists, groups, shared inboxes).";

/// Merges a newly seen card into the stored one: newer non-empty fields win, lists are unioned.
pub fn merge_card(old: &Card, new: &Card) -> Card {
    let pick = |a: &String, b: &String| {
        if b.trim().is_empty() {
            a.clone()
        } else {
            b.clone()
        }
    };
    let union = |a: &[String], b: &[String]| {
        let mut out: Vec<String> = a.to_vec();
        for x in b {
            let norm: String = x.chars().filter(|c| !c.is_whitespace()).collect();
            if !out
                .iter()
                .any(|y| y.chars().filter(|c| !c.is_whitespace()).collect::<String>() == norm)
            {
                out.push(x.clone());
            }
        }
        out
    };
    Card {
        name: pick(&old.name, &new.name),
        title: pick(&old.title, &new.title),
        company: pick(&old.company, &new.company),
        phones: union(&old.phones, &new.phones),
        address: pick(&old.address, &new.address),
        website: pick(&old.website, &new.website),
        socials: union(&old.socials, &new.socials),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn card(title: &str, phones: &[&str]) -> Card {
        Card {
            title: title.into(),
            phones: phones.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[rstest]
    fn newer_title_wins() {
        assert_eq!(
            merge_card(&card("CFO", &[]), &card("CEO", &[])).title,
            "CEO"
        );
    }

    #[rstest]
    fn empty_field_keeps_old_value() {
        assert_eq!(merge_card(&card("CFO", &[]), &card("", &[])).title, "CFO");
    }

    #[rstest]
    fn phones_are_unioned_ignoring_spaces() {
        assert_eq!(
            merge_card(&card("", &["+33 6 12"]), &card("", &["+33612", "+1 555"])).phones,
            vec!["+33 6 12".to_string(), "+1 555".to_string()]
        );
    }

    #[rstest]
    fn name_only_card_counts_as_empty() {
        assert!(
            Card {
                name: "Marc".into(),
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[rstest]
    fn schema_requires_both_fields() {
        assert_eq!(schema()["required"], json!(["clean_body", "card"]));
    }

    #[rstest::rstest]
    fn forwards_are_flagged_to_the_model() {
        assert!(user_prompt("Aude", "TR: Event", "body").contains("FORWARDS another email"));
    }

    #[rstest::rstest]
    fn replies_are_not_flagged_as_forwards() {
        assert!(!user_prompt("Aude", "RE: Event", "body").contains("FORWARDS"));
    }
}
