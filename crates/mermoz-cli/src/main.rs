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
  mermoz thread M-1287 --conversation       Read every thread of its conversation
  mermoz list --conversations --status todo One row per conversation
  mermoz conversation split M-1290          Take a thread out of its conversation
  mermoz files invoice                      Files shared anywhere, by name
  mermoz attachments M-1287 --out /tmp/a    Save a thread's attachments
  mermoz status M-1287 waiting              Change a thread's status
  mermoz draft reply M-1287 --file r.txt    Draft a reply (never sends)
  mermoz draft new --to a@x.com --body hi --attach plan.pdf --attach photo.jpg
                                            Draft with files (25 MB in total at most)
  mermoz send dft_8f2c                      Ask to send; the user approves in Mermoz
  mermoz chat M-1287 \"should I accept?\"     Ask the AI about a thread (proposals only)
  mermoz do \"archive mail older than 5 months not in done\"
                                            Plan a bulk change and show it (--yes runs it)

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
    /// Connect a Gmail account, or with --outlook an Outlook one (opens the browser).
    ///
    /// A company Google Workspace that blocks Mermoz can sign in through its own
    /// Google Cloud OAuth client (Desktop app) instead:
    ///   mermoz add-account --client-json ~/Downloads/client_secret_….json
    ///   mermoz add-account --client-id ID --client-secret SECRET
    /// That client is stored with the account and used for its Drive too.
    #[command(verbatim_doc_comment)]
    AddAccount {
        /// The client_secret_*.json downloaded from Google Cloud.
        #[arg(long, conflicts_with_all = ["client_id", "client_secret"])]
        client_json: Option<std::path::PathBuf>,
        /// OAuth client ID (with --client-secret).
        #[arg(long, requires = "client_secret")]
        client_id: Option<String>,
        /// OAuth client secret (with --client-id).
        #[arg(long, requires = "client_id")]
        client_secret: Option<String>,
        /// An Outlook account (outlook.com, Hotmail, Live or Microsoft 365) instead.
        #[arg(long, conflicts_with_all = ["client_json", "client_id", "client_secret"])]
        outlook: bool,
        /// Wait for the browser sign-in to finish and say how it went.
        #[arg(long)]
        wait: bool,
    },
    /// Sign an account in to Google again, keeping its mail (opens the browser).
    /// Needed when `mermoz accounts` says "sign in again".
    Reconnect {
        /// Account id or email.
        account: String,
    },
    /// List threads, newest first.
    List {
        /// Status key(s), comma separated (unread, todo, waiting, archive, ...).
        #[arg(long)]
        status: Option<String>,
        /// Project, i.e. Gmail label name (e.g. "Clients/Acme").
        #[arg(long)]
        project: Option<String>,
        /// Account email or id.
        #[arg(long)]
        account: Option<String>,
        /// Gmail label ("mailbox") name, e.g. "Clients/Acme".
        #[arg(long)]
        label: Option<String>,
        /// One row per conversation (the thread leading it), as in the app.
        #[arg(long)]
        conversations: bool,
        /// Only automated senders and mailing lists (implies --conversations).
        #[arg(long)]
        feeds: bool,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// List your Gmail labels ("mailboxes") with thread counts.
    Labels,
    /// Search the directory built from email signatures (name, email or company).
    Contacts {
        query: Vec<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Show one person's card and recent threads.
    Contact { email: String },
    /// Full-text search: every word must match, as a whole word, accents and case aside;
    /// from:, to:, subject: restrict to a field. Best matches first.
    Search {
        query: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
        /// Search all of Gmail instead (Gmail's own syntax), for mail Mermoz doesn't hold.
        #[arg(long)]
        gmail: bool,
    },
    /// Show a thread with all messages.
    Thread {
        id: String,
        /// Every message of the thread's conversation (Gmail threads read as one chat).
        #[arg(long)]
        conversation: bool,
    },
    /// Split or join conversations (Gmail threads read as one chat).
    #[command(subcommand)]
    Conversation(ConversationCmd),
    /// Files shared in any conversation, newest first, optionally matching a name.
    Files {
        query: Vec<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
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
    /// Add a thread to a project (a Gmail label, created if missing); "none" clears all.
    Project {
        thread: String,
        project: String,
        /// Take the thread out of the project instead.
        #[arg(long)]
        remove: bool,
    },
    /// Accept the AI's suggested status and projects for threads (marks them read),
    /// or for a whole column with --column (optionally only one --to/--to-project group).
    #[command(
        after_help = "Examples:\n  mermoz accept M-12 M-14\n  mermoz accept --column backlog\n  mermoz accept --column backlog --to archive --to-project Receipts"
    )]
    Accept {
        #[arg(required_unless_present = "column", conflicts_with = "column")]
        threads: Vec<String>,
        /// Every pending suggestion of this status column.
        #[arg(long)]
        column: Option<String>,
        /// Only the group going to this status ("none": groups that only add projects).
        #[arg(long, requires = "column")]
        to: Option<String>,
        /// Only the group adding exactly these projects (repeat for several).
        #[arg(long = "to-project", requires = "column")]
        to_projects: Vec<String>,
    },
    /// Turn down the AI's suggestion for threads: they stay where they are and are
    /// not suggested again until new mail arrives.
    Reject {
        #[arg(required = true)]
        threads: Vec<String>,
    },
    /// Stop a whole-mailbox sort and clear the suggestions it left that nobody
    /// reviewed. Accepted and turned-down suggestions are kept.
    #[command(after_help = "Examples:\n  mermoz resort --stop")]
    Resort {
        #[arg(long, required = true)]
        stop: bool,
    },
    /// The AI's pending suggestions in a status column, grouped by where they go,
    /// and how far background sorting of that column has got; with --to/--to-project,
    /// the threads of that one group (accept them with `mermoz accept`).
    #[command(
        after_help = "Examples:\n  mermoz suggestions backlog --ask\n  mermoz suggestions backlog\n  mermoz suggestions backlog --to archive\n  mermoz suggestions backlog --to none --to-project Receipts"
    )]
    Suggestions {
        #[arg(required_unless_present = "clear")]
        status: Option<String>,
        /// First ask the AI about the column's newest --limit threads (at most 50), as
        /// ✨ on the column does.
        #[arg(long, conflicts_with = "clear")]
        ask: bool,
        /// Clear every suggestion on read threads that nobody reviewed (Unread mail keeps
        /// its own); accepted and turned-down ones are kept.
        #[arg(long, conflicts_with_all = ["to", "to_projects"])]
        clear: bool,
        /// List the threads going to this status ("none": groups that only add projects).
        #[arg(long)]
        to: Option<String>,
        /// List the threads going to exactly these projects (repeat for several).
        #[arg(long = "to-project")]
        to_projects: Vec<String>,
        /// Most threads to list.
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Set a follow-up date (YYYY-MM-DD); "none" clears.
    Due { thread: String, date: String },
    /// Give a thread's card your own title instead of the email subject; --clear
    /// shows the subject again. The email itself is not changed.
    Rename {
        thread: String,
        #[arg(required_unless_present = "clear")]
        title: Option<String>,
        #[arg(long, conflicts_with = "title")]
        clear: bool,
    },
    /// Mark a thread read or unread.
    Read {
        thread: String,
        #[arg(long)]
        unread: bool,
    },
    /// Create and manage drafts.
    #[command(subcommand)]
    Draft(DraftCmd),
    /// Google Drive files of a thread, their text, search, saving attachments.
    /// Connecting Drive and sharing files happen in the app only.
    #[command(subcommand)]
    Drive(DriveCmd),
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
    /// Ask the account's AI about a thread (kept with the thread, like in the app). Without
    /// a question, shows the conversation so far. Proposals are listed, never applied.
    Chat {
        thread: String,
        question: Vec<String>,
        /// Don't let the AI search the web for this answer.
        #[arg(long)]
        no_research: bool,
        /// Forget this thread's conversation.
        #[arg(long, conflicts_with = "question")]
        clear: bool,
    },
    /// The Feed: open mail as cards, one thing to do each, in order (now, today, this
    /// week, waiting on others, can wait).
    Feed {
        /// Only this project's cards.
        #[arg(long)]
        project: Option<String>,
    },
    /// Put a thread off until a time said in plain words ("vendredi soir", "friday 7pm",
    /// "dans 3 jours", "15 nov"; default tomorrow morning). It leaves its column and comes
    /// back Unread then, or when new mail arrives. "none" brings it back now.
    Later { thread: String, when: Vec<String> },
    /// Ask Mermoz to do something to many threads in plain words ("archive all messages
    /// past 5 months not in done"). The account's AI turns it into a plan; Mermoz shows
    /// what it would change and how many threads. Dry run unless --yes. Nothing here can
    /// send, reply to or forward mail.
    ///   mermoz do "archive everything from news.example older than a month"
    ///   mermoz do "archive everything from news.example older than a month" --yes
    ///   mermoz do "delete the promos from shop.example" --yes --confirm 37
    ///   mermoz do --plan plan.json            A plan written by hand (see docs/protocol.md)
    ///   mermoz do --undo bop_8f2c             Put every thread of a run back
    #[command(verbatim_doc_comment)]
    Do {
        /// What to do, in plain words.
        sentence: Vec<String>,
        /// Run the plan (otherwise only show it).
        #[arg(long)]
        yes: bool,
        /// Deleting needs the number of threads, as the preview shows it.
        #[arg(long)]
        confirm: Option<i64>,
        /// A plan as JSON (a file, or - for stdin) instead of asking the AI.
        #[arg(long, conflicts_with = "undo")]
        plan: Option<String>,
        /// Undo a run by its id.
        #[arg(long, conflicts_with_all = ["sentence", "yes"])]
        undo: Option<String>,
        /// The last runs, newest first.
        #[arg(long, conflicts_with_all = ["sentence", "yes", "undo", "plan"])]
        history: bool,
    },
    /// Instant sorting: the provider in use, whether it has a key, and today's calls.
    /// Keys and the provider are changed in the app (Settings › Developer).
    Router,
    /// Trigger an immediate sync.
    Sync,
    /// Run an MCP server on stdio.
    Mcp,
}

