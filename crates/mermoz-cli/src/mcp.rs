//! `mermoz mcp`: a stdio MCP server exposing the mailbox as tools, backed by mermozd.

use crate::client::Client;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

const PROTOCOL_VERSION: &str = "2025-06-18";

fn tools() -> Value {
    let s = |d: &str| json!({ "type": "string", "description": d });
    json!([
        { "name": "search_mail", "description": "Full-text search across all local mail (instant, offline). Supports from:, to:, subject: prefixes. Returns threads best match first.",
          "inputSchema": { "type": "object", "properties": { "query": s("search words"), "limit": { "type": "integer", "default": 20 } }, "required": ["query"] } },
        { "name": "list_threads", "description": "List threads (issues) filtered by status/project/account, newest first. Statuses: triage, backlog, todo, in_progress, waiting, review, done, canceled.",
          "inputSchema": { "type": "object", "properties": { "status": { "type": "array", "items": { "type": "string" } }, "project_id": s("project id"), "account_id": s("account id"), "limit": { "type": "integer", "default": 50 } } } },
        { "name": "get_thread", "description": "Read a whole thread: every message's plain-text body, recipients and attachment ids.",
          "inputSchema": { "type": "object", "properties": { "id": s("thread id or number like M-1287") }, "required": ["id"] } },
        { "name": "update_thread", "description": "Change a thread's status, priority (0 none,1 urgent,2 high,3 normal,4 low), project_id (null clears) or due date (YYYY-MM-DD, null clears).",
          "inputSchema": { "type": "object", "properties": { "id": s("thread id or number"), "status": s("status key"), "priority": { "type": "integer" }, "project_id": { "type": ["string", "null"] }, "due": { "type": ["string", "null"] } }, "required": ["id"] } },
        { "name": "get_attachment", "description": "Get an attachment's local file path (readable offline); text attachments also return their content.",
          "inputSchema": { "type": "object", "properties": { "id": s("attachment id from get_thread") }, "required": ["id"] } },
        { "name": "create_draft", "description": "Create a draft. With thread_id it is a reply and recipients/subject default from the thread. Drafts never send by themselves.",
          "inputSchema": { "type": "object", "properties": { "thread_id": s("thread to reply to"), "to": { "type": "array", "items": { "type": "string" } }, "cc": { "type": "array", "items": { "type": "string" } }, "subject": s("subject"), "body": s("plain text body") }, "required": ["body"] } },
        { "name": "send_draft", "description": "Ask to send a draft. Unless the user enabled agent sending, the draft goes to the user's Review lane and this returns approval_required: tell the user, do not retry.",
          "inputSchema": { "type": "object", "properties": { "id": s("draft id") }, "required": ["id"] } },
        { "name": "list_statuses", "description": "Workflow statuses.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "list_projects", "description": "Projects threads can belong to.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "submit_feedback", "description": "Report friction with these tools (missing option, confusing result, slowness) so Mermoz can improve.",
          "inputSchema": { "type": "object", "properties": { "text": s("what went wrong or is missing"), "tool": s("tool name") }, "required": ["text"] } }
    ])
}

fn call_tool(c: &mut Client, name: &str, a: &Value) -> Result<Value, String> {
    let r = |res: Result<Value, crate::client::RpcError>| {
        res.map_err(|e| match e.data {
            Some(d) => format!("{} {}", e.message, d),
            None => e.message,
        })
    };
    match name {
        "search_mail" => r(c.call(
            "search",
            json!({ "query": a["query"], "limit": a.get("limit").cloned().unwrap_or(json!(20)) }),
        )),
        "list_threads" => {
            let mut p = a.clone();
            if p.get("limit").is_none() {
                p["limit"] = json!(50);
            }
            r(c.call("threads.list", p))
        }
        "get_thread" => r(c.call("threads.get", json!({ "id": a["id"] }))),
        "update_thread" => {
            let mut p = a.clone();
            let id = p
                .as_object_mut()
                .and_then(|o| o.remove("id"))
                .unwrap_or(Value::Null);
            p["ids"] = json!([id]);
            r(c.call("threads.update", p))
        }
        "get_attachment" => {
            let mut v = r(c.call("attachments.get", json!({ "id": a["id"] })))?;
            let mime = v["mime"].as_str().unwrap_or("").to_string();
            let textual = mime.starts_with("text/")
                || mime.contains("json")
                || mime.contains("xml")
                || mime.contains("csv");
            if textual
                && let Some(path) = v["path"].as_str()
                && let Ok(t) = std::fs::read_to_string(path)
            {
                v["content"] = json!(t.chars().take(200_000).collect::<String>());
            }
            Ok(v)
        }
        "create_draft" => r(c.call("drafts.create", a.clone())),
        "send_draft" => r(c.call("drafts.send", json!({ "id": a["id"] }))),
        "list_statuses" => r(c.call("statuses.list", json!({}))),
        "list_projects" => r(c.call("projects.list", json!({}))),
        "submit_feedback" => r(c.call(
            "feedback.submit",
            json!({ "text": a["text"], "command": a.get("tool"), "agent": "mcp" }),
        )),
        other => Err(format!("unknown tool {other}")),
    }
}

pub fn serve() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut client: Option<Client> = None;
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = req.get("id").cloned() else {
            continue;
        }; // notifications need no reply
        let method = req["method"].as_str().unwrap_or("");
        let result: Result<Value, (i64, String)> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "mermoz", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Mermoz is the user's local mailbox. Threads are issues with statuses. Search and reading are instant and offline. Sending always needs the user's approval unless they enabled agent sending."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = req["params"]["name"].as_str().unwrap_or("");
                let args = req["params"].get("arguments").cloned().unwrap_or(json!({}));
                if client.is_none() {
                    client = Client::connect().ok();
                }
                let res = match client.as_mut() {
                    Some(c) => call_tool(c, name, &args),
                    None => Err("Mermoz isn't running. Ask the user to open Mermoz.app.".into()),
                };
                if res.as_ref().is_err_and(|e| {
                    e.contains("lost connection") || e.contains("closed the connection")
                }) {
                    client = None;
                }
                Ok(match res {
                    Ok(v) => {
                        json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v)? }], "isError": false })
                    }
                    Err(e) => {
                        json!({ "content": [{ "type": "text", "text": e }], "isError": true })
                    }
                })
            }
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        let resp = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        };
        writeln!(out, "{resp}")?;
        out.flush()?;
    }
    Ok(())
}
