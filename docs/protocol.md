# mermozd protocol

`mermozd` is the only process that touches the network and the database. The
app, the CLI and the MCP server are clients of this protocol.

## Transport

- Unix domain socket at `$MERMOZ_HOME/mermozd.sock`.
  `MERMOZ_HOME` defaults to `~/Library/Application Support/Mermoz`;
  `MERMOZ_SOCKET` overrides the socket path (macOS limits it to 104 bytes).
- JSON-RPC 2.0, one JSON object per line (`\n` terminated), UTF-8.
- Requests are answered in order per connection. A connection that called
  `subscribe` additionally receives notifications (no `id`) at any time.

## Conventions

- Timestamps: integer Unix milliseconds.
- Dates without time (due dates): `"YYYY-MM-DD"`.
- Priority: `0` none, `1` urgent, `2` high, `3` normal, `4` low.
- Status keys: `unread` (entry status), then for read mail `todo`, `waiting`,
  `in_progress`, `backlog`, `review` (an agent draft awaits approval), `done`,
  `archive`, `deleted` (Gmail Trash). `done`, `archive` and `deleted` are out of
  the inbox. Reading an `unread` thread moves it to the open status new mail
  interrupted, else the router's suggestion, else `todo`; new mail from someone
  else brings it back to `unread`.
- Projects are the user's Gmail labels, identified by name (nested with `/`).
  A thread can be in several.
- Thread `number` is a stable human id like `"M-1287"`; every method taking a
  thread `id` also accepts its number.

## Errors

| code   | name                | meaning                                              |
| ------ | ------------------- | ---------------------------------------------------- |
| -32601 | method_not_found    |                                                      |
| -32602 | invalid_params      | `message` says which param and why                   |
| -32004 | not_found           | unknown thread / draft / account / attachment        |
| -32010 | approval_required   | send moved to Review; `data: {draft, thread}`        |
| -32011 | app_only            | method only callable from Mermoz.app                 |
| -32020 | upstream            | provider (Gmail) error; `data: {status, body}`       |

## Objects

```jsonc
// Account
{"id": "acc_1", "email": "me@example.com", "provider": "gmail",
 "sync": {"state": "initial|idle|syncing|error", "synced": 1200, "total": 5400, "error": null}}

// ThreadSummary
{"id": "thr_…", "number": "M-1287", "account_id": "acc_1",
 "subject": "…", "snippet": "…", "participants": ["Marc Petit", "You"],
 "last_from": "Marc Petit", "last_date": 1791700000000,
 "unread": true, "message_count": 3, "has_attachments": true,
 "status": "todo", "priority": 2, "projects": ["Clients/Acme"],
 "due": "2026-10-16" | null, "summary": "…" | null, "next_action": "…" | null,
 "suggestion": Suggestion | null}

// Suggestion: the router's pending prediction (absent once read or decided).
// Fields under the confidence bar are null / empty. On already-read threads in
// open columns (background sorting) it holds only what would change, at
// confidence ≥ 0.7, and never a priority.
{"status": "archive" | null, "status_confidence": 0.93 | null,
 "projects": ["Receipts"], "project_confidence": 0.88 | null,
 "priority": 0 | null, "automated": 0.97 | null}

// Message
{"id": "msg_…", "from": {"name": "Marc Petit", "email": "marc@…"},
 "to": [{"name": "", "email": "…"}], "cc": [], "date": 1791700000000,
 "subject": "…", "body_text": "… full text …", "body_new": "… without quoted history, signature or boilerplate …",
 "has_quoted": true, "body_clean": "… agent's reading version …" | null, "has_html": true, "is_from_me": false,
 "attachments": [{"id": "att_…", "filename": "report.pdf", "mime": "application/pdf", "size": 2400000}]}

// Status
{"key": "todo", "name": "Todo", "order": 2, "color": "#5A7386"}

// Contact (directory entry built from signatures)
{"email": "marc@acme.example", "name": "Marc Petit", "title": "CFO", "company": "Acme",
 "phones": ["+33 6 12 34 56 78"], "address": "", "website": "", "socials": [], "last_seen": 1791700000000}

// Project (a Gmail label; id is its name)
{"id": "Clients/Acme", "name": "Clients/Acme", "color": "#5B8DEF"}

// Draft
{"id": "dft_…", "account_id": "acc_1", "thread_id": "thr_…" | null,
 "to": ["a@b.c"], "cc": [], "subject": "…", "body": "…",
 "state": "draft|review|sent|rejected", "origin": "app|agent",
 "created": 1791700000000, "updated": 1791700000000}

// ChatMessage: one turn of "Chat about this". Actions are proposals; nothing is applied.
{"id": "cht_…", "thread_id": "thr_…", "role": "user|assistant", "text": "light Markdown",
 "actions": [{"kind": "draft_reply", "label": "Draft a reply", "body": "…"},
             {"kind": "status", "label": "Move to Waiting", "status": "waiting"},
             {"kind": "project", "label": "File in Finance", "project": "Finance"},
             {"kind": "follow_up", "label": "Remind me on Oct 20", "due": "2026-10-20"}],
 "searched": false, "research_unavailable": false, "created": 1791700000000}
```

