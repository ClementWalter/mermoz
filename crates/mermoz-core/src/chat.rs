//! "Chat about this": a multi-turn conversation with the account's AI about one thread.
//! Builds the context the AI sees (the thread, its people, its status, the user's own
//! earlier mail with them, today's date), streams the answer from the provider, and turns
//! the `<actions>` block the AI may end with into proposals the user clicks to apply.

use crate::agent::Provider;
use crate::error::{Error, Result};
use crate::model::{Message, ThreadSummary};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

/// One proposal under an answer; nothing runs until the user clicks it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ChatAction {
    /// "draft_reply", "status", "project" or "follow_up".
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Follow-up date, YYYY-MM-DD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    /// Reply text for "draft_reply".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// A stored turn of a thread's conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    pub thread_id: String,
    /// "user" or "assistant".
    pub role: String,
    pub text: String,
    #[serde(default)]
    pub actions: Vec<ChatAction>,
    /// The AI looked things up on the web for this answer.
    #[serde(default)]
    pub searched: bool,
    /// Web search was asked for but this AI refused it, so the answer comes without.
    #[serde(default)]
    pub research_unavailable: bool,
    pub created: i64,
}

/// One message as the provider sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub role: &'static str,
    pub text: String,
}

/// One of the user's earlier messages to the people of this thread, from another thread.
#[derive(Debug, Clone, PartialEq)]
pub struct PastMessage {
    pub date: i64,
    pub subject: String,
    pub body: String,
}

/// Everything the AI is told about the thread.
pub struct ThreadContext<'a> {
    /// e.g. "2026-10-12 (Monday)".
    pub today: &'a str,
    pub me: &'a str,
    pub thread: &'a ThreadSummary,
    pub messages: &'a [Message],
    pub history: &'a [PastMessage],
    /// (key, display name) of the statuses the AI may propose.
    pub statuses: &'a [(String, String)],
    pub projects: &'a [String],
}

/// A message longer than this is cut (with a visible note) so one huge mail can't crowd out the rest.
const MAX_MESSAGE_CHARS: usize = 8_000;
/// Whole-thread budget; the newest messages are kept and the count of dropped ones is stated.
const MAX_THREAD_CHARS: usize = 60_000;
pub const MAX_ACTIONS: usize = 4;

const INSTRUCTIONS: &str = "You are the user's assistant inside Mermoz, their email app. \
The user opened a chat about one email conversation (shown below) because they are not sure what to do or what to say.\n\
Help them decide: explain what is being asked of them, lay out the realistic options with their trade-offs, \
ask one short question when the answer would change your advice, and recommend a path. \
When web search is available, use it to check facts that matter to the decision (who someone is, a company, prices, dates, rules). \
Reply in the language the user writes in. Be brief and easy to scan: light Markdown only (short paragraphs, \"- \" lists, **bold**, links); no headings, no tables.\n\
You cannot send email or change anything yourself; you can only propose actions that the user clicks. \
When a concrete next step fits, end your answer with an <actions> block holding a JSON array of at most 4 proposals, and nothing after it:\n\
<actions>[{\"kind\":\"draft_reply\",\"label\":\"Draft a reply\",\"body\":\"the full reply, ready to edit\"},\
{\"kind\":\"status\",\"status\":\"waiting\",\"label\":\"Move to Waiting\"},\
{\"kind\":\"project\",\"project\":\"Clients/Acme\",\"label\":\"File in Clients/Acme\"},\
{\"kind\":\"follow_up\",\"due\":\"2026-10-20\",\"label\":\"Remind me on Oct 20\"}]</actions>\n\
Only use statuses and projects listed in the context. A draft reply is written as the user, in the conversation's language, \
plain text, with greeting and sign-off, without subject line or quoted history. Leave the block out when no action fits, and never mention it.";

/// Today in the user's time zone, as the AI is told it: "2026-10-12 (Monday)".
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d (%A)").to_string()
}

fn fmt_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn who(name: &str, email: &str) -> String {
    if name.is_empty() {
        email.to_string()
    } else {
        format!("{name} <{email}>")
    }
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max).collect();
    format!("{kept}\n[… message cut, too long]")
}

/// The system prompt: fixed instructions, then the thread (stable across turns, so cacheable).
pub fn system_prompt(c: &ThreadContext) -> String {
    format!("{INSTRUCTIONS}\n\n{}", render_context(c))
}

