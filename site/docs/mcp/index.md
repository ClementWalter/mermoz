<!-- https://mermoz.xyz/docs/mcp/ · updated 2026-10-13 -->

# Mermoz MCP server

`mermoz mcp` runs an MCP server on **stdio** that gives AI agents the user's Mermoz mailbox as 26 tools: search, read, sort, draft, Drive files and more. It is local: it runs on the user's Mac, next to Mermoz.app, and talks to the app's daemon over a Unix socket. There is no hosted or remote (Streamable HTTP) endpoint: Mermoz has no server of its own that holds mail.

- **Requires:** Mermoz.app installed and running (macOS 26 or later), signed in to Gmail. If the app isn't running, tools answer "Mermoz isn't running. Ask the user to open Mermoz.app."
- **Command:** `mermoz mcp` once the CLI is in PATH (Mermoz › Settings › Developer › Install in PATH), or the full path `/Applications/Mermoz.app/Contents/Resources/mermoz mcp`.
- **Protocol version:** 2025-06-18; capability `tools`.
- **Sending:** `send_draft` moves the draft to the user's Review lane and returns `approval_required` unless the user enabled agent sending. Tell the user; do not retry.

## Add Mermoz to Claude Code

```sh
claude mcp add mermoz -- mermoz mcp
# for every project: claude mcp add --scope user mermoz -- mermoz mcp
```

## Add Mermoz to Claude Desktop

Claude Desktop does not read your shell's PATH, so use the full path. In Claude Desktop, **Settings › Developer › Edit Config** opens `~/Library/Application Support/Claude/claude_desktop_config.json`; add:

```json
{
  "mcpServers": {
    "mermoz": {
      "command": "/Applications/Mermoz.app/Contents/Resources/mermoz",
      "args": ["mcp"]
    }
  }
}
```

Then quit and reopen Claude Desktop.

## Add Mermoz to Codex

```sh
codex mcp add mermoz -- mermoz mcp
```

Or in `~/.codex/config.toml`:

```toml
[mcp_servers.mermoz]
command = "mermoz"
args = ["mcp"]
```

## Any other MCP client

Launch `mermoz mcp` (or `/Applications/Mermoz.app/Contents/Resources/mermoz mcp`) as a stdio server: newline-delimited JSON-RPC 2.0 on stdin and stdout. Discovery documents: [server.json](/server.json) (official MCP registry format) and [/.well-known/mcp.json](/.well-known/mcp.json).

## Tools

### search_mail

Full-text search across all local mail (instant, offline). Supports from:, to:, subject: prefixes. Returns threads best match first.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `limit` | integer | no | (default 20) |
| `query` | string | yes | search words |

### list_threads

List threads (issues) filtered by status/project/account, newest first. Statuses: unread, todo, waiting, in_progress, backlog, review, done, archive, deleted. Projects are the user's Gmail labels; each thread lists its `projects` and the AI's pending `suggestion`. `suggested`: true for threads with a pending suggestion, or {status, projects} for one destination listed by column_suggestions. `conversations`: one row per conversation (Gmail threads with the same people about the same matter, as the user sees them), each with `conversation_threads`; `kind`: ["feed", "list"] for automated senders and mailing lists.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `account_id` | string | no | account id |
| `conversations` | boolean | no |  |
| `kind` | array of string | no |  |
| `label_id` | string | no | Gmail label id from list_labels (pass its account_id too) |
| `limit` | integer | no | (default 50) |
| `project` | string | no | project (Gmail label) name |
| `status` | array of string | no |  |
| `suggested` | boolean or object | no |  |

### get_thread

Read a whole thread: every message's plain-text body, recipients and attachment ids. With `conversation`, every message of the conversation it belongs to (each with its `thread_id`), plus its people, files and links.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `conversation` | boolean | no |  |
| `id` | string | yes | thread id or number like M-1287 |

### split_conversation

Take a thread out of the conversation Mermoz grouped it into (when it is a different matter). Final: it is never grouped with them again.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `id` | string | yes | thread id or number |

### merge_conversations

Join the conversations of two or more threads into one (same people, same matter).

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `ids` | array of string | yes |  |

### list_files

Files shared in any conversation, newest first, with their attachment id (read with get_attachment) and thread.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `limit` | integer | no | (default 50) |
| `query` | string | no | part of a file name |

### update_thread

Change a thread's status, priority (0 none,1 urgent,2 high,3 normal,4 low), projects (Gmail labels: add_projects/remove_projects, or projects to replace) or due date (YYYY-MM-DD, null clears). 'deleted' moves the thread to Gmail Trash.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `add_projects` | array of string | no |  |
| `due` | string or null | no |  |
| `id` | string | yes | thread id or number |
| `priority` | integer | no |  |
| `projects` | array of string | no |  |
| `remove_projects` | array of string | no |  |
| `status` | string | no | status key |