## Methods

| method               | params                                                                                   | result                                   |
| -------------------- | ---------------------------------------------------------------------------------------- | ---------------------------------------- |
| `hello`              | `{}`                                                                                     | `{"version": "0.1.0"}`                   |
| `accounts.list`      | `{}`                                                                                     | `[Account]`                              |
| `accounts.add_gmail` | `{}`                                                                                     | `{"auth_url": "https://…"}`; completes in background, emits `accounts_changed` |
| `accounts.remove`    | `{"id"}`                                                                                 | `{}`                                     |
| `statuses.list`      | `{}`                                                                                     | `[Status]`                               |
| `labels.list`        | `{}`                                                                                     | `[{"account_id","id","name","count"}]` user labels ("mailboxes"), nested with `/` |
| `projects.list`      | `{}`                                                                                     | `[Project]`                              |
| `projects.create`    | `{"name", "color"?}` — the Gmail label is created when a thread first gets it            | `Project`                                |
| `projects.update`    | `{"id", "name"?, "color"?}` — renames the Gmail label (and its sub-labels) on every account | `Project`                             |
| `projects.delete`    | `{"id"}` — deletes the Gmail label on every account; the mail stays                       | `{"threads": n}`                         |
| `counts`             | `{}`                                                                                     | `{"status": {key: n}, "project": {name: n}, "account": {id: n}}` (open threads only: `done`/`archive`/`deleted` are omitted) |
| `threads.list`       | `{"status"?: [key], "project"?, "account_id"?, "label_id"?, "priority"?: [n], "order"?: "last_date"\|"priority"\|"due"\|"created", "reverse"?: false, "limit"?: 200, "offset"?: 0}` | `{"threads": [ThreadSummary], "total": n}`; newest / most urgent / soonest first unless `reverse` (no priority / no due date stay last) |
| `threads.get`        | `{"id"}`                                                                                 | `{"thread": ThreadSummary, "messages": [Message]}` |
| `threads.update`     | `{"ids": [id], "status"?, "priority"?, "add_projects"?: [name], "remove_projects"?: [name], "projects"?: [name] (replaces), "project"? (one name or null: replaces), "due"? (null clears)}` | `{"updated": n}` |
| `threads.accept_suggestions` | `{"ids": [id]}` — applies each pending suggestion and marks the thread read     | `{"accepted": n}`                        |
| `suggestions.summary` | `{"statuses": [key], "project"?, "account_id"?, "focus"?: false}` — pending suggestions per column, grouped by target; `focus` makes background sorting take these columns first | `{"columns": [{"status", "pending": n, "groups": [{"status": key \| null, "projects": [name], "count": n}], "classified": n, "total": n}], "sorting": bool, "paused": "limit" \| "unavailable" \| null, "calls_today": n, "daily_cap": n}` |
| `suggestions.accept` | `{"status", "project"?, "account_id"?, "target"?: {"status": key \| null, "projects": [name]}}` — accepts the column's pending suggestions, or one group | `{"accepted": n, "before": [ThreadSummary]}` (snapshots for `threads.restore`) |
| `threads.restore`    | `{"threads": [{"id", "unread", "status", "priority", "projects", "due"}]}` — puts threads back as in the snapshots (a `ThreadSummary` works too) | `{"restored": n}` |
| `router.get`         | `{}` — instant routing with TypeSafe Jev (Settings › Developer)                         | `{"configured": bool, "error": "…" \| null, "model": "jev-latest", "daily_cap": n, "calls_today": n}` |
| `router.set`         | `{"api_key"?: "…" \| null, "daily_cap"?: n}` — app only; null removes the key and stops routing; `daily_cap` bounds background sorting calls per day (default 20,000) | same as `router.get`                     |
| `router.test`        | `{"api_key"?}` — one tiny call with the given or stored key                              | `{"ok": true, "ms": n}`                  |
| `threads.mark_read`  | `{"id", "read"?: true}`                                                                  | `{}`                                     |
| `messages.html`      | `{"id", "full"?: false}` — quoted history removed unless `full`                         | `{"html": "…", "has_quoted": bool}` sanitized, remote content stripped |
| `search`             | `{"query", "limit"?: 50}`                                                                | `{"threads": [ThreadSummary]}` full-text, most recent first |
| `attachments.get`    | `{"id"}`                                                                                 | `{"path": "/abs/path", "filename", "mime", "size"}` |
| `drafts.list`        | `{"state"?: "review"}`                                                                   | `[Draft]`                                |
| `drafts.get`         | `{"id"}`                                                                                 | `Draft`                                  |
| `drafts.create`      | `{"account_id"?, "thread_id"?, "to"?, "cc"?, "subject"?, "body"}` — with `thread_id`, it is a reply: recipients/subject default from the thread | `Draft` |
| `drafts.update`      | `{"id", "to"?, "cc"?, "subject"?, "body"?}`                                              | `Draft`                                  |
| `drafts.delete`      | `{"id"}`                                                                                 | `{}`                                     |
| `drafts.send`        | `{"id"}`                                                                                 | `{"sent": true}` or error `approval_required` |
| `review.approve`     | `{"id"}` — app only                                                                      | `{"sent": true}`                         |
| `review.reject`      | `{"id"}` — app only                                                                      | `{}`                                     |
| `providers.list`     | `{}`                                                                                     | `[{"id","name","kind","base_url","model","has_key"}]` (keys never leave the daemon) |
| `providers.save`     | `{"id"?, "name", "kind": "anthropic"\|"openai", "base_url"?, "model", "api_key"?}` — app only | `Provider`                              |
| `providers.delete`   | `{"id"}` — app only                                                                      | `{}`                                     |
| `providers.test`     | `{"id"}`                                                                                 | `{"ok", "clean_body"?, "card"?, "error"?}` on a built-in sample |
| `accounts.agent.get` | `{"account_id"}`                                                                         | `{"provider_ids", "digest_enabled", "done", "pending", "failed", "error"}` |
| `accounts.agent.set` | `{"account_id", "provider_ids"?: [id\|"*"], "digest_enabled"?}` — app only               | `{}`                                     |
| `contacts.list`      | `{"query"?, "limit"?: 200}`                                                              | `[Contact]` most recently heard from first |
| `contacts.get`       | `{"email"}`                                                                              | `{"contact": Contact, "threads": [ThreadSummary]}` |
| `settings.get`       | `{}`                                                                                     | `{"agents_may_send": false}`             |
| `settings.set`       | `{"agents_may_send"}` — app only                                                         | `{}`                                     |
| `chat.get`           | `{"thread_id"}`                                                                          | `{"messages": [ChatMessage], "research_available": bool}` |
| `chat.send`          | `{"thread_id", "text", "research"?: true}` — asks the account's allowed AI; streams `chat_delta` | `ChatMessage` (the answer)       |
| `chat.clear`         | `{"thread_id"}`                                                                          | `{"deleted": n}`                         |
| `feedback.submit`    | `{"text", "command"?, "agent"?}`                                                         | `{"id"}`                                 |
| `feedback.list`      | `{}`                                                                                     | `[{"id","text","command","agent","created"}]` |
| `subscribe`          | `{}`                                                                                     | `{}` then notifications                  |

## Send gate

`drafts.send` sends immediately when the caller is Mermoz.app, or when
`agents_may_send` is true. Otherwise the draft moves to state `review`, its
thread to status `review`, and the call fails with `approval_required`:

```json
{"code": -32010, "message": "approval_required: draft dft_8f2c is waiting for the user to approve it in Mermoz. Tell the user; do not retry.",
 "data": {"draft": "dft_8f2c", "thread": "M-1262"}}
```

The caller is Mermoz.app when the peer process's executable (from the socket's
peer pid) lives inside a bundle named `Mermoz.app`. `review.*` and
`settings.set` are refused with `app_only` for any other caller.

## Notifications

```json
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "threads_changed", "ids": ["thr_…"]}}
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "accounts_changed"}}
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "sync_progress", "account_id": "acc_1", "synced": 1200, "total": 5400}}
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "drafts_changed"}}
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "contacts_changed"}}
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "suggestions_changed"}}
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "chat_delta", "thread_id": "thr_…", "text": "the answer so far"}}
```

`chat_delta` carries the whole visible answer so far (never the actions block), at
most every 60 ms while `chat.send` streams; the final answer is `chat.send`'s result.