pub fn render_context(c: &ThreadContext) -> String {
    let t = c.thread;
    let mut out = String::new();
    out.push_str(&format!("Today: {}\nThe user: {}\n\n", c.today, c.me));
    let status = c
        .statuses
        .iter()
        .find(|(k, _)| *k == t.status)
        .map(|(_, n)| n.as_str())
        .unwrap_or(t.status.as_str());
    out.push_str(&format!(
        "Conversation {}: \"{}\"\nStatus: {status} · Priority: {} · Projects: {} · Follow-up: {}\n",
        t.number,
        t.subject,
        priority_name(t.priority),
        if t.projects.is_empty() {
            "none".to_string()
        } else {
            t.projects.join(", ")
        },
        t.due.as_deref().unwrap_or("none"),
    ));
    out.push_str(&format!(
        "Participants: {}\n\n",
        participants(c.messages, c.me).join(", ")
    ));
    let bodies: Vec<String> = c
        .messages
        .iter()
        .map(|m| {
            let from = if m.is_from_me {
                format!("the user ({})", m.from.email)
            } else {
                who(&m.from.name, &m.from.email)
            };
            let body = m.body_clean.as_deref().unwrap_or(&m.body_new);
            let files: Vec<&str> = m.attachments.iter().map(|a| a.filename.as_str()).collect();
            let files = if files.is_empty() {
                String::new()
            } else {
                format!("\n[attachments: {}]", files.join(", "))
            };
            format!(
                "--- From {from} · {}\n{}{files}\n",
                fmt_date(m.date),
                cut(body.trim(), MAX_MESSAGE_CHARS)
            )
        })
        .collect();
    // Newest first until the budget runs out, then shown oldest first.
    let mut kept = vec![];
    let mut used = 0;
    for b in bodies.iter().rev() {
        if used + b.len() > MAX_THREAD_CHARS && !kept.is_empty() {
            break;
        }
        used += b.len();
        kept.push(b.as_str());
    }
    kept.reverse();
    out.push_str("Messages, oldest first:\n");
    if kept.len() < bodies.len() {
        out.push_str(&format!(
            "[{} earlier messages left out for length]\n",
            bodies.len() - kept.len()
        ));
    }
    for b in kept {
        out.push_str(b);
    }
    if !c.history.is_empty() {
        out.push_str("\nThe user's own earlier messages to these people, in other conversations (newest first):\n");
        for h in c.history {
            out.push_str(&format!(
                "--- {} · \"{}\"\n{}\n",
                fmt_date(h.date),
                h.subject,
                h.body.trim()
            ));
        }
    }
    let statuses: Vec<String> = c
        .statuses
        .iter()
        .map(|(k, n)| format!("{k} ({n})"))
        .collect();
    out.push_str(&format!(
        "\nStatuses you may propose: {}\nProjects you may propose: {}\n",
        statuses.join(", "),
        if c.projects.is_empty() {
            "none yet".to_string()
        } else {
            c.projects.join(", ")
        }
    ));
    out
}

/// A Google Drive document the thread links to, as text.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedDoc {
    pub title: String,
    /// "Google Doc", "Google Sheet"…
    pub kind: String,
    pub text: String,
}

/// One linked document longer than this is cut, with a visible note.
const MAX_DOC_CHARS: usize = 20_000;
/// All linked documents together; later ones are left out past it, and the AI is told.
const MAX_DOCS_CHARS: usize = 40_000;

/// The linked documents section appended to the system prompt; empty without any.
pub fn render_documents(docs: &[LinkedDoc]) -> String {
    if docs.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\nDocuments linked in this conversation (from the user's Google Drive, as text):\n",
    );
    let mut used = 0;
    for (i, d) in docs.iter().enumerate() {
        let body = cut(d.text.trim(), MAX_DOC_CHARS);
        if used + body.len() > MAX_DOCS_CHARS && i > 0 {
            out.push_str(&format!(
                "[{} more linked documents left out for length]\n",
                docs.len() - i
            ));
            break;
        }
        used += body.len();
        out.push_str(&format!("--- {} \"{}\"\n{body}\n", d.kind, d.title));
    }
    out
}

fn priority_name(p: i64) -> &'static str {
    match p {
        1 => "Urgent",
        2 => "High",
        3 => "Normal",
        4 => "Low",
        _ => "None",
    }
}

/// Everyone on the thread except the user, each address once, in order of appearance.
pub fn participants(messages: &[Message], me: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for m in messages {
        for a in std::iter::once(&m.from).chain(&m.to).chain(&m.cc) {
            let e = a.email.to_lowercase();
            if e.is_empty() || e == me.to_lowercase() || !seen.insert(e) {
                continue;
            }
            out.push(who(&a.name, &a.email));
        }
    }
    out
}

/// The conversation as the provider expects it: alternating roles, starting and ending
/// with the user. Earlier proposals are restated so the AI remembers what it offered.
pub fn provider_turns(history: &[ChatMessage], question: &str) -> Vec<Turn> {
    let mut turns: Vec<Turn> = vec![];
    let all = history
        .iter()
        .map(|m| {
            let role = if m.role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            let mut text = m.text.clone();
            if !m.actions.is_empty() {
                let labels: Vec<&str> = m.actions.iter().map(|a| a.label.as_str()).collect();
                text.push_str(&format!("\n\n(Proposed: {})", labels.join("; ")));
            }
            (role, text)
        })
        .chain(std::iter::once(("user", question.to_string())));
    for (role, text) in all {
        if text.trim().is_empty() {
            continue;
        }
        match turns.last_mut() {
            Some(last) if last.role == role => {
                last.text.push_str("\n\n");
                last.text.push_str(&text);
            }
            None if role == "assistant" => {}
            _ => turns.push(Turn { role, text }),
        }
    }
    turns
}

const OPEN: &str = "<actions>";
const CLOSE: &str = "</actions>";

/// The part of a streaming answer to show: the `<actions>` block (or the start of its tag
/// at the very end) is hidden as it arrives.
pub fn visible_while_streaming(raw: &str) -> &str {
    if let Some(i) = raw.find(OPEN) {
        return raw[..i].trim_end();
    }
    for n in (1..OPEN.len()).rev() {
        if raw.ends_with(&OPEN[..n]) {
            return &raw[..raw.len() - n];
        }
    }
    raw
}

