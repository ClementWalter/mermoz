//! `mermoz`: command-line access to the local Mermoz mailbox for people and agents.
//! Every command is non-interactive and supports `--json`.

mod client;
mod mcp;

use clap::{Parser, Subcommand};
use client::{Client, RpcError};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::ExitCode;

/// Exit codes are part of the agent contract (see `mermoz --help`).
const EXIT_ERROR: u8 = 1;
const EXIT_APPROVAL: u8 = 3;
const EXIT_NOT_FOUND: u8 = 4;
const EXIT_DAEMON: u8 = 5;

const HELP_FOOTER: &str = "\
Examples:
  mermoz list --status todo                 Threads waiting on you
  mermoz search \"from:marc audit\" --json    Full-text search, JSON output
  mermoz thread M-1287                      Read a whole thread
  mermoz attachments M-1287 --out /tmp/a    Save a thread's attachments
  mermoz status M-1287 waiting              Change a thread's status
  mermoz draft reply M-1287 --file r.txt    Draft a reply (never sends)
  mermoz send dft_8f2c                      Ask to send; the user approves in Mermoz

Exit codes: 0 ok · 1 error · 2 usage · 3 approval required (tell the user) · 4 not found · 5 Mermoz not running";

#[derive(Parser)]
#[command(name = "mermoz", version, about = "Your Mermoz mailbox from the terminal. Instant, offline, agent-friendly.", after_help = HELP_FOOTER)]
struct Cli {
    /// Machine-readable JSON output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List connected accounts and their sync state.
    Accounts,
    /// Connect a Gmail account (opens the browser).
    AddAccount,
    /// List threads, newest first.
    List {
        /// Status key(s), comma separated (triage, todo, waiting, ...).
        #[arg(long)]
        status: Option<String>,
        /// Project name or id.
        #[arg(long)]
        project: Option<String>,
        /// Account email or id.
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Full-text search (from:, to:, subject: prefixes supported).
    Search {
        query: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Show a thread with all messages.
    Thread { id: String },
    /// List a thread's attachments, or save them with --out.
    Attachments {
        thread: String,
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Set the status of one or more threads.
    Status {
        #[arg(required = true, num_args = 2..)]
        args: Vec<String>,
    },
    /// Set priority: none, urgent, high, normal, low.
    Priority { thread: String, priority: String },
    /// Put a thread in a project (created if missing); "none" clears.
    Project { thread: String, project: String },
    /// Set a follow-up date (YYYY-MM-DD); "none" clears.
    Due { thread: String, date: String },
    /// Mark a thread read or unread.
    Read {
        thread: String,
        #[arg(long)]
        unread: bool,
    },
    /// Create and manage drafts.
    #[command(subcommand)]
    Draft(DraftCmd),
    /// Ask to send a draft. Unless enabled by the user, it waits for approval in Mermoz (exit 3).
    Send { draft: String },
    /// Workflow statuses.
    Statuses,
    /// Projects.
    Projects,
    /// Report friction with this CLI so it can improve.
    Feedback {
        text: Vec<String>,
        /// The command that caused trouble.
        #[arg(long)]
        command: Option<String>,
        /// Who is reporting (e.g. claude-code).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Trigger an immediate sync.
    Sync,
    /// Run an MCP server on stdio.
    Mcp,
}

#[derive(Subcommand)]
enum DraftCmd {
    /// Draft a reply; recipients and subject default from the thread.
    Reply {
        thread: String,
        #[arg(long)]
        body: Option<String>,
        /// Read the body from a file ("-" for stdin).
        #[arg(long)]
        file: Option<String>,
    },
    /// Draft a new message.
    New {
        #[arg(long, value_delimiter = ',', required = true)]
        to: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        cc: Vec<String>,
        #[arg(long, default_value = "")]
        subject: String,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        file: Option<String>,
        /// Account email or id to send from.
        #[arg(long)]
        account: Option<String>,
    },
    /// List drafts (optionally by state: draft, review, rejected).
    List {
        #[arg(long)]
        state: Option<String>,
    },
    /// Show a draft.
    Show { id: String },
    /// Replace a draft's body.
    Edit {
        id: String,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        file: Option<String>,
    },
    /// Delete a draft.
    Delete { id: String },
}

struct Out {
    json: bool,
}

impl Out {
    fn line(&self, s: impl AsRef<str>) {
        let _ = writeln!(std::io::stdout(), "{}", s.as_ref());
    }
    fn json(&self, v: &Value) {
        self.line(serde_json::to_string_pretty(v).unwrap_or_default());
    }
}

fn read_body(body: Option<String>, file: Option<String>) -> Result<String, RpcError> {
    let usage = |m: String| RpcError {
        code: -32602,
        message: m,
        data: None,
    };
    match (body, file) {
        (Some(b), _) => Ok(b),
        (None, Some(f)) if f == "-" => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| usage(e.to_string()))?;
            Ok(s)
        }
        (None, Some(f)) => {
            std::fs::read_to_string(&f).map_err(|e| usage(format!("cannot read {f}: {e}")))
        }
        (None, None) => Err(usage(
            "give the body with --body TEXT or --file PATH (- for stdin)".into(),
        )),
    }
}

fn fmt_date(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn prio_name(p: i64) -> &'static str {
    match p {
        1 => "Urgent",
        2 => "High",
        3 => "Normal",
        4 => "Low",
        _ => "-",
    }
}

fn parse_priority(s: &str) -> Result<i64, RpcError> {
    match s.to_lowercase().as_str() {
        "none" | "0" => Ok(0),
        "urgent" | "1" => Ok(1),
        "high" | "2" => Ok(2),
        "normal" | "3" => Ok(3),
        "low" | "4" => Ok(4),
        _ => Err(RpcError {
            code: -32602,
            message: format!("priority must be one of none, urgent, high, normal, low (got '{s}')"),
            data: None,
        }),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn print_threads(out: &Out, threads: &Value) {
    let empty = vec![];
    let arr = threads.as_array().unwrap_or(&empty);
    if arr.is_empty() {
        out.line("No threads.");
    }
    for t in arr {
        out.line(format!(
            "{:<8} {:<11} {:<6} {:<18} {}  {}{}",
            t["number"].as_str().unwrap_or(""),
            t["status"].as_str().unwrap_or(""),
            prio_name(t["priority"].as_i64().unwrap_or(0)),
            truncate(t["last_from"].as_str().unwrap_or(""), 18),
            fmt_date(t["last_date"].as_i64().unwrap_or(0)),
            if t["unread"].as_bool().unwrap_or(false) {
                "* "
            } else {
                ""
            },
            truncate(t["subject"].as_str().unwrap_or(""), 70),
        ));
    }
}

/// Status by key or display name, case-insensitive.
fn resolve_status(c: &mut Client, s: &str) -> Result<String, RpcError> {
    let statuses = c.call("statuses.list", json!({}))?;
    let norm = s.to_lowercase().replace([' ', '-'], "_");
    for st in statuses.as_array().into_iter().flatten() {
        let key = st["key"].as_str().unwrap_or("");
        if key == norm
            || st["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .replace(' ', "_")
                == norm
        {
            return Ok(key.to_string());
        }
    }
    let keys: Vec<&str> = statuses
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["key"].as_str())
        .collect();
    Err(RpcError {
        code: -32602,
        message: format!("unknown status '{s}'; valid: {}", keys.join(", ")),
        data: None,
    })
}

fn resolve_project(c: &mut Client, name: &str, create: bool) -> Result<String, RpcError> {
    let projects = c.call("projects.list", json!({}))?;
    for p in projects.as_array().into_iter().flatten() {
        if p["id"].as_str() == Some(name)
            || p["name"]
                .as_str()
                .map(|n| n.eq_ignore_ascii_case(name))
                .unwrap_or(false)
        {
            return Ok(p["id"].as_str().unwrap_or("").to_string());
        }
    }
    if !create {
        return Err(RpcError {
            code: -32004,
            message: format!("no project named '{name}'"),
            data: None,
        });
    }
    Ok(c.call("projects.create", json!({ "name": name }))?["id"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

fn resolve_account(c: &mut Client, a: &str) -> Result<String, RpcError> {
    let accounts = c.call("accounts.list", json!({}))?;
    for acc in accounts.as_array().into_iter().flatten() {
        if acc["id"].as_str() == Some(a)
            || acc["email"]
                .as_str()
                .map(|e| e.eq_ignore_ascii_case(a))
                .unwrap_or(false)
        {
            return Ok(acc["id"].as_str().unwrap_or("").to_string());
        }
    }
    let emails: Vec<&str> = accounts
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["email"].as_str())
        .collect();
    Err(RpcError {
        code: -32004,
        message: format!("no account '{a}'; connected: {}", emails.join(", ")),
        data: None,
    })
}

fn print_draft(out: &Out, d: &Value) {
    out.line(format!(
        "{}  [{}]",
        d["id"].as_str().unwrap_or(""),
        d["state"].as_str().unwrap_or("")
    ));
    out.line(format!(
        "To: {}",
        d["to"]
            .as_array()
            .map(|a| a
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", "))
            .unwrap_or_default()
    ));
    let cc = d["cc"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if !cc.is_empty() {
        out.line(format!("Cc: {cc}"));
    }
    out.line(format!(
        "Subject: {}\n",
        d["subject"].as_str().unwrap_or("")
    ));
    out.line(d["body"].as_str().unwrap_or(""));
}

fn run(cli: Cli) -> Result<(), RpcError> {
    let out = Out { json: cli.json };
    if let Cmd::Mcp = cli.cmd {
        return mcp::serve().map_err(|e| RpcError {
            code: -32000,
            message: e.to_string(),
            data: None,
        });
    }
    let mut c = Client::connect()?;
    match cli.cmd {
        Cmd::Accounts => {
            let v = c.call("accounts.list", json!({}))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            for a in v.as_array().into_iter().flatten() {
                let s = &a["sync"];
                out.line(format!(
                    "{}  {}  {} ({}/{})",
                    a["id"].as_str().unwrap_or(""),
                    a["email"].as_str().unwrap_or(""),
                    s["state"].as_str().unwrap_or(""),
                    s["synced"],
                    s["total"]
                ));
            }
        }
        Cmd::AddAccount => {
            let v = c.call("accounts.add_gmail", json!({}))?;
            let url = v["auth_url"].as_str().unwrap_or("");
            let _ = std::process::Command::new("open").arg(url).status();
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(format!(
                "Opened the browser to sign in. If it didn't open:\n{url}"
            ));
        }
        Cmd::List {
            status,
            project,
            account,
            limit,
        } => {
            let mut p = json!({ "limit": limit });
            if let Some(s) = status {
                let keys: Vec<String> = s
                    .split(',')
                    .map(|x| resolve_status(&mut c, x.trim()))
                    .collect::<Result<_, _>>()?;
                p["status"] = json!(keys);
            }
            if let Some(pr) = project {
                p["project_id"] = json!(resolve_project(&mut c, &pr, false)?);
            }
            if let Some(a) = account {
                p["account_id"] = json!(resolve_account(&mut c, &a)?);
            }
            let v = c.call("threads.list", p)?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            print_threads(&out, &v["threads"]);
            let shown = v["threads"].as_array().map(|a| a.len()).unwrap_or(0);
            let total = v["total"].as_i64().unwrap_or(0);
            if (shown as i64) < total {
                out.line(format!("… {shown} of {total} shown (use --limit)"));
            }
        }
        Cmd::Search { query, limit } => {
            let v = c.call(
                "search",
                json!({ "query": query.join(" "), "limit": limit }),
            )?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            print_threads(&out, &v["threads"]);
        }
        Cmd::Thread { id } => {
            let v = c.call("threads.get", json!({ "id": id }))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            let t = &v["thread"];
            out.line(format!(
                "{}  {}",
                t["number"].as_str().unwrap_or(""),
                t["subject"].as_str().unwrap_or("")
            ));
            out.line(format!(
                "status: {} · priority: {} · due: {}",
                t["status"].as_str().unwrap_or(""),
                prio_name(t["priority"].as_i64().unwrap_or(0)),
                t["due"].as_str().unwrap_or("-")
            ));
            for m in v["messages"].as_array().into_iter().flatten() {
                let f = &m["from"];
                out.line(format!(
                    "\n── {} <{}> · {}",
                    f["name"].as_str().unwrap_or(""),
                    f["email"].as_str().unwrap_or(""),
                    fmt_date(m["date"].as_i64().unwrap_or(0))
                ));
                out.line(m["body_text"].as_str().unwrap_or("").trim_end());
                for a in m["attachments"].as_array().into_iter().flatten() {
                    out.line(format!(
                        "  attachment {} {} ({} bytes)",
                        a["id"].as_str().unwrap_or(""),
                        a["filename"].as_str().unwrap_or(""),
                        a["size"]
                    ));
                }
            }
        }
        Cmd::Attachments { thread, out: dir } => {
            let v = c.call("threads.get", json!({ "id": thread }))?;
            let mut list = vec![];
            for m in v["messages"].as_array().into_iter().flatten() {
                for a in m["attachments"].as_array().into_iter().flatten() {
                    let info = c.call("attachments.get", json!({ "id": a["id"] }))?;
                    let mut entry = info.clone();
                    entry["id"] = a["id"].clone();
                    if let Some(d) = &dir {
                        std::fs::create_dir_all(d).map_err(|e| RpcError {
                            code: -32000,
                            message: e.to_string(),
                            data: None,
                        })?;
                        let dest = d.join(info["filename"].as_str().unwrap_or("attachment"));
                        std::fs::copy(info["path"].as_str().unwrap_or(""), &dest).map_err(|e| {
                            RpcError {
                                code: -32000,
                                message: e.to_string(),
                                data: None,
                            }
                        })?;
                        entry["saved_to"] = json!(dest.to_string_lossy());
                    }
                    list.push(entry);
                }
            }
            if out.json {
                return {
                    out.json(&json!(list));
                    Ok(())
                };
            }
            if list.is_empty() {
                out.line("No attachments.");
            }
            for e in list {
                out.line(format!(
                    "{}  {}  {}",
                    e["id"].as_str().unwrap_or(""),
                    e["filename"].as_str().unwrap_or(""),
                    e["saved_to"].as_str().or(e["path"].as_str()).unwrap_or("")
                ));
            }
        }
        Cmd::Status { mut args } => {
            let status = resolve_status(&mut c, &args.pop().unwrap_or_default())?;
            let v = c.call("threads.update", json!({ "ids": args, "status": status }))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(format!("{} thread(s) → {status}", v["updated"]));
        }
        Cmd::Priority { thread, priority } => {
            let p = parse_priority(&priority)?;
            let v = c.call("threads.update", json!({ "ids": [thread], "priority": p }))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(format!("priority → {}", prio_name(p)));
        }
        Cmd::Project { thread, project } => {
            let pid = if project.eq_ignore_ascii_case("none") {
                Value::Null
            } else {
                json!(resolve_project(&mut c, &project, true)?)
            };
            let v = c.call(
                "threads.update",
                json!({ "ids": [thread], "project_id": pid }),
            )?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(format!("project → {project}"));
        }
        Cmd::Due { thread, date } => {
            let d = if date.eq_ignore_ascii_case("none") {
                Value::Null
            } else {
                json!(date)
            };
            let v = c.call("threads.update", json!({ "ids": [thread], "due": d }))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(format!("due → {date}"));
        }
        Cmd::Read { thread, unread } => {
            let v = c.call(
                "threads.mark_read",
                json!({ "id": thread, "read": !unread }),
            )?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(if unread {
                "marked unread"
            } else {
                "marked read"
            });
        }
        Cmd::Draft(d) => match d {
            DraftCmd::Reply { thread, body, file } => {
                let body = read_body(body, file)?;
                let v = c.call(
                    "drafts.create",
                    json!({ "thread_id": thread, "body": body }),
                )?;
                if out.json {
                    return {
                        out.json(&v);
                        Ok(())
                    };
                }
                print_draft(&out, &v);
            }
            DraftCmd::New {
                to,
                cc,
                subject,
                body,
                file,
                account,
            } => {
                let body = read_body(body, file)?;
                let mut p = json!({ "to": to, "cc": cc, "subject": subject, "body": body });
                if let Some(a) = account {
                    p["account_id"] = json!(resolve_account(&mut c, &a)?);
                }
                let v = c.call("drafts.create", p)?;
                if out.json {
                    return {
                        out.json(&v);
                        Ok(())
                    };
                }
                print_draft(&out, &v);
            }
            DraftCmd::List { state } => {
                let v = c.call("drafts.list", json!({ "state": state }))?;
                if out.json {
                    return {
                        out.json(&v);
                        Ok(())
                    };
                }
                for d in v.as_array().into_iter().flatten() {
                    out.line(format!(
                        "{}  [{}]  {}",
                        d["id"].as_str().unwrap_or(""),
                        d["state"].as_str().unwrap_or(""),
                        d["subject"].as_str().unwrap_or("")
                    ));
                }
            }
            DraftCmd::Show { id } => {
                let v = c.call("drafts.get", json!({ "id": id }))?;
                if out.json {
                    return {
                        out.json(&v);
                        Ok(())
                    };
                }
                print_draft(&out, &v);
            }
            DraftCmd::Edit { id, body, file } => {
                let body = read_body(body, file)?;
                let v = c.call("drafts.update", json!({ "id": id, "body": body }))?;
                if out.json {
                    return {
                        out.json(&v);
                        Ok(())
                    };
                }
                print_draft(&out, &v);
            }
            DraftCmd::Delete { id } => {
                let v = c.call("drafts.delete", json!({ "id": id }))?;
                if out.json {
                    return {
                        out.json(&v);
                        Ok(())
                    };
                }
                out.line("deleted");
            }
        },
        Cmd::Send { draft } => {
            let v = c.call("drafts.send", json!({ "id": draft }))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line("sent");
        }
        Cmd::Statuses => {
            let v = c.call("statuses.list", json!({}))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            for s in v.as_array().into_iter().flatten() {
                out.line(format!(
                    "{:<12} {}",
                    s["key"].as_str().unwrap_or(""),
                    s["name"].as_str().unwrap_or("")
                ));
            }
        }
        Cmd::Projects => {
            let v = c.call("projects.list", json!({}))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            for p in v.as_array().into_iter().flatten() {
                out.line(format!(
                    "{}  {}",
                    p["id"].as_str().unwrap_or(""),
                    p["name"].as_str().unwrap_or("")
                ));
            }
        }
        Cmd::Feedback {
            text,
            command,
            agent,
        } => {
            let v = c.call(
                "feedback.submit",
                json!({ "text": text.join(" "), "command": command, "agent": agent }),
            )?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line("thanks, feedback recorded");
        }
        Cmd::Sync => {
            let v = c.call("sync.now", json!({}))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line("sync requested");
        }
        Cmd::Mcp => unreachable!(),
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json_mode = cli.json;
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let code = match e.code {
                -32010 => EXIT_APPROVAL,
                -32004 => EXIT_NOT_FOUND,
                client::DAEMON_UNREACHABLE => EXIT_DAEMON,
                -32602 => 2,
                _ => EXIT_ERROR,
            };
            if json_mode {
                let v =
                    json!({ "error": { "code": e.code, "message": e.message, "data": e.data } });
                let _ = writeln!(
                    std::io::stdout(),
                    "{}",
                    serde_json::to_string_pretty(&v).unwrap_or_default()
                );
            }
            let _ = writeln!(std::io::stderr(), "error: {}", e.message);
            ExitCode::from(code)
        }
    }
}
