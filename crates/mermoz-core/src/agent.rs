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
Remove: quoted earlier messages, signatures, legal disclaimers, confidentiality notices, \"sent from my phone\" footers, unsubscribe and tracking boilerplate, \
and leftover formatting debris (stray symbols, repeated blank lines, broken line wraps). Keep greetings and sign-off names. \
Use plain text: paragraphs separated by one blank line, bullet lists as \"• \" lines. Never summarize, translate, rephrase or add anything.\n\
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
    format!("From: {from}\nSubject: {subject}\n\n<email>\n{body}\n</email>")
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
        let raw = match p.kind.as_str() {
            "anthropic" => {
                self.anthropic(p, api_key, &user_prompt(from, subject, body))
                    .await?
            }
            "openai" => {
                self.openai(p, api_key, &user_prompt(from, subject, body))
                    .await?
            }
            other => {
                return Err(Error::InvalidParams(format!(
                    "unknown provider kind '{other}'"
                )));
            }
        };
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

    async fn anthropic(&self, p: &Provider, key: &str, user: &str) -> Result<String> {
        let base = p
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.anthropic.com".into());
        let mut body = json!({
            "model": p.model,
            "max_tokens": 16000,
            "system": [{ "type": "text", "text": SYSTEM, "cache_control": { "type": "ephemeral" } }],
            "messages": [{ "role": "user", "content": user }],
            // Mechanical rewrite + extraction: low effort keeps it fast and cheap.
            "output_config": { "effort": "low", "format": { "type": "json_schema", "schema": schema() } }
        });
        let mut req = self
            .http
            .post(format!("{}/v1/messages", base.trim_end_matches('/')))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01");
        // Server-side refusal fallback where the model family supports it (not Haiku).
        if !p.model.contains("haiku") {
            body["fallbacks"] = json!("default");
            req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
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

    async fn openai(&self, p: &Provider, key: &str, user: &str) -> Result<String> {
        let base = p.base_url.clone().ok_or_else(|| {
            Error::InvalidParams("OpenAI-compatible provider needs a base URL".into())
        })?;
        let body = json!({
            "model": p.model,
            "messages": [{ "role": "system", "content": SYSTEM }, { "role": "user", "content": user }],
            "response_format": { "type": "json_schema", "json_schema": { "name": "digest", "strict": true, "schema": schema() } }
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
}