/// Splits a finished answer into the text to show and the raw proposals.
pub fn split_actions(raw: &str) -> (String, Vec<ChatAction>) {
    let Some(i) = raw.find(OPEN) else {
        return (raw.trim().to_string(), vec![]);
    };
    let visible = raw[..i].trim().to_string();
    let rest = &raw[i + OPEN.len()..];
    let inner = rest.find(CLOSE).map(|j| &rest[..j]).unwrap_or(rest).trim();
    // Some models wrap the array in a code fence.
    let inner = inner
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let actions = serde_json::from_str::<Vec<Value>>(inner)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| serde_json::from_value::<ChatAction>(v).ok())
        .collect();
    (visible, actions)
}

/// Keeps only proposals the app can apply to this thread, with canonical names and a
/// label for each; at most [`MAX_ACTIONS`].
pub fn validate_actions(
    actions: Vec<ChatAction>,
    thread: &ThreadSummary,
    statuses: &[(String, String)],
    projects: &[String],
) -> Vec<ChatAction> {
    let mut out: Vec<ChatAction> = vec![];
    for a in actions {
        let fixed = match a.kind.as_str() {
            "draft_reply" => {
                let body = a.body.clone().unwrap_or_default();
                (!body.trim().is_empty()).then(|| ChatAction {
                    kind: a.kind.clone(),
                    label: a.label.clone(),
                    body: Some(body.trim().to_string()),
                    ..Default::default()
                })
            }
            "status" => {
                let want = a.status.clone().unwrap_or_default().to_lowercase();
                statuses
                    .iter()
                    .find(|(k, n)| *k == want || n.to_lowercase() == want)
                    .filter(|(k, _)| *k != thread.status && k != "review")
                    .map(|(k, n)| ChatAction {
                        kind: a.kind.clone(),
                        label: if a.label.trim().is_empty() {
                            format!("Move to {n}")
                        } else {
                            a.label.clone()
                        },
                        status: Some(k.clone()),
                        ..Default::default()
                    })
            }
            "project" => {
                let want = a.project.clone().unwrap_or_default().to_lowercase();
                projects
                    .iter()
                    .find(|p| p.to_lowercase() == want)
                    .filter(|p| !thread.projects.contains(p))
                    .map(|p| ChatAction {
                        kind: a.kind.clone(),
                        label: if a.label.trim().is_empty() {
                            format!("File in {p}")
                        } else {
                            a.label.clone()
                        },
                        project: Some(p.clone()),
                        ..Default::default()
                    })
            }
            "follow_up" => a
                .due
                .as_deref()
                .and_then(|d| chrono::NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok())
                .map(|d| ChatAction {
                    kind: a.kind.clone(),
                    label: if a.label.trim().is_empty() {
                        format!("Remind me on {}", d.format("%Y-%m-%d"))
                    } else {
                        a.label.clone()
                    },
                    due: Some(d.format("%Y-%m-%d").to_string()),
                    ..Default::default()
                }),
            _ => None,
        };
        let Some(mut f) = fixed else { continue };
        if f.label.trim().is_empty() {
            f.label = "Draft a reply".into();
        }
        f.label = f.label.trim().chars().take(60).collect();
        if out.iter().any(|o| o.kind == f.kind && o.label == f.label) {
            continue;
        }
        out.push(f);
        if out.len() == MAX_ACTIONS {
            break;
        }
    }
    out
}

// ---- streaming providers ---------------------------------------------------

/// Turns a byte stream into the payloads of its `data:` lines (Server-Sent Events).
#[derive(Default)]
pub struct SseLines {
    buf: Vec<u8>,
}

impl SseLines {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = vec![];
        while let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(data) = line.trim_end().strip_prefix("data:") {
                out.push(data.trim().to_string());
            }
        }
        out
    }
}

/// What a streamed answer produced.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reply {
    pub text: String,
    pub searched: bool,
    pub research_unavailable: bool,
}

/// What one provider event adds to the answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Text(String),
    Searched,
    /// Anthropic paused a long server-side tool loop (`pause_turn`); the turn can resume.
    Paused,
    Done,
    Failed(String),
    Nothing,
}

/// Anthropic Messages API stream events.
pub fn anthropic_event(data: &str) -> Delta {
    let Ok(ev) = serde_json::from_str::<Value>(data) else {
        return Delta::Nothing;
    };
    match ev["type"].as_str() {
        Some("content_block_delta") if ev["delta"]["type"] == "text_delta" => {
            Delta::Text(ev["delta"]["text"].as_str().unwrap_or("").to_string())
        }
        Some("content_block_start") if ev["content_block"]["type"] == "server_tool_use" => {
            Delta::Searched
        }
        Some("message_delta") => match ev["delta"]["stop_reason"].as_str() {
            Some("refusal") => Delta::Failed("Your AI declined to answer this.".into()),
            Some("pause_turn") => Delta::Paused,
            _ => Delta::Nothing,
        },
        Some("message_stop") => Delta::Done,
        Some("error") => Delta::Failed(
            ev["error"]["message"]
                .as_str()
                .unwrap_or("Your AI stopped with an error.")
                .to_string(),
        ),
        _ => Delta::Nothing,
    }
}

/// OpenAI Responses API stream events (ChatGPT sign-in).
pub fn responses_event(data: &str) -> Delta {
    let Ok(ev) = serde_json::from_str::<Value>(data) else {
        return Delta::Nothing;
    };
    let kind = ev["type"].as_str().unwrap_or("");
    match kind {
        "response.output_text.delta" => Delta::Text(ev["delta"].as_str().unwrap_or("").to_string()),
        "response.completed" => Delta::Done,
        "response.failed" | "error" => Delta::Failed(
            ev["response"]["error"]["message"]
                .as_str()
                .or(ev["message"].as_str())
                .unwrap_or("Your AI stopped with an error.")
                .to_string(),
        ),
        k if k.starts_with("response.web_search_call.") => Delta::Searched,
        _ => Delta::Nothing,
    }
}

