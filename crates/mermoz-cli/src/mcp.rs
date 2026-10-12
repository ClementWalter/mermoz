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
        { "name": "list_threads", "description": "List threads (issues) filtered by status/project/account, newest first. Statuses: unread, todo, waiting, in_progress, backlog, review, done, archive, deleted. Projects are the user's Gmail labels; each thread lists its `projects` and the AI's pending `suggestion`. `suggested`: true for threads with a pending suggestion, or {status, projects} for one destination listed by column_suggestions. `conversations`: one row per conversation (Gmail threads with the same people about the same matter, as the user sees them), each with `conversation_threads`; `kind`: [\"feed\", \"list\"] for automated senders and mailing lists.",
          "inputSchema": { "type": "object", "properties": { "status": { "type": "array", "items": { "type": "string" } }, "project": s("project (Gmail label) name"), "account_id": s("account id"), "label_id": s("Gmail label id from list_labels (pass its account_id too)"), "suggested": { "type": ["boolean", "object"] }, "conversations": { "type": "boolean" }, "kind": { "type": "array", "items": { "type": "string" } }, "limit": { "type": "integer", "default": 50 } } } },
        { "name": "get_thread", "description": "Read a whole thread: every message's plain-text body, recipients and attachment ids. With `conversation`, every message of the conversation it belongs to (each with its `thread_id`), plus its people, files and links.",
          "inputSchema": { "type": "object", "properties": { "id": s("thread id or number like M-1287"), "conversation": { "type": "boolean" } }, "required": ["id"] } },
        { "name": "split_conversation", "description": "Take a thread out of the conversation Mermoz grouped it into (when it is a different matter). Final: it is never grouped with them again.",
          "inputSchema": { "type": "object", "properties": { "id": s("thread id or number") }, "required": ["id"] } },
        { "name": "merge_conversations", "description": "Join the conversations of two or more threads into one (same people, same matter).",
          "inputSchema": { "type": "object", "properties": { "ids": { "type": "array", "items": { "type": "string" } } }, "required": ["ids"] } },
        { "name": "list_files", "description": "Files shared in any conversation, newest first, with their attachment id (read with get_attachment) and thread.",
          "inputSchema": { "type": "object", "properties": { "query": s("part of a file name"), "limit": { "type": "integer", "default": 50 } } } },
        { "name": "update_thread", "description": "Change a thread's status, priority (0 none,1 urgent,2 high,3 normal,4 low), projects (Gmail labels: add_projects/remove_projects, or projects to replace) or due date (YYYY-MM-DD, null clears). 'deleted' moves the thread to Gmail Trash.",
          "inputSchema": { "type": "object", "properties": { "id": s("thread id or number"), "status": s("status key"), "priority": { "type": "integer" }, "add_projects": { "type": "array", "items": { "type": "string" } }, "remove_projects": { "type": "array", "items": { "type": "string" } }, "projects": { "type": "array", "items": { "type": "string" } }, "due": { "type": ["string", "null"] } }, "required": ["id"] } },
        { "name": "accept_suggestions", "description": "Accept the AI's pending suggestion (status, projects, priority) for threads; they count as read. Pass `ids`, or `column` (a status) to accept that column's pending suggestions — all of them, or only the group given by `to_status`/`to_projects` as listed by column_suggestions.",
          "inputSchema": { "type": "object", "properties": { "ids": { "type": "array", "items": { "type": "string" } }, "column": s("status key of the column"), "to_status": { "type": ["string", "null"], "description": "group's target status (null: groups that only add projects)" }, "to_projects": { "type": "array", "items": { "type": "string" } } } } },
        { "name": "reject_suggestions", "description": "Turn down the AI's pending suggestion for threads: they stay where they are and are not suggested again.",
          "inputSchema": { "type": "object", "properties": { "ids": { "type": "array", "items": { "type": "string" } } }, "required": ["ids"] } },
        { "name": "column_suggestions", "description": "The AI's pending suggestions in status columns, grouped by target (status + projects) with counts, and how far background sorting has got (classified of total). Nothing moves until accepted.",
          "inputSchema": { "type": "object", "properties": { "statuses": { "type": "array", "items": { "type": "string" } }, "project": s("limit to a project"), "account_id": s("limit to an account") }, "required": ["statuses"] } },
        { "name": "resort_mailbox", "description": "Ask the AI to sort whole status columns again in the background (all of todo, waiting, in_progress, backlog, done, archive unless `statuses` is given). Each thread gets a fresh suggestion; threads whose suggestion the user turned down or that the user moved by hand are left alone. Nothing moves until accepted; follow progress with column_suggestions.",
          "inputSchema": { "type": "object", "properties": { "statuses": { "type": "array", "items": { "type": "string" } } } } },
        { "name": "get_attachment", "description": "Get an attachment's local file path (readable offline); text attachments also return their content.",
          "inputSchema": { "type": "object", "properties": { "id": s("attachment id from get_thread") }, "required": ["id"] } },
        { "name": "create_draft", "description": "Create a draft. With thread_id it is a reply and recipients/subject default from the thread. Drafts never send by themselves.",
          "inputSchema": { "type": "object", "properties": { "thread_id": s("thread to reply to"), "to": { "type": "array", "items": { "type": "string" } }, "cc": { "type": "array", "items": { "type": "string" } }, "subject": s("subject"), "body": s("plain text body") }, "required": ["body"] } },
        { "name": "send_draft", "description": "Ask to send a draft. Unless the user enabled agent sending, the draft goes to the user's Review lane and this returns approval_required: tell the user, do not retry.",
          "inputSchema": { "type": "object", "properties": { "id": s("draft id") }, "required": ["id"] } },
        { "name": "search_contacts", "description": "Search the user's directory, built automatically from email signatures: names, titles, companies, phones, addresses, websites. Empty query lists recent contacts.",
          "inputSchema": { "type": "object", "properties": { "query": s("name, email or company"), "limit": { "type": "integer", "default": 20 } } } },
        { "name": "list_labels", "description": "The user's Gmail labels (mailboxes) with ids, accounts and thread counts.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "list_statuses", "description": "Workflow statuses.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "list_projects", "description": "Projects: the user's Gmail labels. A thread can be in several.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "chat_about_thread", "description": "Ask the user's own AI (the one allowed for the thread's account in Mermoz) about a thread; it sees the thread, its people, status and the user's earlier mail with them, and may search the web. The conversation is kept with the thread and shown in the app. Returns the answer and proposed actions (status, project, follow_up date, draft_reply body); nothing is applied: use update_thread or create_draft to act. Without a question, returns the conversation so far.",
          "inputSchema": { "type": "object", "properties": { "id": s("thread id or number"), "question": s("what to ask"), "research": { "type": "boolean", "default": true, "description": "allow web search" } }, "required": ["id"] } },
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
        "get_thread" => r(c.call(
            "threads.get",
            json!({ "id": a["id"], "conversation": a.get("conversation").cloned().unwrap_or(json!(false)) }),
        )),
        "split_conversation" => r(c.call("conversations.split", json!({ "id": a["id"] }))),
        "merge_conversations" => r(c.call("conversations.merge", json!({ "ids": a["ids"] }))),
        "list_files" => r(c.call(
            "files.list",
            json!({ "query": a.get("query").cloned().unwrap_or(Value::Null), "limit": a.get("limit").cloned().unwrap_or(json!(50)) }),
        )),
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
        "list_labels" => r(c.call("labels.list", json!({}))),
        "search_contacts" => r(c.call("contacts.list", json!({ "query": a.get("query"), "limit": a.get("limit").cloned().unwrap_or(json!(20)) }))),
        "list_statuses" => r(c.call("statuses.list", json!({}))),
        "list_projects" => r(c.call("projects.list", json!({}))),
        "accept_suggestions" => match a.get("column").and_then(|v| v.as_str()) {
            None => r(c.call("threads.accept_suggestions", json!({ "ids": a["ids"] }))),
            Some(col) => {
                let target = if a.get("to_status").is_some() || a.get("to_projects").is_some() {
                    json!({ "status": a.get("to_status").cloned().unwrap_or(Value::Null),
                            "projects": a.get("to_projects").cloned().unwrap_or(json!([])) })
                } else {
                    Value::Null
                };
                let mut v = r(c.call("suggestions.accept", json!({ "status": col, "target": target })))?;
                // Snapshots serve the app's undo; they would only flood an agent's context.
                if let Some(o) = v.as_object_mut() {
                    o.remove("before");
                }
                Ok(v)
            }
        },
        "column_suggestions" => r(c.call("suggestions.summary", a.clone())),
        "reject_suggestions" => r(c.call("threads.reject_suggestions", json!({ "ids": a["ids"] }))),
        "resort_mailbox" => r(c.call("suggestions.resort", a.clone())),
        "chat_about_thread" => match a.get("question").and_then(|q| q.as_str()) {
            Some(q) if !q.trim().is_empty() => r(c.call(
                "chat.send",
                json!({ "thread_id": a["id"], "text": q, "research": a.get("research").cloned().unwrap_or(json!(true)) }),
            )),
            _ => r(c.call("chat.get", json!({ "thread_id": a["id"] }))),
        },
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