#[derive(Subcommand)]
enum ConversationCmd {
    /// Take a thread out of its conversation for good.
    Split { thread: String },
    /// Join the conversations of these threads into one.
    Merge {
        #[arg(required = true, num_args = 2..)]
        threads: Vec<String>,
    },
}
#[derive(Subcommand)]
enum DriveCmd {
    /// Drive files linked in a thread, shared with its people, and attachments saved.
    Files { thread: String },
    /// A Drive file's text (Docs as Markdown, Sheets as CSV, Slides as text).
    Read {
        file_id: String,
        /// Account id or email (default: the first account).
        #[arg(long)]
        account: Option<String>,
    },
    /// Search Drive by name and content; no words lists recently opened files.
    Search {
        words: Vec<String>,
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Save an attachment to Drive, in Mermoz/<thread's project>.
    Save { attachment: String },
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
        /// Attach a file; repeat for several.
        #[arg(long, value_name = "PATH")]
        attach: Vec<String>,
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
        /// Attach a file; repeat for several.
        #[arg(long, value_name = "PATH")]
        attach: Vec<String>,
    },
    /// Add files to a draft.
    Attach {
        id: String,
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<String>,
    },
    /// Remove an attachment from a draft (ids are shown by `draft show`).
    Detach { id: String, attachment: String },
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

/// A previewed plan: each step's sentence, how it was read, counts per status and a sample.
fn print_plan(out: &Out, v: &Value) {
    for st in v["steps"].as_array().into_iter().flatten() {
        out.line(st["summary"].as_str().unwrap_or(""));
        out.line(format!(
            "  I understood: {}",
            st["interpretation"].as_str().unwrap_or("")
        ));
        if let Some(r) = st["resolved"].as_str() {
            out.line(format!("  senders: {r}"));
        }
        let counts: Vec<String> = st["by_status"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| format!("{} {}", c["name"].as_str().unwrap_or(""), c["count"]))
            .collect();
        if !counts.is_empty() {
            out.line(format!("  currently: {}", counts.join(" · ")));
        }
        for t in st["sample"].as_array().into_iter().flatten() {
            out.line(format!(
                "  {:<8} {}  ({})",
                t["number"].as_str().unwrap_or(""),
                t["subject"].as_str().unwrap_or(""),
                t["last_from"].as_str().unwrap_or("")
            ));
        }
    }
    if let Some(w) = v["warning"].as_str() {
        out.line(format!("note: {w}"));
    }
    print_choices(out, v);
}

/// Candidates the user can pick to re-plan ("Did you mean …").
fn print_choices(out: &Out, v: &Value) {
    let choices: Vec<&Value> = v["choices"].as_array().into_iter().flatten().collect();
    if choices.is_empty() {
        return;
    }
    out.line("did you mean:");
    for c in choices {
        out.line(format!(
            "  {}   ({})",
            c["label"].as_str().unwrap_or(""),
            c["pick"]
        ));
    }
}

/// Absolute paths for the daemon, which does not share the caller's working directory.
fn attach_paths(paths: &[String]) -> Result<Vec<String>, RpcError> {
    paths
        .iter()
        .map(|p| {
            std::fs::canonicalize(p)
                .map(|a| a.to_string_lossy().into_owned())
                .map_err(|e| RpcError {
                    code: -32004,
                    message: format!("cannot attach {p}: {e}"),
                    data: None,
                })
        })
        .collect()
}

/// `accounts.add_gmail` params: the user's own sign-in client, when given.
fn add_account_params(
    client_json: Option<&std::path::Path>,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<Value, RpcError> {
    if let Some(p) = client_json {
        let raw = std::fs::read_to_string(p).map_err(|e| RpcError {
            code: -32602,
            message: format!("cannot read {}: {e}", p.display()),
            data: None,
        })?;
        return Ok(json!({ "client_json": raw }));
    }
    Ok(match (client_id, client_secret) {
        (Some(id), Some(secret)) => json!({ "client_id": id, "client_secret": secret }),
        _ => json!({}),
    })
}

/// A failed sign-in in plain words; Google's refusals add the client ID an admin
/// allow-lists and where.
fn sign_in_report(status: &Value) -> String {
    let error = status["error"].as_str().unwrap_or("sign-in failed");
    match status["problem"]["client_id"].as_str() {
        Some(id) => format!(
            "{error}\nClient ID: {id}\nAdmins trust it in the Google Admin console › Security › API controls › App access control.\nOwn client: mermoz add-account --client-json PATH"
        ),
        None => error.to_string(),
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

/// One chat turn; proposals print as the command that would apply them.
fn print_chat(out: &Out, m: &Value) {
    let who = if m["role"] == "assistant" {
        "AI"
    } else {
        "You"
    };
    out.line(format!("── {who}"));
    out.line(m["text"].as_str().unwrap_or("").trim_end());
    if m["searched"] == true {
        out.line("(searched the web)");
    }
    for a in m["actions"].as_array().into_iter().flatten() {
        let hint = match a["kind"].as_str().unwrap_or("") {
            "status" => format!(
                "mermoz status <thread> {}",
                a["status"].as_str().unwrap_or("")
            ),
            "project" => format!(
                "mermoz project <thread> \"{}\"",
                a["project"].as_str().unwrap_or("")
            ),
            "follow_up" => format!("mermoz due <thread> {}", a["due"].as_str().unwrap_or("")),
            "draft_reply" => format!(
                "mermoz draft reply <thread> --body \"{}\"",
                a["body"].as_str().unwrap_or("").replace('"', "\\\"")
            ),
            _ => String::new(),
        };
        out.line(format!(
            "  → {}  ({hint})",
            a["label"].as_str().unwrap_or("")
        ));
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
        let n = t["conversation_threads"].as_i64().unwrap_or(1);
        if n > 1 {
            out.line(format!("{:<8} ({n} threads in this conversation)", ""));
        }
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

/// Label by name (case-insensitive) or id; returns (account_id, label_id).
fn resolve_label(c: &mut Client, name: &str) -> Result<(String, String), RpcError> {
    let labels = c.call("labels.list", json!({}))?;
    for l in labels.as_array().into_iter().flatten() {
        if l["id"].as_str() == Some(name)
            || l["name"]
                .as_str()
                .map(|n| n.eq_ignore_ascii_case(name))
                .unwrap_or(false)
        {
            return Ok((
                l["account_id"].as_str().unwrap_or("").into(),
                l["id"].as_str().unwrap_or("").into(),
            ));
        }
    }
    Err(RpcError {
        code: -32004,
        message: format!("no label named '{name}'; see `mermoz labels`"),
        data: None,
    })
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

/// One Drive file per line: id, type, name, link.
fn drive_line(f: &Value) -> String {
    format!(
        "{}  {:<13} {}  {}",
        f["id"].as_str().unwrap_or(""),
        f["kind"].as_str().unwrap_or(""),
        f["name"].as_str().unwrap_or(""),
        f["web_link"].as_str().unwrap_or("")
    )
}

/// Human output for every `drive` command: whichever parts the answer has.
fn print_drive(out: &Out, v: &Value) {
    if let Some(t) = v.get("text") {
        out.line(
            t.as_str()
                .unwrap_or("(this file has no text Mermoz can read)"),
        );
        return;
    }
    if let Some(folder) = v["folder"].as_str() {
        out.line(format!(
            "saved in {folder}: {}",
            v["web_link"].as_str().unwrap_or("")
        ));
        return;
    }
    for l in v["linked"].as_array().into_iter().flatten() {
        match l["file"].as_object() {
            Some(_) => out.line(format!("linked  {}", drive_line(&l["file"]))),
            None => out.line(format!(
                "linked  {}  ({})",
                l["url"].as_str().unwrap_or(""),
                l["access"].as_str().unwrap_or("")
            )),
        }
    }
    for f in v["shared"].as_array().into_iter().flatten() {
        out.line(format!("shared  {}", drive_line(f)));
    }
    for s in v["saved"].as_array().into_iter().flatten() {
        out.line(format!(
            "saved   {}  {}  {}",
            s["name"].as_str().unwrap_or(""),
            s["folder"].as_str().unwrap_or(""),
            s["web_link"].as_str().unwrap_or("")
        ));
    }
    for f in v["files"].as_array().into_iter().flatten() {
        out.line(drive_line(f));
    }
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
    for a in d["attachments"].as_array().into_iter().flatten() {
        let what = match a["url"].as_str() {
            Some(u) => u.to_string(),
            None => format!("{} bytes", a["size"].as_i64().unwrap_or(0)),
        };
        out.line(format!(
            "Attached: {}  ({})  {}",
            a["filename"].as_str().unwrap_or(""),
            what,
            a["id"].as_str().unwrap_or("")
        ));
    }
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
                let id = a["id"].as_str().unwrap_or("");
                // Google refused the sign-in: nothing syncs until the user signs in again.
                // Otherwise it still syncs, but the next consent needs the app's own client.
                let hint = if a["sign_in_expired"] == true {
                    format!(
                        "  not syncing, Google asks you to sign in again: mermoz reconnect {id}"
                    )
                } else if a["needs_reconnect"] == true {
                    format!("  sign in again: mermoz reconnect {id}")
                } else {
                    String::new()
                };
                out.line(format!(
                    "{}  {}  {} ({}/{}){hint}",
                    id,
                    a["email"].as_str().unwrap_or(""),
                    s["state"].as_str().unwrap_or(""),
                    s["synced"],
                    s["total"]
                ));
            }
        }
        Cmd::Reconnect { account } => {
            let v = c.call("accounts.reconnect", json!({ "account_id": account }))?;
            let url = v["auth_url"].as_str().unwrap_or("");
            let _ = std::process::Command::new("open").arg(url).status();
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(format!(
                "Opened the browser to sign in again. If it didn't open:\n{url}"
            ));
        }
        Cmd::AddAccount {
            client_json,
            client_id,
            client_secret,
            outlook,
            wait,
        } => {
            let v = if outlook {
                c.call("accounts.add_outlook", json!({}))?
            } else {
                let params = add_account_params(client_json.as_deref(), client_id, client_secret)?;
                c.call("accounts.add_gmail", params)?
            };
            let url = v["auth_url"].as_str().unwrap_or("");
            let _ = std::process::Command::new("open").arg(url).status();
            if !wait {
                if out.json {
                    out.json(&v);
                } else {
                    out.line(format!(
                        "Opened the browser to sign in. If it didn't open:\n{url}"
                    ));
                }
                return Ok(());
            }
            if !out.json {
                out.line(format!(
                    "Opened the browser to sign in. If it didn't open:\n{url}"
                ));
            }
            let status = loop {
                let s = c.call("accounts.sign_in_status", json!({}))?;
                if s["running"] != true {
                    break s;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            };
            if status["error"].is_string() {
                return Err(RpcError {
                    code: -32000,
                    message: sign_in_report(&status),
                    data: Some(status),
                });
            }
            if out.json {
                out.json(&status);
            } else {
                out.line("Signed in. Mermoz is syncing the account.");
            }
        }
        Cmd::List {
            status,
            project,
            account,
            label,
            conversations,
            feeds,
            limit,
        } => {
            let mut p = json!({ "limit": limit, "conversations": conversations || feeds });
            if feeds {
                p["kind"] = json!(["feed", "list"]);
            }
            if let Some(s) = status {
                let keys: Vec<String> = s
                    .split(',')
                    .map(|x| resolve_status(&mut c, x.trim()))
                    .collect::<Result<_, _>>()?;
                p["status"] = json!(keys);
            }
            if let Some(pr) = project {
                p["project"] = json!(resolve_project(&mut c, &pr, false)?);
            }
            if let Some(a) = account {
                p["account_id"] = json!(resolve_account(&mut c, &a)?);
            }
            if let Some(l) = label {
                let (acc, id) = resolve_label(&mut c, &l)?;
                p["label_id"] = json!(id);
                p["account_id"] = json!(acc);
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
        Cmd::Search {
            query,
            limit,
            gmail: true,
        } => {
            let v = c.call(
                "search.gmail",
                json!({ "query": query.join(" "), "limit": limit }),
            )?;
            if out.json {
                out.json(&v);
                return Ok(());
            }
            let hits = v["results"].as_array().cloned().unwrap_or_default();
            if hits.is_empty() {
                out.line("No matches on Gmail either.");
            }
            for h in hits {
                let here = if h["in_mermoz"] == true {
                    ""
                } else {
                    "  (on Gmail)"
                };
                out.line(format!(
                    "{}  {}  {}{here}",
                    h["date"].as_str().unwrap_or(""),
                    h["from"].as_str().unwrap_or(""),
                    h["subject"].as_str().unwrap_or("")
                ));
            }
        }
        Cmd::Search { query, limit, .. } => {
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
            if v["threads"].as_array().is_none_or(|a| a.is_empty()) {
                out.line(
                    "No matches in Mermoz. Search all of Gmail: mermoz search --gmail <words>",
                );
            }
            print_threads(&out, &v["threads"]);
        }
        Cmd::Conversation(cmd) => {
            let v = match cmd {
                ConversationCmd::Split { thread } => {
                    c.call("conversations.split", json!({ "id": thread }))?
                }
                ConversationCmd::Merge { threads } => {
                    c.call("conversations.merge", json!({ "ids": threads }))?
                }
            };
            if out.json {
                out.json(&v);
            } else {
                out.line(format!(
                    "conversation {}",
                    v["conversation_id"].as_str().unwrap_or("")
                ));
            }
        }
        Cmd::Files { query, limit } => {
            let q = query.join(" ");
            let v = c.call(
                "files.list",
                json!({ "query": if q.is_empty() { Value::Null } else { json!(q) }, "limit": limit }),
            )?;
            if out.json {
                out.json(&v);
                return Ok(());
            }
            let files = v.as_array().cloned().unwrap_or_default();
            if files.is_empty() {
                out.line("No files.");
            }
            for f in files {
                out.line(format!(
                    "{:<24} {:<18} {}  {} ({} bytes)  thread {}",
                    f["id"].as_str().unwrap_or(""),
                    truncate(f["from"].as_str().unwrap_or(""), 18),
                    fmt_date(f["date"].as_i64().unwrap_or(0)),
                    f["filename"].as_str().unwrap_or(""),
                    f["size"],
                    f["thread_id"].as_str().unwrap_or("")
                ));
            }
        }
        Cmd::Thread { id, conversation } => {
            let v = c.call(
                "threads.get",
                json!({ "id": id, "conversation": conversation }),
            )?;
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
            let projects: Vec<&str> = t["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str())
                .collect();
            out.line(format!(
                "status: {} · priority: {} · due: {} · projects: {}",
                t["status"].as_str().unwrap_or(""),
                prio_name(t["priority"].as_i64().unwrap_or(0)),
                t["due"].as_str().unwrap_or("-"),
                if projects.is_empty() {
                    "-".to_string()
                } else {
                    projects.join(", ")
                }
            ));
            if let Some(sg) = t["suggestion"].as_object() {
                let sp: Vec<&str> = sg["projects"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.as_str())
                    .collect();
                out.line(format!(
                    "suggested: {}{} (mermoz accept {})",
                    sg["status"].as_str().unwrap_or("-"),
                    if sp.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", sp.join(", "))
                    },
                    t["number"].as_str().unwrap_or("")
                ));
            }
            let mut current = String::new();
            for m in v["messages"].as_array().into_iter().flatten() {
                // In a conversation, say where each Gmail thread starts.
                let tid = m["thread_id"].as_str().unwrap_or("").to_string();
                if conversation && tid != current {
                    let subject = v["conversation"]["threads"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|t| t["id"] == tid.as_str())
                        .map(|t| {
                            format!(
                                "{}  {}",
                                t["number"].as_str().unwrap_or(""),
                                t["subject"].as_str().unwrap_or("")
                            )
                        })
                        .unwrap_or_default();
                    out.line(format!("\n══ thread {subject}"));
                    current = tid;
                }
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
        Cmd::Project {
            thread,
            project,
            remove,
        } => {
            let p = if project.eq_ignore_ascii_case("none") {
                json!({ "ids": [thread], "projects": [] })
            } else {
                // Existing projects match case-insensitively; a new name creates the label.
                let name = resolve_project(&mut c, &project, false).unwrap_or(project.clone());
                let key = if remove {
                    "remove_projects"
                } else {
                    "add_projects"
                };
                json!({ "ids": [thread], key: [name] })
            };
            let v = c.call("threads.update", p)?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(if remove {
                format!("removed from {project}")
            } else {
                format!("project + {project}")
            });
        }
        Cmd::Accept {
            threads,
            column,
            to,
            to_projects,
        } => {
            let v = match column {
                None => c.call("threads.accept_suggestions", json!({ "ids": threads }))?,
                Some(col) => {
                    let status = resolve_status(&mut c, &col)?;
                    let target = if to.is_none() && to_projects.is_empty() {
                        Value::Null
                    } else {
                        let to = match to.as_deref() {
                            None | Some("none") => Value::Null,
                            Some(t) => json!(resolve_status(&mut c, t)?),
                        };
                        json!({ "status": to, "projects": to_projects })
                    };
                    let mut v = c.call(
                        "suggestions.accept",
                        json!({ "status": status, "target": target }),
                    )?;
                    // Snapshots are for the app's undo; agents only need the count.
                    if let Some(o) = v.as_object_mut() {
                        o.remove("before");
                    }
                    v
                }
            };
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(format!("accepted {}", v["accepted"].as_i64().unwrap_or(0)));
        }
        Cmd::Reject { threads } => {
            let v = c.call("threads.reject_suggestions", json!({ "ids": threads }))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(format!("rejected {}", v["rejected"].as_i64().unwrap_or(0)));
        }
        Cmd::Resort { stop } => {
            let v = c.call("suggestions.resort", json!({ "stop": stop }))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(format!(
                "stopped; cleared {} unreviewed suggestions",
                v["cleared"].as_i64().unwrap_or(0)
            ));
        }
        Cmd::Suggestions {
            status,
            clear,
            ask,
            to,
            to_projects,
            limit,
        } => {
            if clear {
                let v = c.call("suggestions.clear", json!({}))?;
                if out.json {
                    return {
                        out.json(&v);
                        Ok(())
                    };
                }
                out.line(format!(
                    "cleared {} unreviewed suggestions",
                    v["cleared"].as_i64().unwrap_or(0)
                ));
                return Ok(());
            }
            let status = resolve_status(&mut c, &status.unwrap_or_default())?;
            if ask {
                let v = c.call(
                    "threads.suggest_column",
                    json!({ "status": status, "limit": limit }),
                )?;
                // --json prints only the listing that follows.
                if !out.json {
                    if let Some(why) = v["paused"].as_str() {
                        out.line(format!("the AI is on hold ({why})"));
                    }
                    out.line(format!(
                        "asked about {} threads, {} now have a suggestion",
                        v["asked"].as_u64().unwrap_or(0),
                        v["suggested"].as_array().map_or(0, |a| a.len())
                    ));
                }
            }
            if to.is_some() || !to_projects.is_empty() {
                let to = match to.as_deref() {
                    None | Some("none") => Value::Null,
                    Some(t) => json!(resolve_status(&mut c, t)?),
                };
                let v = c.call(
                    "threads.list",
                    json!({ "status": [status], "suggested": { "status": to, "projects": to_projects }, "limit": limit }),
                )?;
                if out.json {
                    return {
                        out.json(&v);
                        Ok(())
                    };
                }
                print_threads(&out, &v["threads"]);
                return Ok(());
            }
            let v = c.call("suggestions.summary", json!({ "statuses": [status] }))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            let col = &v["columns"][0];
            out.line(format!(
                "{} pending{}",
                col["pending"].as_i64().unwrap_or(0),
                paused_words(
                    v["paused"].as_str(),
                    v["provider_name"].as_str().unwrap_or("TypeSafe")
                )
            ));
            for g in col["groups"].as_array().into_iter().flatten() {
                let mut to: Vec<String> =
                    g["status"].as_str().map(String::from).into_iter().collect();
                to.extend(
                    g["projects"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| p.as_str().map(|p| format!("+{p}"))),
                );
                out.line(format!(
                    "{:>6}  → {}",
                    g["count"].as_i64().unwrap_or(0),
                    to.join(" ")
                ));
            }
        }
        Cmd::Feed { project } => {
            let v = c.call("feed.list", json!({ "project": project }))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            for s in v["sections"].as_array().into_iter().flatten() {
                out.line(s["title"].as_str().unwrap_or(""));
                for card in s["cards"].as_array().into_iter().flatten() {
                    let mut extra = vec![];
                    if let Some(d) = card["due"].as_str() {
                        extra.push(format!("due {d}"));
                    }
                    if let Some(a) = card["amount"].as_str() {
                        extra.push(a.to_string());
                    }
                    let n = card["members"].as_array().map_or(0, |m| m.len());
                    out.line(format!(
                        "  {:<8} {:<9} {}{}",
                        if n > 0 {
                            format!("{n} mails")
                        } else {
                            card["thread"]["number"].as_str().unwrap_or("").to_string()
                        },
                        card["kind"].as_str().unwrap_or(""),
                        truncate(card["title"].as_str().unwrap_or(""), 70),
                        if extra.is_empty() {
                            String::new()
                        } else {
                            format!(" · {}", extra.join(" · "))
                        }
                    ));
                }
            }
            if v["sorting"] == true {
                out.line(format!(
                    "(sorting: {} of {} judged)",
                    v["judged"], v["open"]
                ));
            }
        }
        Cmd::Later { thread, when } => {
            let text = when.join(" ");
            if text.eq_ignore_ascii_case("none") {
                let v = c.call("feed.unsnooze", json!({ "ids": [thread] }))?;
                if out.json {
                    out.json(&v);
                } else {
                    out.line("back now");
                }
                return Ok(());
            }
            // Empty means the picker's "Tomorrow morning".
            let at = if text.trim().is_empty() {
                c.call("later.options", json!({}))?
                    .as_array()
                    .and_then(|o| o.iter().find(|o| o["key"] == "t").cloned())
                    .unwrap_or(Value::Null)
            } else {
                c.call("later.parse", json!({ "text": text }))?
            };
            let (Some(until), Some(label)) = (at["until"].as_i64(), at["when"].as_str()) else {
                return Err(RpcError {
                    code: -32602,
                    message: format!(
                        "'{text}' doesn't say when: try \"vendredi soir\", \"friday 7pm\", \"dans 3 jours\" or \"15 nov\""
                    ),
                    data: None,
                });
            };
            let v = c.call(
                "feed.snooze",
                json!({ "ids": [thread], "until": until, "label": label }),
            )?;
            if out.json {
                out.json(&v);
            } else {
                out.line(format!("later: back {label}"));
            }
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
        Cmd::Rename {
            thread,
            title,
            clear: _,
        } => {
            let v = c.call("threads.rename", json!({ "id": thread, "title": title }))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            out.line(match v["thread"]["title"].as_str() {
                Some(t) => format!("title → {t}"),
                None => format!(
                    "title → subject ({})",
                    v["thread"]["subject"].as_str().unwrap_or("")
                ),
            });
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
            DraftCmd::Reply {
                thread,
                body,
                file,
                attach,
            } => {
                let body = read_body(body, file)?;
                let v = c.call(
                    "drafts.create",
                    json!({ "thread_id": thread, "body": body, "attachments": attach_paths(&attach)? }),
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
                attach,
            } => {
                let body = read_body(body, file)?;
                let mut p = json!({ "to": to, "cc": cc, "subject": subject, "body": body, "attachments": attach_paths(&attach)? });
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
            DraftCmd::Attach { id, paths } => {
                let v = c.call(
                    "drafts.attach",
                    json!({ "id": id, "attachments": attach_paths(&paths)? }),
                )?;
                if out.json {
                    return {
                        out.json(&v);
                        Ok(())
                    };
                }
                print_draft(&out, &v);
            }
            DraftCmd::Detach { id, attachment } => {
                let v = c.call(
                    "drafts.detach",
                    json!({ "id": id, "attachment_id": attachment }),
                )?;
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
        Cmd::Labels => {
            let v = c.call("labels.list", json!({}))?;
            if out.json {
                out.json(&v);
                return Ok(());
            }
            for l in v.as_array().into_iter().flatten() {
                out.line(format!(
                    "{:>6}  {}",
                    l["count"],
                    l["name"].as_str().unwrap_or("")
                ));
            }
        }
        Cmd::Contacts { query, limit } => {
            let q = query.join(" ");
            let v = c.call("contacts.list", json!({ "query": if q.is_empty() { Value::Null } else { json!(q) }, "limit": limit }))?;
            if out.json {
                out.json(&v);
                return Ok(());
            }
            for p in v.as_array().into_iter().flatten() {
                out.line(format!(
                    "{:<28} {:<34} {}",
                    truncate(p["name"].as_str().unwrap_or(""), 28),
                    p["email"].as_str().unwrap_or(""),
                    [
                        p["title"].as_str().unwrap_or(""),
                        p["company"].as_str().unwrap_or("")
                    ]
                    .iter()
                    .filter(|x| !x.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" · ")
                ));
            }
        }
        Cmd::Contact { email } => {
            let v = c.call("contacts.get", json!({ "email": email }))?;
            if out.json {
                out.json(&v);
                return Ok(());
            }
            let p = &v["contact"];
            out.line(format!(
                "{} <{}>",
                p["name"].as_str().unwrap_or(""),
                p["email"].as_str().unwrap_or("")
            ));
            for k in ["title", "company", "address", "website"] {
                if let Some(x) = p[k].as_str().filter(|x| !x.is_empty()) {
                    out.line(format!("{k}: {x}"));
                }
            }
            for k in ["phones", "socials"] {
                for x in p[k]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str())
                {
                    out.line(format!("{}: {x}", k.trim_end_matches('s')));
                }
            }
            out.line("");
            print_threads(&out, &v["threads"]);
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
        Cmd::Do {
            sentence,
            yes,
            confirm,
            plan,
            undo,
            history,
        } => {
            let invalid = |m: String| RpcError {
                code: -32602,
                message: m,
                data: None,
            };
            if history {
                let v = c.call("do.list", json!({}))?;
                if out.json {
                    out.json(&v);
                } else {
                    for o in v.as_array().into_iter().flatten() {
                        out.line(format!(
                            "{}  {:<7} {}",
                            o["id"].as_str().unwrap_or(""),
                            o["state"].as_str().unwrap_or(""),
                            o["summary"].as_str().unwrap_or("")
                        ));
                    }
                }
                return Ok(());
            }
            if let Some(id) = undo {
                let v = c.call("do.undo", json!({ "id": id }))?;
                if out.json {
                    out.json(&v);
                } else {
                    out.line(format!("undone: {} threads put back", v["restored"]));
                }
                return Ok(());
            }
            let text = sentence.join(" ");
            let v = match plan {
                Some(path) => {
                    let raw = if path == "-" {
                        let mut s = String::new();
                        std::io::stdin()
                            .read_to_string(&mut s)
                            .map_err(|e| invalid(format!("cannot read stdin: {e}")))?;
                        s
                    } else {
                        std::fs::read_to_string(&path)
                            .map_err(|e| invalid(format!("cannot read {path}: {e}")))?
                    };
                    let plan: Value = serde_json::from_str(&raw)
                        .map_err(|e| invalid(format!("{path} is not JSON: {e}")))?;
                    c.call("do.preview", json!({ "plan": plan, "text": text }))?
                }
                None if text.trim().is_empty() => {
                    return Err(invalid(
                        "say what to do, e.g. mermoz do \"archive newsletters older than a month\""
                            .into(),
                    ));
                }
                None => c.call("do.ask", json!({ "text": text }))?,
            };
            if v["kind"] != "plan" {
                // No AI, or the AI asked back: nothing was planned.
                if out.json {
                    out.json(&v);
                } else {
                    print_choices(&out, &v);
                }
                return Err(invalid(
                    v["message"]
                        .as_str()
                        .unwrap_or("nothing planned")
                        .to_string(),
                ));
            }
            if !yes {
                if out.json {
                    out.json(&v);
                } else {
                    print_plan(&out, &v);
                    out.line(if v["deletes"] == true {
                        format!("dry run: nothing changed. To delete, run again with --yes --confirm {}", v["total"])
                    } else {
                        "dry run: nothing changed. Run again with --yes to do it.".into()
                    });
                }
                return Ok(());
            }
            let r = c.call("do.run", json!({ "id": v["id"], "confirm": confirm }))?;
            if out.json {
                out.json(&json!({ "plan": v, "run": r }));
            } else {
                print_plan(&out, &v);
                out.line(format!(
                    "done: {} threads changed. Undo: mermoz do --undo {}",
                    r["done"],
                    r["id"].as_str().unwrap_or("")
                ));
            }
        }
        Cmd::Chat {
            thread,
            question,
            no_research,
            clear,
        } => {
            let v = if clear {
                c.call("chat.clear", json!({ "thread_id": thread }))?
            } else if question.is_empty() {
                c.call("chat.get", json!({ "thread_id": thread }))?
            } else {
                c.call(
                    "chat.send",
                    json!({ "thread_id": thread, "text": question.join(" "), "research": !no_research }),
                )?
            };
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            if clear {
                out.line("conversation cleared");
            } else if question.is_empty() {
                for m in v["messages"].as_array().into_iter().flatten() {
                    print_chat(&out, m);
                }
            } else {
                print_chat(&out, &v);
            }
        }
        Cmd::Drive(d) => {
            let account = |c: &mut Client, a: Option<String>| -> Result<String, RpcError> {
                match a {
                    Some(a) => resolve_account(c, &a),
                    None => c.call("accounts.list", json!({}))?[0]["id"]
                        .as_str()
                        .map(String::from)
                        .ok_or_else(|| RpcError {
                            code: -32004,
                            message: "no account connected".into(),
                            data: None,
                        }),
                }
            };
            let v = match d {
                DriveCmd::Files { thread } => {
                    c.call("drive.files_for_thread", json!({ "thread_id": thread }))?
                }
                DriveCmd::Read {
                    file_id,
                    account: a,
                } => {
                    let acc = account(&mut c, a)?;
                    c.call(
                        "drive.content",
                        json!({ "account_id": acc, "file_id": file_id }),
                    )?
                }
                DriveCmd::Search {
                    words,
                    account: a,
                    limit,
                } => {
                    let acc = account(&mut c, a)?;
                    c.call(
                        "drive.search",
                        json!({ "account_id": acc, "query": words.join(" "), "limit": limit }),
                    )?
                }
                DriveCmd::Save { attachment } => c.call(
                    "drive.save_attachment",
                    json!({ "attachment_id": attachment }),
                )?,
            };
            if out.json {
                out.json(&v);
                return Ok(());
            }
            print_drive(&out, &v);
        }
        Cmd::Router => {
            let v = c.call("router.get", json!({}))?;
            if out.json {
                return {
                    out.json(&v);
                    Ok(())
                };
            }
            for line in router_lines(&v) {
                out.line(line);
            }
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

/// " · sorting paused: …" after a pending count; empty when not paused.
fn paused_words(reason: Option<&str>, provider: &str) -> String {
    match reason {
        Some("credits") => format!(" · sorting paused: your {provider} credits ran out"),
        Some(_) => " · sorting paused: AI unavailable, retrying later".into(),
        None => String::new(),
    }
}

/// `router.get` for people: the provider in use first, then each provider's key.
fn router_lines(v: &Value) -> Vec<String> {
    let name = v["provider_name"].as_str().unwrap_or("TypeSafe");
    let state = if !v["configured"].as_bool().unwrap_or(false) {
        "no key".to_string()
    } else if let Some(r) = v["paused"].as_str() {
        paused_words(Some(r), name).trim_start_matches(" · ").into()
    } else {
        "on".into()
    };
    let mut lines = vec![format!(
        "provider: {name} ({}) · {state}",
        v["model"].as_str().unwrap_or("")
    )];
    for p in v["providers"].as_array().into_iter().flatten() {
        lines.push(format!(
            "  {:<9} {:<7} {} calls today{}",
            p["name"].as_str().unwrap_or(""),
            if p["configured"].as_bool().unwrap_or(false) {
                "key"
            } else {
                "no key"
            },
            p["calls_today"].as_i64().unwrap_or(0),
            p["error"]
                .as_str()
                .map(|e| format!(" · {e}"))
                .unwrap_or_default()
        ));
    }
    if let Some(m) = v["offer"]["message"].as_str() {
        lines.push(format!("{m} (Settings › Developer in Mermoz)"));
    }
    lines
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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::name("high", 2)]
    #[case::uppercase("URGENT", 1)]
    #[case::digit("4", 4)]
    #[case::none("none", 0)]
    fn priority_parses(#[case] input: &str, #[case] want: i64) {
        assert_eq!(parse_priority(input).unwrap(), want);
    }

    #[rstest]
    fn bad_priority_lists_valid_values() {
        assert!(
            parse_priority("max")
                .unwrap_err()
                .message
                .contains("urgent, high")
        );
    }

    #[rstest]
    #[case::short("abc", 5, "abc")]
    #[case::long("abcdefgh", 5, "abcd…")]
    fn truncation(#[case] input: &str, #[case] n: usize, #[case] want: &str) {
        assert_eq!(truncate(input, n), want);
    }

    fn router_state(provider: &str, configured: bool, paused: Option<&str>) -> Value {
        json!({
            "provider_name": provider,
            "model": "subq-snap-preview",
            "configured": configured,
            "paused": paused,
            "providers": [],
        })
    }

    #[rstest]
    #[case::on("SubQ", true, None, "provider: SubQ (subq-snap-preview) · on")]
    #[case::no_key("SubQ", false, None, "provider: SubQ (subq-snap-preview) · no key")]
    #[case::out_of_credits(
        "SubQ",
        true,
        Some("credits"),
        "provider: SubQ (subq-snap-preview) · sorting paused: your SubQ credits ran out"
    )]
    fn router_names_the_provider_first(
        #[case] provider: &str,
        #[case] configured: bool,
        #[case] paused: Option<&str>,
        #[case] want: &str,
    ) {
        assert_eq!(
            router_lines(&router_state(provider, configured, paused))[0],
            want
        );
    }

    #[rstest]
    fn router_lists_each_provider_s_key() {
        let mut v = router_state("TypeSafe", true, None);
        v["providers"] = json!([{ "name": "SubQ", "configured": true, "calls_today": 12 }]);
        assert_eq!(router_lines(&v)[1], "  SubQ      key     12 calls today");
    }

    #[rstest]
    fn router_passes_on_the_offer() {
        let mut v = router_state("TypeSafe", true, Some("credits"));
        v["offer"] = json!({ "message": "TypeSafe credits ran out. Use SubQ instead?" });
        assert_eq!(
            router_lines(&v)[1],
            "TypeSafe credits ran out. Use SubQ instead? (Settings › Developer in Mermoz)"
        );
    }

    #[rstest]
    fn missing_body_explains_flags() {
        assert!(
            read_body(None, None)
                .unwrap_err()
                .message
                .contains("--body")
        );
    }

    #[rstest]
    fn pasted_client_is_sent_as_ids() {
        assert_eq!(
            add_account_params(None, Some("company-cid".into()), Some("s".into())).unwrap(),
            json!({ "client_id": "company-cid", "client_secret": "s" })
        );
    }

    #[rstest]
    fn client_file_is_sent_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("client_secret.json");
        std::fs::write(&f, r#"{"installed":{"client_id":"company-cid"}}"#).unwrap();
        assert_eq!(
            add_account_params(Some(&f), None, None).unwrap()["client_json"],
            r#"{"installed":{"client_id":"company-cid"}}"#
        );
    }

    #[rstest]
    fn no_client_means_the_app_client() {
        assert_eq!(add_account_params(None, None, None).unwrap(), json!({}));
    }

    #[rstest]
    fn refusal_report_says_where_admins_trust_the_client() {
        let status = json!({ "error": "Your organization only allows apps it trusts.", "problem": { "client_id": "app-cid" } });
        assert!(sign_in_report(&status).contains("Client ID: app-cid"));
    }
}