/// OpenAI-compatible Chat Completions stream chunks.
pub fn completions_event(data: &str) -> Delta {
    if data == "[DONE]" {
        return Delta::Done;
    }
    let Ok(ev) = serde_json::from_str::<Value>(data) else {
        return Delta::Nothing;
    };
    match ev["choices"][0]["delta"]["content"].as_str() {
        Some(t) => Delta::Text(t.to_string()),
        None => Delta::Nothing,
    }
}

/// The web search tool version the model accepts (dynamic filtering on recent models).
pub fn anthropic_search_tool(model: &str) -> Value {
    const RECENT: &[&str] = &[
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-fable",
        "claude-mythos",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
    ];
    let kind = if RECENT.iter().any(|p| model.starts_with(p)) {
        "web_search_20260209"
    } else {
        "web_search_20250305"
    };
    json!({ "type": kind, "name": "web_search", "max_uses": 5 })
}

/// Whether this provider kind can search the web at all.
pub fn can_research(p: &Provider) -> bool {
    matches!(p.kind.as_str(), "anthropic" | "chatgpt")
}

/// How many times a paused Anthropic turn is resumed before the answer so far is kept.
pub const MAX_CONTINUATIONS: usize = 3;

/// Rebuilds the content blocks of a streamed Anthropic response, so a paused turn can be
/// sent back exactly as the API produced it: text, thinking (with its signature), server
/// tool calls (input from `input_json_delta`), and whole blocks such as search results.
#[derive(Default)]
pub struct ContentBlocks {
    blocks: Vec<Value>,
    partial_json: std::collections::HashMap<usize, String>,
}