### accept_suggestions

Accept the AI's pending suggestion (status, projects, priority) for threads; they count as read. Pass `ids`, or `column` (a status) to accept that column's pending suggestions — all of them, or only the group given by `to_status`/`to_projects` as listed by column_suggestions.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `column` | string | no | status key of the column |
| `ids` | array of string | no |  |
| `to_projects` | array of string | no |  |
| `to_status` | string or null | no | group's target status (null: groups that only add projects) |

### reject_suggestions

Turn down the AI's pending suggestion for threads: they stay where they are and are not suggested again.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `ids` | array of string | yes |  |

### column_suggestions

The AI's pending suggestions in status columns, grouped by target (status + projects) with counts, and how far background sorting has got (classified of total). Nothing moves until accepted.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `account_id` | string | no | limit to an account |
| `project` | string | no | limit to a project |
| `statuses` | array of string | yes |  |

### stop_resort

Stop a whole-mailbox sort and clear every suggestion on read threads that nobody reviewed. Accepted and turned-down suggestions are kept; Unread mail keeps its own.

No parameters.

### get_attachment

Get an attachment's local file path (readable offline); text attachments also return their content.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `id` | string | yes | attachment id from get_thread |

### create_draft

Create a draft. With thread_id it is a reply and recipients/subject default from the thread. Drafts never send by themselves. Attachments are copied into Mermoz; Gmail sends at most 25 MB of files per email.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `attachments` | array of string | no | files to attach: absolute paths |
| `body` | string | yes | plain text body |
| `cc` | array of string | no |  |
| `forward_attachments` | array of string | no | attachment ids from get_thread to send again (forwarding) |
| `subject` | string | no | subject |
| `thread_id` | string | no | thread to reply to |
| `to` | array of string | no |  |

### send_draft

Ask to send a draft. Unless the user enabled agent sending, the draft goes to the user's Review lane and this returns approval_required: tell the user, do not retry.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `id` | string | yes | draft id |

### search_contacts

Search the user's directory, built automatically from email signatures: names, titles, companies, phones, addresses, websites. Empty query lists recent contacts.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `limit` | integer | no | (default 20) |
| `query` | string | no | name, email or company |

### list_labels

The user's Gmail labels (mailboxes) with ids, accounts and thread counts.

No parameters.

### list_statuses

Workflow statuses.

No parameters.

### list_projects

Projects: the user's Gmail labels. A thread can be in several.

No parameters.

### chat_about_thread

Ask the user's own AI (the one allowed for the thread's account in Mermoz) about a thread; it sees the thread, its people, status and the user's earlier mail with them, and may search the web. The conversation is kept with the thread and shown in the app. Returns the answer and proposed actions (status, project, follow_up date, draft_reply body); nothing is applied: use update_thread or create_draft to act. Without a question, returns the conversation so far.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `id` | string | yes | thread id or number |
| `question` | string | no | what to ask |
| `research` | boolean | no | allow web search (default True) |

### thread_drive_files

Google Drive files of a thread: those linked in its messages (with access: ok, no_access or not_connected), files shared by or with its participants, and its attachments already saved to Drive. Needs the account's Drive connected in Mermoz.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `id` | string | yes | thread id or number |

### read_drive_file

A Google Drive file's text: Docs as Markdown, Sheets as CSV (first sheet), Slides as text, plain-text files as they are. Cached locally.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `account_id` | string | yes | account id |
| `file_id` | string | yes | Drive file id |

### search_drive

Search the account's Google Drive by name and content; without a query, the files the user opened most recently.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `account_id` | string | yes | account id |
| `limit` | integer | no | (default 20) |
| `query` | string | no | words to find |

### save_attachment_to_drive

Save an attachment to the user's Google Drive, in Mermoz/&lt;the thread's project&gt; (created if missing). Returns the folder and an Open in Drive link; saving again returns the same file. Sharing files stays with the user in the app.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `id` | string | yes | attachment id from get_thread |

### feed

The user's Feed: every open thread as a card (one thing to do), in sections now, today, week, waiting, later, ordered by deadline then urgency. Each card has a kind (reply, decision, sign, promise, event, payment, intro, waiting, cleanup, reading, other), plain-language reasons, the deadline and amount written in the mail, and the AI's words when written. Automated mail and newsletters come as batch cards listing their threads.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `project` | string | no | only this project's cards |

### snooze_thread

Later: hide threads from the Feed until a time (Unix ms) or until new mail arrives in them. until null brings them back now.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `ids` | array of string | yes |  |
| `until` | integer or null | no |  |

### submit_feedback

Report friction with these tools (missing option, confusing result, slowness) so Mermoz can improve.

| parameter | type | required | notes |
| --- | --- | --- | --- |
| `text` | string | yes | what went wrong or is missing |
| `tool` | string | no | tool name |
