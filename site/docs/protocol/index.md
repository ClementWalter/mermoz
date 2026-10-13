<!-- https://mermoz.xyz/docs/protocol/ · updated 2026-10-13 -->

# Mermoz JSON-RPC protocol (mermozd)

This is the protocol between Mermoz's daemon, `mermozd`, and its clients: the Mac app, the `mermoz` CLI and the MCP server. Use it to build your own client; for agents, the [CLI](/docs/cli/) and the [MCP server](/docs/mcp/) are simpler. Source: [docs/protocol.md](https://github.com/ClementWalter/mermoz/blob/main/docs/protocol.md).

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
 "suggestion": Suggestion | null,
 "conversation_id": "thr_…", "conversation_kind": "person|feed|list", "conversation_threads": 2}

// Conversation (threads.get with "conversation": true): Gmail threads with the same
// people about the same matter, read as one chat. Its id is a thread id; a thread
// alone is its own conversation. Feeds group an automated sender, lists a mailing list.
{"id": "thr_…", "kind": "person|feed|list", "threads": [ThreadSummary],
 "people": [{"name": "Nora Lind", "email": "nora@…", "title": "", "company": "", "messages": 3}],
 "files": [SharedFile], "links": [{"url": "https://…", "message_id": "msg_…", "from": "Nora Lind", "date": 1791700000000}]}

// SharedFile (newest first)
{"id": "att_…", "filename": "cellar-plan.pdf", "mime": "application/pdf", "size": 2400,
 "message_id": "msg_…", "thread_id": "thr_…", "from": "You", "date": 1791700000000}

// Suggestion: the router's pending prediction (absent once read or decided).
// Fields under the confidence bar are null / empty. On already-read threads in
// open columns (background sorting) it holds only what would change, at
// confidence ≥ 0.7, and never a priority.
{"status": "archive" | null, "status_confidence": 0.93 | null,
 "projects": ["Receipts"], "project_confidence": 0.88 | null,
 "priority": 0 | null, "automated": 0.97 | null}