impl ContentBlocks {
    pub fn apply(&mut self, data: &str) {
        let Ok(ev) = serde_json::from_str::<Value>(data) else {
            return;
        };
        let i = ev["index"].as_u64().unwrap_or(0) as usize;
        match ev["type"].as_str() {
            Some("content_block_start") => {
                if self.blocks.len() <= i {
                    self.blocks.resize(i + 1, Value::Null);
                }
                self.blocks[i] = ev["content_block"].clone();
            }
            Some("content_block_delta") => {
                let Some(b) = self.blocks.get_mut(i) else {
                    return;
                };
                let d = &ev["delta"];
                let append = |b: &mut Value, field: &str, piece: &Value| {
                    let s = format!(
                        "{}{}",
                        b[field].as_str().unwrap_or(""),
                        piece.as_str().unwrap_or("")
                    );
                    b[field] = json!(s);
                };
                match d["type"].as_str() {
                    Some("text_delta") => append(b, "text", &d["text"]),
                    Some("thinking_delta") => append(b, "thinking", &d["thinking"]),
                    Some("signature_delta") => append(b, "signature", &d["signature"]),
                    Some("input_json_delta") => self
                        .partial_json
                        .entry(i)
                        .or_default()
                        .push_str(d["partial_json"].as_str().unwrap_or("")),
                    Some("citations_delta") => {
                        if !b["citations"].is_array() {
                            b["citations"] = json!([]);
                        }
                        if let Some(c) = b["citations"].as_array_mut() {
                            c.push(d["citation"].clone());
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                if let (Some(raw), Some(b)) = (self.partial_json.remove(&i), self.blocks.get_mut(i))
                {
                    b["input"] = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
                }
            }
            _ => {}
        }
    }

    pub fn into_blocks(self) -> Vec<Value> {
        self.blocks.into_iter().filter(|b| !b.is_null()).collect()
    }
}

/// Builds the HTTP request for one streamed answer. `resume` holds the blocks of a paused
/// Anthropic turn, sent back as the trailing assistant message.
fn request(
    http: &reqwest::Client,
    p: &Provider,
    key: &str,
    system: &str,
    turns: &[Turn],
    research: bool,
    resume: &[Value],
) -> Result<reqwest::RequestBuilder> {
    let mut msgs: Vec<Value> = turns
        .iter()
        .map(|t| json!({ "role": t.role, "content": t.text }))
        .collect();
    if !resume.is_empty() {
        msgs.push(json!({ "role": "assistant", "content": resume }));
    }
    match p.kind.as_str() {
        "anthropic" => {
            let base = p
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".into());
            let mut body = json!({
                "model": p.model,
                "max_tokens": 16000,
                "stream": true,
                "system": [{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }],
                "messages": msgs,
            });
            if research {
                body["tools"] = json!([anthropic_search_tool(&p.model)]);
            }
            let mut req = http
                .post(format!("{}/v1/messages", base.trim_end_matches('/')))
                .header("anthropic-version", "2023-06-01");
            let mut betas: Vec<&str> = vec![];
            if p.auth == "oauth" {
                req = req.bearer_auth(key);
                betas.push("oauth-2025-04-20");
            } else {
                req = req.header("x-api-key", key);
            }
            if !p.model.contains("haiku") {
                body["fallbacks"] = json!("default");
                betas.push("server-side-fallback-2026-07-01");
            }
            if !betas.is_empty() {
                req = req.header("anthropic-beta", betas.join(","));
            }
            Ok(req.json(&body))
        }
        "chatgpt" => {
            let mut body = json!({
                "model": p.model,
                "instructions": system,
                "input": msgs,
                "store": false,
                "stream": true,
            });
            if research {
                body["tools"] = json!([{ "type": "web_search" }]);
            }
            Ok(http
                .post(format!("{}/responses", crate::chatgpt::api_base()))
                .bearer_auth(key)
                .json(&body))
        }
        "openai" => {
            let base = p.base_url.clone().ok_or_else(|| {
                Error::InvalidParams("OpenAI-compatible provider needs a base URL".into())
            })?;
            let mut all = vec![json!({ "role": "system", "content": system })];
            all.extend(msgs);
            Ok(http
                .post(format!("{}/chat/completions", base.trim_end_matches('/')))
                .bearer_auth(key)
                .json(&json!({ "model": p.model, "messages": all, "stream": true })))
        }
        other => Err(Error::InvalidParams(format!(
            "unknown provider kind '{other}'"
        ))),
    }
}

/// Streams one answer. `on_text` gets the whole answer so far after every piece. With
/// `research`, a provider that refuses its web search tool (400) is asked again without it.
pub async fn stream_reply(
    p: &Provider,
    key: &str,
    system: &str,
    turns: &[Turn],
    research: bool,
    on_text: &mut (dyn FnMut(&str) + Send),
) -> Result<Reply> {
    let wanted = research;
    let research = research && can_research(p);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    let mut research = research;
    let mut reply = Reply {
        research_unavailable: wanted && !research,
        ..Default::default()
    };
    let parse: fn(&str) -> Delta = match p.kind.as_str() {
        "anthropic" => anthropic_event,
        "chatgpt" => responses_event,
        _ => completions_event,
    };
    // Blocks of the turn so far; non-empty only while resuming a paused turn.
    let mut resume: Vec<Value> = vec![];
    let mut continuations = 0;
    loop {
        let mut resp = request(&http, p, key, system, turns, research, &resume)?
            .send()
            .await?;
        if research && resume.is_empty() && resp.status() == reqwest::StatusCode::BAD_REQUEST {
            tracing::warn!(provider = %p.name, "web search refused; answering without it");
            reply.research_unavailable = true;
            research = false;
            resp = request(&http, p, key, system, turns, false, &resume)?
                .send()
                .await?;
        }
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Upstream {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        let mut lines = SseLines::default();
        let mut blocks = ContentBlocks::default();
        let mut paused = false;
        'read: while let Some(chunk) = resp.chunk().await? {
            for data in lines.push(&chunk) {
                blocks.apply(&data);
                match parse(&data) {
                    Delta::Text(t) => {
                        reply.text.push_str(&t);
                        on_text(&reply.text);
                    }
                    Delta::Searched => reply.searched = true,
                    Delta::Paused => paused = true,
                    Delta::Done => break 'read,
                    Delta::Failed(m) => return Err(Error::other(m)),
                    Delta::Nothing => {}
                }
            }
        }
        if !paused {
            break;
        }
        if continuations == MAX_CONTINUATIONS {
            tracing::warn!(provider = %p.name, "turn still paused after {MAX_CONTINUATIONS} continuations; keeping the answer so far");
            break;
        }
        // The paused turn goes back as the trailing assistant message; the server resumes it.
        continuations += 1;
        resume.extend(blocks.into_blocks());
    }
    if reply.text.trim().is_empty() {
        return Err(Error::other("Your AI returned an empty answer. Try again."));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Address;
    use rstest::*;

    fn thread() -> ThreadSummary {
        ThreadSummary {
            id: "thr_1".into(),
            number: "M-12".into(),
            account_id: "acc".into(),
            subject: "Audit report v2".into(),
            snippet: String::new(),
            participants: vec![],
            last_from: String::new(),
            last_date: 0,
            unread: false,
            message_count: 1,
            has_attachments: false,
            status: "todo".into(),
            priority: 2,
            projects: vec!["Clients/Acme".into()],
            due: None,
            summary: None,
            next_action: None,
            suggestion: None,
            conversation_id: "thr_1".into(),
            conversation_kind: "person".into(),
            conversation_threads: 1,
        }
    }

    fn message(from: &str, body: &str, mine: bool) -> Message {
        Message {
            id: "m".into(),
            thread_id: "thr_1".into(),
            from: Address {
                name: "Marc Petit".into(),
                email: from.into(),
            },
            to: vec![Address {
                name: String::new(),
                email: "me@example.com".into(),
            }],
            cc: vec![Address {
                name: "Léa".into(),
                email: "lea@acme.example".into(),
            }],
            bcc: vec![],
            reply_to: vec![],
            is_bounce: false,
            is_auto_reply: false,
            delivery_failure: None,
            date: 1_791_700_000_000,
            subject: "Audit report v2".into(),
            body_text: body.into(),
            body_new: body.into(),
            has_quoted: false,
            has_signature: false,
            forwarded: false,
            forwarded_from: None,
            quoted_only: false,
            reaction: None,
            duplicate_of: None,
            copies: 1,
            body_clean: None,
            has_html: false,
            is_from_me: mine,
            attachments: vec![],
            auth: None,
        }
    }

    fn statuses() -> Vec<(String, String)> {
        vec![
            ("todo".into(), "To do".into()),
            ("waiting".into(), "Waiting".into()),
            ("review".into(), "Review".into()),
            ("archive".into(), "Archive".into()),
        ]
    }

    fn projects() -> Vec<String> {
        vec!["Clients/Acme".into(), "Finance".into()]
    }

    #[fixture]
    fn context_text() -> String {
        let t = thread();
        let msgs = vec![message(
            "marc@acme.example",
            "Please sign off the timeline",
            false,
        )];
        let history = vec![PastMessage {
            date: 1_790_000_000_000,
            subject: "Kickoff".into(),
            body: "Happy to start Monday".into(),
        }];
        render_context(&ThreadContext {
            today: "2026-10-12 (Monday)",
            me: "me@example.com",
            thread: &t,
            messages: &msgs,
            history: &history,
            statuses: &statuses(),
            projects: &projects(),
        })
    }

    #[rstest]
    fn context_states_today(context_text: String) {
        assert!(context_text.contains("Today: 2026-10-12 (Monday)"));
    }

    #[rstest]
    fn context_shows_status_by_name(context_text: String) {
        assert!(context_text.contains("Status: To do · Priority: High"));
    }

    #[rstest]
    fn context_lists_projects(context_text: String) {
        assert!(context_text.contains("Projects: Clients/Acme"));
    }

    #[rstest]
    fn context_carries_the_message_body(context_text: String) {
        assert!(context_text.contains("Please sign off the timeline"));
    }

    #[rstest]
    fn context_includes_earlier_mail_with_these_people(context_text: String) {
        assert!(context_text.contains("Happy to start Monday"));
    }

    #[rstest]
    fn context_offers_the_projects_to_propose(context_text: String) {
        assert!(context_text.contains("Projects you may propose: Clients/Acme, Finance"));
    }

    fn doc(title: &str, text: &str) -> LinkedDoc {
        LinkedDoc {
            title: title.into(),
            kind: "Google Doc".into(),
            text: text.into(),
        }
    }

    #[rstest]
    fn no_documents_adds_nothing() {
        assert_eq!(render_documents(&[]), "");
    }

    #[rstest]
    fn documents_carry_title_and_text() {
        assert!(
            render_documents(&[doc("Remediation plan", "Phase 1 ends Oct 30")])
                .contains("--- Google Doc \"Remediation plan\"\nPhase 1 ends Oct 30")
        );
    }

    #[rstest]
    fn a_long_document_is_cut() {
        assert!(
            render_documents(&[doc("Big", &"x".repeat(MAX_DOC_CHARS + 10))])
                .contains("[… message cut, too long]")
        );
    }

    #[rstest]
    fn documents_past_the_budget_are_counted() {
        let big = "x".repeat(MAX_DOC_CHARS);
        let docs = vec![doc("A", &big), doc("B", &big), doc("C", &big)];
        assert!(render_documents(&docs).contains("[1 more linked documents left out for length]"));
    }

    #[rstest]
    fn participants_skip_the_user() {
        let msgs = vec![message("marc@acme.example", "x", false)];
        assert_eq!(
            participants(&msgs, "ME@example.com"),
            vec![
                "Marc Petit <marc@acme.example>".to_string(),
                "Léa <lea@acme.example>".to_string()
            ]
        );
    }

    #[rstest]
    fn clean_body_is_preferred() {
        let t = thread();
        let mut m = message("marc@acme.example", "raw body", false);
        m.body_clean = Some("tidy body".into());
        let msgs = vec![m];
        let text = render_context(&ThreadContext {
            today: "",
            me: "me@example.com",
            thread: &t,
            messages: &msgs,
            history: &[],
            statuses: &statuses(),
            projects: &[],
        });
        assert!(text.contains("tidy body"));
    }

    #[rstest]
    fn long_threads_keep_the_newest_message() {
        let t = thread();
        let big = "x".repeat(MAX_MESSAGE_CHARS);
        let mut msgs: Vec<Message> = (0..12)
            .map(|_| message("marc@acme.example", &big, false))
            .collect();
        msgs.push(message("marc@acme.example", "the latest word", false));
        let text = render_context(&ThreadContext {
            today: "",
            me: "me@example.com",
            thread: &t,
            messages: &msgs,
            history: &[],
            statuses: &statuses(),
            projects: &[],
        });
        assert!(text.contains("the latest word"));
    }

    #[rstest]
    fn long_threads_say_what_was_left_out() {
        let t = thread();
        let big = "x".repeat(MAX_MESSAGE_CHARS);
        let msgs: Vec<Message> = (0..12)
            .map(|_| message("marc@acme.example", &big, false))
            .collect();
        let text = render_context(&ThreadContext {
            today: "",
            me: "me@example.com",
            thread: &t,
            messages: &msgs,
            history: &[],
            statuses: &statuses(),
            projects: &[],
        });
        assert!(text.contains("earlier messages left out for length"));
    }

    fn msg(role: &str, text: &str) -> ChatMessage {
        ChatMessage {
            id: "c".into(),
            thread_id: "t".into(),
            role: role.into(),
            text: text.into(),
            actions: vec![],
            searched: false,
            research_unavailable: false,
            created: 0,
        }
    }

    #[rstest]
    fn first_question_is_one_user_turn() {
        assert_eq!(
            provider_turns(&[], "What now?"),
            vec![Turn {
                role: "user",
                text: "What now?".into()
            }]
        );
    }

    #[rstest]
    fn unanswered_question_merges_with_the_next() {
        let turns = provider_turns(&[msg("user", "First?")], "Second?");
        assert_eq!(turns[0].text, "First?\n\nSecond?");
    }

    #[rstest]
    fn turns_alternate_roles() {
        let turns = provider_turns(&[msg("user", "Q"), msg("assistant", "A")], "Q2");
        let roles: Vec<&str> = turns.iter().map(|t| t.role).collect();
        assert_eq!(roles, vec!["user", "assistant", "user"]);
    }

    #[rstest]
    fn earlier_proposals_are_restated() {
        let mut a = msg("assistant", "Wait for him.");
        a.actions = vec![ChatAction {
            kind: "status".into(),
            label: "Move to Waiting".into(),
            ..Default::default()
        }];
        let turns = provider_turns(&[msg("user", "Q"), a], "ok");
        assert!(turns[1].text.ends_with("(Proposed: Move to Waiting)"));
    }

    #[rstest]
    #[case::whole_block("Advice.\n<actions>[{\"kind\"", "Advice.")]
    #[case::partial_tag("Advice.\n<act", "Advice.\n")]
    #[case::lone_bracket("a < b", "a < b")]
    #[case::plain("Just text", "Just text")]
    fn streaming_hides_the_actions_block(#[case] raw: &str, #[case] want: &str) {
        assert_eq!(visible_while_streaming(raw), want);
    }

    const ANSWER: &str = "Reply today.\n\n<actions>[{\"kind\":\"status\",\"status\":\"waiting\",\"label\":\"Move to Waiting\"}]</actions>";

    #[rstest]
    fn split_keeps_the_text_before_the_block() {
        assert_eq!(split_actions(ANSWER).0, "Reply today.");
    }

    #[rstest]
    fn split_parses_the_proposals() {
        assert_eq!(
            split_actions(ANSWER).1[0].status.as_deref(),
            Some("waiting")
        );
    }

    #[rstest]
    fn split_accepts_a_fenced_array() {
        let raw = "Hi\n<actions>\n```json\n[{\"kind\":\"follow_up\",\"due\":\"2026-10-20\"}]\n```\n</actions>";
        assert_eq!(split_actions(raw).1.len(), 1);
    }

    #[rstest]
    fn malformed_block_yields_no_proposals() {
        assert!(split_actions("Hi <actions>[{oops</actions>").1.is_empty());
    }

    #[rstest]
    fn malformed_block_is_still_hidden() {
        assert_eq!(split_actions("Hi <actions>[{oops").0, "Hi");
    }

    fn validate(a: ChatAction) -> Vec<ChatAction> {
        validate_actions(vec![a], &thread(), &statuses(), &projects())
    }

    fn action(kind: &str) -> ChatAction {
        ChatAction {
            kind: kind.into(),
            ..Default::default()
        }
    }

    #[rstest]
    fn status_by_display_name_becomes_its_key() {
        let a = ChatAction {
            status: Some("Waiting".into()),
            ..action("status")
        };
        assert_eq!(validate(a)[0].status.as_deref(), Some("waiting"));
    }

    #[rstest]
    fn status_gets_a_default_label() {
        let a = ChatAction {
            status: Some("archive".into()),
            ..action("status")
        };
        assert_eq!(validate(a)[0].label, "Move to Archive");
    }

    #[rstest]
    #[case::unknown("someday")]
    #[case::current("todo")]
    #[case::review_is_not_for_the_ai("review")]
    fn unusable_status_is_dropped(#[case] status: &str) {
        let a = ChatAction {
            status: Some(status.into()),
            ..action("status")
        };
        assert!(validate(a).is_empty());
    }

    #[rstest]
    fn project_name_is_made_canonical() {
        let a = ChatAction {
            project: Some("finance".into()),
            ..action("project")
        };
        assert_eq!(validate(a)[0].project.as_deref(), Some("Finance"));
    }

    #[rstest]
    #[case::unknown("Holidays")]
    #[case::already_filed("Clients/Acme")]
    fn unusable_project_is_dropped(#[case] project: &str) {
        let a = ChatAction {
            project: Some(project.into()),
            ..action("project")
        };
        assert!(validate(a).is_empty());
    }

    #[rstest]
    #[case::valid("2026-10-20", 1)]
    #[case::not_a_date("next week", 0)]
    fn follow_up_needs_a_date(#[case] due: &str, #[case] kept: usize) {
        let a = ChatAction {
            due: Some(due.into()),
            ..action("follow_up")
        };
        assert_eq!(validate(a).len(), kept);
    }

    #[rstest]
    fn empty_draft_is_dropped() {
        let a = ChatAction {
            body: Some("  ".into()),
            ..action("draft_reply")
        };
        assert!(validate(a).is_empty());
    }

    #[rstest]
    fn draft_gets_a_default_label() {
        let a = ChatAction {
            body: Some("Hi Marc".into()),
            ..action("draft_reply")
        };
        assert_eq!(validate(a)[0].label, "Draft a reply");
    }

    #[rstest]
    fn unknown_kind_is_dropped() {
        assert!(validate(action("send_now")).is_empty());
    }

    #[rstest]
    fn at_most_four_proposals() {
        let many: Vec<ChatAction> = (0..6)
            .map(|i| ChatAction {
                body: Some("x".into()),
                label: format!("Draft {i}"),
                ..action("draft_reply")
            })
            .collect();
        assert_eq!(
            validate_actions(many, &thread(), &statuses(), &projects()).len(),
            MAX_ACTIONS
        );
    }

    #[rstest]
    fn sse_lines_wait_for_the_end_of_line() {
        let mut s = SseLines::default();
        s.push(b"data: {\"a\"");
        assert_eq!(s.push(b":1}\n\n"), vec!["{\"a\":1}".to_string()]);
    }

    #[rstest]
    fn sse_lines_skip_event_names() {
        let mut s = SseLines::default();
        assert_eq!(s.push(b"event: ping\ndata: x\n"), vec!["x".to_string()]);
    }

    #[rstest]
    fn sse_lines_keep_multibyte_text_split_across_chunks() {
        let mut s = SseLines::default();
        let bytes = "data: é\n".as_bytes();
        s.push(&bytes[..7]);
        assert_eq!(s.push(&bytes[7..]), vec!["é".to_string()]);
    }

    #[rstest]
    #[case::text(r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}"#, Delta::Text("Hi".into()))]
    #[case::search(r#"{"type":"content_block_start","content_block":{"type":"server_tool_use","name":"web_search"}}"#, Delta::Searched)]
    #[case::stop(r#"{"type":"message_stop"}"#, Delta::Done)]
    #[case::ping(r#"{"type":"ping"}"#, Delta::Nothing)]
    fn anthropic_events(#[case] data: &str, #[case] want: Delta) {
        assert_eq!(anthropic_event(data), want);
    }

    #[rstest]
    fn anthropic_refusal_is_an_error() {
        assert!(matches!(
            anthropic_event(r#"{"type":"message_delta","delta":{"stop_reason":"refusal"}}"#),
            Delta::Failed(_)
        ));
    }

    #[rstest]
    #[case::text(r#"{"type":"response.output_text.delta","delta":"Hi"}"#, Delta::Text("Hi".into()))]
    #[case::search(r#"{"type":"response.web_search_call.searching"}"#, Delta::Searched)]
    #[case::done(r#"{"type":"response.completed","response":{}}"#, Delta::Done)]
    fn responses_events(#[case] data: &str, #[case] want: Delta) {
        assert_eq!(responses_event(data), want);
    }

    #[rstest]
    #[case::text(r#"{"choices":[{"delta":{"content":"Hi"}}]}"#, Delta::Text("Hi".into()))]
    #[case::done("[DONE]", Delta::Done)]
    fn completions_events(#[case] data: &str, #[case] want: Delta) {
        assert_eq!(completions_event(data), want);
    }

    #[rstest]
    #[case::opus_5_5("claude-opus-5-5", "web_search_20260209")]
    #[case::haiku("claude-haiku-4-5", "web_search_20250305")]
    fn search_tool_follows_the_model(#[case] model: &str, #[case] want: &str) {
        assert_eq!(anthropic_search_tool(model)["type"], want);
    }

    #[rstest]
    #[case::anthropic("anthropic", true)]
    #[case::chatgpt("chatgpt", true)]
    #[case::compatible("openai", false)]
    fn research_depends_on_the_provider(#[case] kind: &str, #[case] want: bool) {
        let p = Provider {
            id: "p".into(),
            name: "P".into(),
            kind: kind.into(),
            base_url: None,
            model: "m".into(),
            auth: "key".into(),
            profile: None,
        };
        assert_eq!(can_research(&p), want);
    }

    #[rstest]
    fn pause_turn_is_a_pause() {
        assert_eq!(
            anthropic_event(r#"{"type":"message_delta","delta":{"stop_reason":"pause_turn"}}"#),
            Delta::Paused
        );
    }

    /// A paused response as Anthropic streams it: thinking, a web search call, its
    /// results (whole), and text with a citation.
    const PAUSED: &[&str] = &[
        r#"{"type":"message_start","message":{}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Check the firm."}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-1"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{}}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":"}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"Acme audit\"}"}}"#,
        r#"{"type":"content_block_stop","index":1}"#,
        r#"{"type":"content_block_start","index":2,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[{"type":"web_search_result","url":"https://acme.example","title":"Acme"}]}}"#,
        r#"{"type":"content_block_stop","index":2}"#,
        r#"{"type":"content_block_start","index":3,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":"Acme is "}}"#,
        r#"{"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":"an auditor."}}"#,
        r#"{"type":"content_block_delta","index":3,"delta":{"type":"citations_delta","citation":{"type":"web_search_result_location","url":"https://acme.example"}}}"#,
        r#"{"type":"content_block_stop","index":3}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"pause_turn"}}"#,
        r#"{"type":"message_stop"}"#,
    ];

    #[fixture]
    fn paused_blocks() -> Vec<Value> {
        let mut b = ContentBlocks::default();
        for ev in PAUSED {
            b.apply(ev);
        }
        b.into_blocks()
    }

    #[rstest]
    fn rebuilt_blocks_keep_their_order(paused_blocks: Vec<Value>) {
        let kinds: Vec<&str> = paused_blocks
            .iter()
            .map(|b| b["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "thinking",
                "server_tool_use",
                "web_search_tool_result",
                "text"
            ]
        );
    }

    #[rstest]
    fn rebuilt_thinking_keeps_its_signature(paused_blocks: Vec<Value>) {
        assert_eq!(
            paused_blocks[0],
            json!({ "type": "thinking", "thinking": "Check the firm.", "signature": "sig-1" })
        );
    }

    #[rstest]
    fn rebuilt_tool_call_has_its_parsed_input(paused_blocks: Vec<Value>) {
        assert_eq!(paused_blocks[1]["input"], json!({ "query": "Acme audit" }));
    }

    #[rstest]
    fn rebuilt_search_results_are_kept_whole(paused_blocks: Vec<Value>) {
        assert_eq!(
            paused_blocks[2]["content"][0]["url"],
            "https://acme.example"
        );
    }

    #[rstest]
    fn rebuilt_text_joins_its_pieces(paused_blocks: Vec<Value>) {
        assert_eq!(paused_blocks[3]["text"], "Acme is an auditor.");
    }

    #[rstest]
    fn rebuilt_text_keeps_its_citations(paused_blocks: Vec<Value>) {
        assert_eq!(
            paused_blocks[3]["citations"][0]["url"],
            "https://acme.example"
        );
    }
}
