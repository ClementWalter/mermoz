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
- Status keys (default set, configurable): `triage`, `backlog`, `todo`,
  `in_progress`, `waiting`, `review`, `done`, `canceled`.
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
 "status": "todo", "priority": 2, "project_id": "prj_1" | null,
 "due": "2026-10-16" | null, "summary": "…" | null, "next_action": "…" | null}

// Message
{"id": "msg_…", "from": {"name": "Marc Petit", "email": "marc@…"},
 "to": [{"name": "", "email": "…"}], "cc": [], "date": 1791700000000,
 "subject": "…", "body_text": "…", "has_html": true, "is_from_me": false,
 "attachments": [{"id": "att_…", "filename": "report.pdf", "mime": "application/pdf", "size": 2400000}]}

// Status
{"key": "todo", "name": "Todo", "order": 2, "color": "#5A7386"}

// Project
{"id": "prj_1", "name": "Q4 audit", "color": "#5B8DEF"}

// Draft
{"id": "dft_…", "account_id": "acc_1", "thread_id": "thr_…" | null,
 "to": ["a@b.c"], "cc": [], "subject": "…", "body": "…",
 "state": "draft|review|sent|rejected", "origin": "app|agent",
 "created": 1791700000000, "updated": 1791700000000}
```

## Methods

| method               | params                                                                                   | result                                   |
| -------------------- | ---------------------------------------------------------------------------------------- | ---------------------------------------- |
| `hello`              | `{}`                                                                                     | `{"version": "0.1.0"}`                   |
| `accounts.list`      | `{}`                                                                                     | `[Account]`                              |
| `accounts.add_gmail` | `{}`                                                                                     | `{"auth_url": "https://…"}`; completes in background, emits `accounts_changed` |
| `accounts.remove`    | `{"id"}`                                                                                 | `{}`                                     |
| `statuses.list`      | `{}`                                                                                     | `[Status]`                               |
| `projects.list`      | `{}`                                                                                     | `[Project]`                              |
| `projects.create`    | `{"name", "color"?}`                                                                     | `Project`                                |
| `counts`             | `{}`                                                                                     | `{"status": {key: n}, "project": {id: n}, "account": {id: n}}` (open threads only, except `done`/`canceled` which are omitted) |
| `threads.list`       | `{"status"?: [key], "project_id"?, "account_id"?, "priority"?: [n], "limit"?: 200, "offset"?: 0}` | `{"threads": [ThreadSummary], "total": n}` ordered by `last_date` desc |
| `threads.get`        | `{"id"}`                                                                                 | `{"thread": ThreadSummary, "messages": [Message]}` |
| `threads.update`     | `{"ids": [id], "status"?, "priority"?, "project_id"? (null clears), "due"? (null clears)}` | `{"updated": n}`                         |
| `threads.mark_read`  | `{"id", "read"?: true}`                                                                  | `{}`                                     |
| `messages.html`      | `{"id"}`                                                                                 | `{"html": "…"}` sanitized, remote content stripped |
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
| `settings.get`       | `{}`                                                                                     | `{"agents_may_send": false}`             |
| `settings.set`       | `{"agents_may_send"}` — app only                                                         | `{}`                                     |
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
```