// Message (thread_id: its Gmail thread, inside a conversation of several)
{"id": "msg_…", "thread_id": "thr_…", "from": {"name": "Marc Petit", "email": "marc@…"},
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

// Feed: sections in order (now, today, week, waiting, later), empty ones left out.
// A section shows its 40 most pressing cards (`more` counts the rest; a batch lists
// its 50 latest mails). `sorting` is true while Jev has open threads left to judge.
{"sections": [{"key": "now", "title": "Now", "cards": [Card], "more": 0}], "open": 23, "judged": 21, "sorting": true}

// Card: one thing to do. `kind`: reply | decision | sign | promise | event | payment |
// intro | waiting | cleanup | reading | other. Batches (`id` "batch:cleanup" /
// "batch:reading") list their threads in `members`. `due` is the user's follow-up date,
// else the deadline Jev picked among the dates written in the mail; `amount` is
// verbatim from the mail. `reasons` explain the card in plain words. `texts` are the
// AI's words, null until written; `texts_wanted` says the account's AI may write them.
{"id": "thr_…", "kind": "decision", "section": "now", "thread": ThreadSummary, "members": [],
 "title": "Choose the kitchen tiles", "why": "…" | null, "reasons": ["The mail asks for it by Fri 16 Oct."],
 "due": "2026-10-16" | null, "due_time": "18:00" | null, "amount": "126,00 €" | null,
 "from_backlog": false, "waiting_days": 6 | null, "texts": Texts | null, "texts_wanted": true}

// Texts: the AI's words for a card (cached until new mail). `options` for decisions,
// `points` for sign / payment / event, `draft` for reply / promise / intro / waiting;
// a batch carries one summary per member in `lines`.
{"title": "…", "why": "…", "draft": "…", "summary": "…", "points": ["…"], "lines": ["…"],
 "options": [{"label": "…", "detail": "…", "recommended": true, "reason": "…", "reply": "…"}]}
```

## Methods

| method               | params                                                                                   | result                                   |
| -------------------- | ---------------------------------------------------------------------------------------- | ---------------------------------------- |
| `hello`              | `{}`                                                                                     | `{"version": "0.1.0"}`                   |
| `accounts.list`      | `{}`                                                                                     | `[Account]`, each with `needs_reconnect` (signed in through another OAuth client: still syncs, sign in again) |
| `accounts.add_gmail` | `{}`, or the user's own Google client: `{"client_json"}` (the Google Cloud download) or `{"client_id","client_secret"}` | `{"auth_url": "https://…"}`; completes in background, emits `accounts_changed`; an own client is stored with the token and used for the account's later consents (Drive too) |
| `accounts.sign_in_status` | `{}`                                                                               | `{"running","error"?,"problem"?:{"kind","message","client_id","custom"},"app_client_id"?}`; `kind`: `admin_policy`, `unverified`, `org_internal`, `app_blocked`, `declined` |
| `accounts.reconnect` | `{"account_id"}`                                                                       | `{"auth_url"}`; signs the account in again with the app's client (or the account's own client), keeping local mail; emits `accounts_changed` |
| `accounts.remove`    | `{"id"}`                                                                                 | `{}`                                     |
| `statuses.list`      | `{}`                                                                                     | `[Status]`                               |
| `labels.list`        | `{}`                                                                                     | `[{"account_id","id","name","count"}]` user labels ("mailboxes"), nested with `/` |
| `projects.list`      | `{}`                                                                                     | `[Project]`                              |
| `projects.create`    | `{"name", "color"?}` — the Gmail label is created when a thread first gets it            | `Project`                                |
| `projects.update`    | `{"id", "name"?, "color"?}` — renames the Gmail label (and its sub-labels) on every account | `Project`                             |
| `projects.delete`    | `{"id"}` — deletes the Gmail label on every account; the mail stays                       | `{"threads": n}`                         |
| `counts`             | `{"conversations"?: false}` — with `conversations`, one per conversation, plus a `kind` map of open feeds and lists                                                                                    | `{"status": {key: n}, "project": {name: n}, "account": {id: n}}` (open threads only: `done`/`archive`/`deleted` are omitted) |
| `threads.list`       | `{"status"?: [key], "project"?, "account_id"?, "label_id"?, "priority"?: [n], "order"?: "last_date"\|"priority"\|"due"\|"created", "reverse"?: false, "limit"?: 200, "offset"?: 0, "conversations"?: false, "kind"?: ["person"\|"feed"\|"list"], "suggested"?: true \| {"status": key \| null, "projects"?: [name]}}` — `suggested`: only threads with a pending suggestion, or with one going to that destination; `conversations` lists only the thread leading each conversation (its latest open thread, else its latest) | `{"threads": [ThreadSummary], "total": n}`; newest / most urgent / soonest first unless `reverse` (no priority / no due date stay last) |
| `search`             | `{"query", "limit"?: 50, "typing"?: false}` — every word must match as a whole word (accents and case aside); with `typing`, the last word, once 3+ letters and not followed by a space, also as a prefix. Best first: the words as a phrase, then all in subject/sender, then anywhere; newest within each | `{"threads": [ThreadSummary]}` |
| `search.gmail`       | `{"query", "limit"?: 20}` — Gmail's own search on every account, for mail Mermoz doesn't hold | `{"results": [{"account_id", "id", "thread_id", "from", "subject", "date", "in_mermoz"}]}` |
| `search.gmail_pull`  | `{"account_id", "ids": [id]}` — stores those Gmail messages | `{"threads": [id]}` |
| `threads.get`        | `{"id", "conversation"?: false, "latest"?: n}` — with `conversation`, every message of the thread's conversation; with `latest` too, only its last n (people, files and links still cover all of it) | `{"thread": ThreadSummary, "messages": [Message], "conversation"?: Conversation, "earlier"?: n}` (`earlier`: messages left out by `latest`) |
| `conversations.split` | `{"id"}` — takes the thread out of its conversation for good                            | `{"conversation_id"}`                    |
| `conversations.merge` | `{"ids": [id]}` — joins the threads' conversations into the latest one's                | `{"conversation_id", "threads": n}`      |
| `conversations.progress` | `{}` — background grouping                                                           | `{"pending": n, "waiting_for_ai": n}`    |
| `files.list`         | `{"query"?, "limit"?: 200}` — files shared anywhere, newest first                        | `[SharedFile]`                           |
| `threads.update`     | the other open threads of each conversation follow · `{"ids": [id], "status"?, "priority"?, "add_projects"?: [name], "remove_projects"?: [name], "projects"?: [name] (replaces), "project"? (one name or null: replaces), "due"? (null clears)}` | `{"updated": n}` |
| `threads.accept_suggestions` | `{"ids": [id]}` — applies each pending suggestion and marks the thread read     | `{"accepted": n}`                        |
| `suggestions.summary` | `{"statuses": [key], "project"?, "account_id"?}` — pending suggestions per column, grouped by target | `{"columns": [{"status", "pending": n, "groups": [{"status": key \| null, "projects": [name], "count": n}]}], "paused": "credits" \| "unavailable" \| null, "calls_today": n, "daily_cap": n}` |
| `suggestions.resort` | `{"stop": true}` — ends a whole-mailbox sort left by an older version and clears what it suggested (as `suggestions.clear`); starting one is refused | `{"stopped": true, "cleared": n}` |
| `suggestions.clear`  | `{}` — deletes suggestions on read threads nobody reviewed (and "no change" verdicts); Unread's, accepted and rejected ones stay; the threads go back in the Sort queue | `{"cleared": n}` |
| `sort.state`         | `{"statuses"?: [key]}` — the Sort queue: read threads in open columns without a decided verdict | `{"to_sort": n, "by_status": [{"status", "count"}], "configured": bool, "paused": "credits" \| "unavailable" \| null, "provider_name": "TypeSafe", "offer": {"provider", "name", "message"} \| null, "learned": [{"sender", "name", "status", "projects", "kind": "rule" \| "avoid", "count"}]}` |
| `sort.next`          | `{"statuses"?: [key], "exclude"?: [id], "count"?: 3}` — the next cards, newest first; suggestions are asked of the AI for these few threads only (at most 12 calls per request) | `{"cards": [{"thread": ThreadSummary, "why": "…" \| null}], "paused": …, "provider_name": "…", "offer": … }` |
| `sort.answer`        | `{"id", "answer": "yes" \| "no" \| "change", "status"?, "projects"?}` — yes accepts, no refuses (never offered again for that sender), change refuses and files where given | `{"undo": {"thread", "since"}, "learned": "Learned: mail from X goes to Y" \| null}` |
| `sort.undo`          | `{"undo"}` from `sort.answer` — the thread returns as it was, the answer is forgotten | `{}` |
| `suggestions.accept` | `{"status", "project"?, "account_id"?, "target"?: {"status": key \| null, "projects": [name]}}` — accepts the column's pending suggestions, or one group | `{"accepted": n, "before": [ThreadSummary]}` (snapshots for `threads.restore`) |
| `threads.reject_suggestions` | `{"ids": [id]}` — turns the pending suggestions down; the threads stay put and are not suggested again until new mail | `{"rejected": n}` |
| `threads.reopen_suggestions` | `{"ids": [id]}` — undoes a rejection: the suggestions are pending again | `{"reopened": n}` |
| `threads.restore`    | `{"threads": [{"id", "unread", "status", "priority", "projects", "due"}]}` — puts threads back as in the snapshots (a `ThreadSummary` works too) | `{"restored": n}` |
| `router.get`         | `{}` — instant routing with a System One provider (Settings › Developer); top-level fields describe the picked one | `{"provider": "typesafe" \| "subq", "provider_name": "TypeSafe", "configured": bool, "error": "…" \| null, "model": "jev-latest" \| "subq-snap-preview", "daily_cap": n, "calls_today": n, "paused": "credits" \| "unavailable" \| null, "providers": [{"id", "name", "model", "configured", "error", "paused", "calls_today"}], "offer": {"provider", "name", "message"} \| null}` |
| `router.set`         | `{"provider"?: "typesafe" \| "subq", "api_key"?: "…" \| null, "key_provider"?: id, "daily_cap"?: n}` — app only; `provider` picks where mail is sent; `api_key` is stored for `key_provider`, else `provider`, else the picked one (null removes it); `daily_cap` bounds background sorting calls per day (default 20,000) | same as `router.get`                     |
| `router.test`        | `{"api_key"?, "provider"?}` — one tiny call with the given or stored key of the provider (default: the picked one) | `{"ok": true, "ms": n}`                  |
| `threads.mark_read`  | `{"id", "read"?: true}` — reading marks the whole conversation read                                                                  | `{"thread": ThreadSummary}` as reading left it |
| `messages.html`      | `{"id", "full"?: false}` — quoted history removed unless `full`                         | `{"html": "…", "has_quoted": bool}` sanitized, remote content stripped |
| `attachments.get`    | `{"id"}`                                                                                 | `{"path": "/abs/path", "filename", "mime", "size"}` |
| `drafts.list`        | `{"state"?: "review"}`                                                                   | `[Draft]`                                |
| `drafts.get`         | `{"id"}`                                                                                 | `Draft`                                  |
| `drafts.create`      | `{"account_id"?, "thread_id"?, "to"?, "cc"?, "subject"?, "body", "attachments"?: [Attach]}` — with `thread_id`, it is a reply: recipients/subject default from the thread. All attachments are added or none (the draft is not created) | `Draft` |
| `drafts.attach`      | `{"id", "attachments": [Attach]}` or `{"id", ...Attach}`                                 | `Draft`                                  |
| `drafts.detach`      | `{"id", "attachment_id"}` (an id from `Draft.attachments`)                               | `Draft`                                  |
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
| `feed.list`          | `{"project"?, "account_id"?}` — the Feed: open threads as cards, in sections            | `Feed`                                   |
| `feed.texts`         | `{"ids": [thread id]}` — asks the account's allowed AI to write these cards' words in the background (cards on screen only); emits `feed_changed` per card | `{"queued": n}` |
| `feed.snooze`        | `{"ids": [id], "until": ms, "label"?}` — Later: the threads leave the feed and their status columns until `until` (or new mail), then come back Unread and first in the feed with "You set this for <label>" | `{"snoozed": n}` |
| `later.options`      | `{}` — the Later picker's quick choices for now: this evening (before 17:00), tomorrow morning, this weekend (weekdays), Friday evening (Mon–Thu), next week | `[{"key": "f", "label": "Friday evening", "until": ms, "when": "Fri 18:00"}]` |
| `later.parse`        | `{"text"}` — plain French or English ("vendredi soir", "friday 7pm", "dans 3 jours", "15 nov"), read locally | `{"until": ms, "when": "Fri 18:00"}` or `null` |
| `feed.learn_archive` | `{"ids": [id]}` — the user archived these automated mails from a Feed card: a yes to "Archive" for each sender, as in the Sort queue (three make a rule; ruled senders' mail is batched) | `{"learned": n}` |
| `feed.unsnooze`      | `{"ids": [id]}`                                                                          | `{"unsnoozed": n}`                       |
| `feedback.submit`    | `{"text", "command"?, "agent"?}`                                                         | `{"id"}`                                 |
| `feedback.list`      | `{}`                                                                                     | `[{"id","text","command","agent","created"}]` |
| `subscribe`          | `{}`                                                                                     | `{}` then notifications                  |

## Google Drive

Drive has its own consent and refresh token per account (stored apart from the
Gmail one), so declining or disconnecting Drive never affects mail. Asked scopes:
`drive.readonly` (read any file the user can open: previews, files shared with a
thread's people, search) and `drive.file` (files Mermoz created or the user picked:
saving attachments, sharing a picked file). Google lets the user untick either;
`drive.status` says what was granted.

```jsonc
// DriveFile
{"id": "1Abc…", "name": "Remediation plan", "mime": "application/vnd.google-apps.document",
 "kind": "Google Doc", "modified": 1791648000000, "owner": {"name": "Marc", "email": "marc@…"} | null,
 "web_link": "https://docs.google.com/…" | null, "icon_link": "…" | null, "has_thumbnail": true}

// DriveLink: a Drive link found in a thread's messages
{"file_id": "1Abc…", "url": "https://docs.google.com/document/d/1Abc…/edit", "link_kind": "document",
 "message_id": "msg_…", "access": "ok|no_access|not_connected|error", "file": DriveFile | null}

// SavedAttachment
{"attachment_id": "att_…", "file_id": "…", "name": "report.pdf", "web_link": "…",
 "folder": "Mermoz/Clients/Acme", "created": 1791700000000}
```

| method | params | result |
| --- | --- | --- |
| `drive.status` | `{"account_id"?}` (all accounts without it) | `{"account_id", "email", "connected", "can_read_all", "can_save", "attempt": {"state": "waiting\|connected\|declined\|error", "error"} \| null}` |
| `drive.connect` | `{"account_id"}` — app only; completes in background, emits `drive_changed` | `{"auth_url"}` |
| `drive.disconnect` | `{"account_id"}` — app only; forgets the token and the cache (does not call Google's revoke, which would end Gmail access too) | `{}` |
| `drive.links` | `{"thread_id"}` | `{"account_id", "connected", "links": [DriveLink]}` |
| `drive.files_for_thread` | `{"thread_id", "limit"?: 20}` — for the thread's Docs tab | `{"account_id", "connected", "can_read_all", "linked": [DriveLink], "shared": [DriveFile] (owned by or shared with its participants, linked ones left out), "saved": [SavedAttachment]}` |
| `drive.files_for_people` | `{"account_id", "emails": [email], "limit"?: 20}` | `{"files": [DriveFile]}` newest first |
| `drive.search` | `{"account_id", "query"?, "limit"?: 20}` — no query: recently opened files | `{"files": [DriveFile]}` |
| `drive.content` | `{"account_id", "file_id"}` — Docs as Markdown, Sheets as CSV (first sheet), Slides and text files as text; cached by version | `{"file": DriveFile, "text": "…" \| null}` |
| `drive.thumbnail` | `{"account_id", "file_id"}` | `{"mime", "data": base64}` or `null` |
| `drive.pick` | `{"account_id", "file_ids"?: [id]}` — app only; opens Google's file picker (`drive.file` only); without a Drive connection the pick connects Drive with `drive.file` alone | `{"pick_id", "auth_url"}` |
| `drive.pick_result` | `{"pick_id"}` | `{"account_id", "state": "waiting\|done\|declined\|error", "files": [DriveFile], "error"}` |
| `drive.save_attachment` | `{"attachment_id"}` — into `Mermoz/<first project, split on "/">` or `Mermoz/No project`, folders created when missing; saving again returns the first save | `SavedAttachment` |
| `drive.saved` | `{"thread_id"}` | `[SavedAttachment]` |
| `drive.share` | `{"account_id", "file_id", "emails": [email], "role": "reader"\|"commenter"}` — app only, after the user confirms; Drive sends no email of its own | `{"shared": [email], "failed": [{"email", "reason"}]}` |

Drive errors: `-32030` `drive_not_connected` (`data: {account_id}`): show "Connect
Google Drive". `-32031` `drive_needs_access` (`data: {account_id, reason}`):
`read_all` (the read permission was unticked), `save` (the "files you use with
Mermoz" permission was unticked) or `pick` (choose the file in Google's picker
first, then retry).

## Send gate

`Attach` is an absolute file path, or `{"path" | "from_attachment" | "url", "filename"?, "inline"?}`:
`from_attachment` copies an attachment of a message already in Mermoz (forwarding),
`url` adds a link listed under the text instead of a file. Files are copied into
`blobs/drafts/<draft>/` and deleted once the draft is sent or deleted. Each
`Draft` has `attachments: [{id, kind: "file"|"link", filename, mime, size, path?, url?, inline}]`.
More than 25 MB of files in one draft is refused with a plain-words message.
Messages larger than 4 MB go through Gmail's upload endpoint.

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
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "drive_changed", "account_id": "acc_1"}}
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "feed_changed"}}
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "chat_delta", "thread_id": "thr_…", "text": "the answer so far"}}
```

`chat_delta` carries the whole visible answer so far (never the actions block), at
most every 60 ms while `chat.send` streams; the final answer is `chat.send`'s result.
A Claude turn paused by its web search loop (`pause_turn`) is resumed automatically, up
to 3 times, into the same answer; past that, the answer so far is kept.
