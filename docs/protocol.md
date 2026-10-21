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
| -32021 | sign_in_expired     | Google no longer accepts the account's sign-in; the user signs in again (`accounts.reconnect`) |

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
 "to": [{"name": "", "email": "…"}], "cc": [], "reply_to": [{"name": "Franck Adam", "email": "franck@…"}],
 "date": 1791700000000,
 "subject": "…", "body_text": "… full text …", "body_new": "… what the bubble shows …",
 "has_quoted": true, "has_signature": true, "forwarded": false, "forwarded_from": "Bank" | null, "quoted_only": false,
 "body_clean": "… agent's reading version …" | null, "has_html": true, "is_from_me": false,
 "reaction": {"emoji": "❤️", "target_message_id": "msg_…" | null} | null,
 "is_bounce": false, "is_auto_reply": false,
 "delivery_failure": {"recipient": "no-reply@…", "status": "5.1.1", "reason": "address doesn't exist",
                      "bounce_message_id": "msg_…"} | null,
 "duplicate_of": "msg_…" | null, "copies": 1,
 "attachments": [{"id": "att_…", "filename": "report.pdf", "mime": "application/pdf", "size": 2400000,
                  "inline": false, "decorative": false}]}

// reply_to: the `Reply-To` header (empty when replies go to `from`). Mail stored before
// it was kept learns it when its thread is opened (one metadata call per message,
// then a threads_changed event).

// is_bounce: a delivery failure report from the mail system (mailer-daemon, postmaster,
// or multipart/report; report-type=delivery-status). is_auto_reply: an automatic answer
// (Auto-Submitted other than "no", X-Autoreply, X-Autorespond, or an "Out of office" /
// "Automatic reply" / "Réponse automatique" subject). delivery_failure: on the user's
// message a bounce reports on (matched by the Message-ID in the bounce, else the
// user's latest earlier message to that recipient): who it missed, the RFC 3463
// status, why in plain words, and the bounce message. A client shows it on that
// message rather than the bounce as a bubble. Older mail learns its kind and report
// when its thread is opened, like reply_to.

// ReplyAddressing (threads.get "reply"): the reply-all a conversation starts with,
// answering message_id (in Gmail thread thread_id): the latest message from a real
// correspondent, i.e. not the user, not a bounce or auto-reply, and not a no-reply
// sender without a Reply-To that reaches someone; else the user's latest message
// (its recipients are kept); else the latest that isn't a bounce or auto-reply.
// Addressing follows RFC 5322: on a received message To is its Reply-To (else From) plus its To,
// Cc its Cc; on the user's own message, its recipients again, a sender that asked for
// replies elsewhere swapped for that address. The user's address is never included.
// no_reply lists To addresses nobody reads (no-reply, noreply, do-not-reply,
// donotreply, ne-pas-repondre, notifications@, mailer-daemon); suggestion is a
// person's address found in the mail, offered in their place, or null.
{"message_id": "msg_…", "thread_id": "thr_…", "to": [Address], "cc": [Address], "no_reply": ["no-reply@…"],
 "suggestion": {"name": "Franck Adam", "email": "franck@…"} | null}

// Message reading (crates/mermoz-core/src/reading.rs), worked out at ingest and kept:
// body_new    only what the sender newly wrote: greeting, body, sign-off name. LF line
//             ends, no blank first/last line, never empty when the message has text.
//             Quoted history, reply headers, signatures, disclaimers, tracking links,
//             bulk-mail boilerplate and Markdown/HTML artefacts are removed. Clients
//             display it as is.
// has_quoted  quoted history was folded away
// has_signature  a signature, mobile footer or disclaimer was folded away
// forwarded   the message forwards an email: body_new is the note, a blank line, then
//             the forwarded text (its header block removed); forwarded_from names its
//             author
// quoted_only the message adds no words of its own; body_new is the text it quotes
// reaction    an Outlook/Teams reaction ("[heart] Nora reacted to your message") to
//             target_message_id (found by In-Reply-To, else by the words it quotes);
//             body_new is then its emoji
// copies      how many times this email was received (a forwarding address, a double
//             import); threads hide the other deliveries and show the kept one with
//             copies >= 2. duplicate_of names the kept copy on a hidden one. Gmail keeps
//             every copy: thread actions apply to all, none is deleted on its own.
// attachments[].decorative  a signature logo, social icon, banner or tracking pixel:
//             never shown as a photo nor counted in Photos (conversation files leave
//             them out)

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
 "created": 1791700000000, "updated": 1791700000000,
 "reply_to_message_id": "msg_…" | null}
// reply_to_message_id: the message the reply answers: it carries that message's
// In-Reply-To/References and ends with it quoted ("On …, Name <addr> wrote:" then
// "> " lines). Without it, a reply answers the thread's ReplyAddressing message and
// adds no quote.

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

## Ask Mermoz to do

A sentence typed in ⌘K ("archive all messages past 5 months not in done") goes to the
first account's allowed AI with the names of statuses, projects and accounts, and the
**candidates** found locally for the sentence's content words: up to 5 senders (address,
display name, thread count, how their subjects usually begin) and up to 4 shared subject
beginnings with counts. Never bodies. The AI fills a fixed schema; the daemon checks it
strictly (unknown fields, actions, statuses or projects, or a description put in `from`,
are refused and asked again once with the reason) and resolves it locally. Nothing in the
schema can send, reply to or forward mail.

Senders are matched by their words: the display names, the address cut on `.` `-` `_` `+`
`@`, and the mailing list, accents and case aside, with one typo allowed in words of 5+
letters (two in 8+). An entry matches a sender when every content word is found;
`a@b.example` matches that address only, `b.example` its addresses and subdomains.

```jsonc
// Plan: 1–5 steps, each a filter (every field narrows; empty ones don't) and one action.
{"steps": [{"filter": {"status_in": [key], "status_not_in": [key],
                       "older_than": {"amount": 5, "unit": "day|week|month|year"}, "newer_than": …,
                       "from": ["nora@lind.example", "news.example", "Paul"],  // address, domain, or name
                       "projects": [name], "not_projects": [name], "unread": bool, "has_attachment": bool,
                       "account": "acc_1", "query": "words", "subject_prefix": "Notes:"},
            "action": {"kind": "set_status", "status": "archive"}       // any status but unread / review
                      | {"kind": "add_project" | "remove_project", "project": name}
                      | {"kind": "mark_read" | "mark_unread"}
                      | {"kind": "later", "until": "2026-10-20"}        // that day, 08:00 local
                      | {"kind": "delete"}}]}                          // Gmail Trash
// Trash is never matched unless status_in asks for deleted.

// DoPreview (do.ask / do.preview). `kind`: plan | question (unclear, or not doable) | needs_ai.
{"kind": "plan", "id": "bop_…", "sentence": "…", "plan": Plan,
 "summary": "Archive 1,842 threads older than 5 months that aren't Done",
 "interpretation": "status ≠ Done AND older than 5 months",
 "steps": [{"summary", "interpretation", "action": "Archive", "count": 1842,
            "resolved": "from gemini@google.example (40)" | null,       // who the sender words matched
            "senders": [{"email", "name", "threads"}],                   // the first 5
            "suggestions": [Choice],                                      // closest senders, when 0 match
            "by_status": [{"status": "todo", "name": "To do", "count": 1200}],
            "sample": [{"id", "number", "status", "subject", "last_from", "last_date"}]}],   // first 5
 "total": 1842, "deletes": false, "confirm": "enter" | "type_count",
 "choices": [Choice],   // when nothing matches: the suggestions, then the sentence's candidates
 "warning": "Nothing matches. I understood: …" | "This applies to every email in Mermoz." | null}
{"kind": "question", "message": "Which notes do you mean?", "choices": [Choice], "round": 1,
 "can_answer": true}   // false after 2 answered rounds: pick a choice or say it another way
{"kind": "needs_ai", "message": "…"}

// Choice: a candidate the user can pick; `pick` re-plans with exactly that filter.
{"label": "Gemini · gemini@google.example · 40 threads · “Notes:…”", "pick": {"from": "gemini@google.example"}, "threads": 40}
{"label": "Subject “Notes:…” · 42 threads", "pick": {"subject_prefix": "Notes:"}, "threads": 42}

// DoOperation (the operation log; survives restarts)
{"id": "bop_…", "sentence", "plan": Plan, "summary", "state": "planned|running|done|failed|undone",
 "total", "done", "origin": "app|agent", "error", "created", "finished", "undone"}
```

| method | params | result |
| --- | --- | --- |
| `do.status` | `{}` | `{"available": true, "provider_name"}` or `{"available": false, "message"}` |
| `do.ask` | `{"text", "rounds"?: [{"question", "answer"}], "pick"?: {"from"} \| {"subject_prefix"}}` — the AI plans from the sentence, the answers so far (at most 2) and the candidates, the daemon previews; nothing changes. A `pick` replaces the plan's sender, words and subject in every step; when the AI still asks, the sentence's verb (delete, archive, mark read…) plans with the pick alone | `DoPreview` |
| `do.replan` | `{"id", "pick"}` — a planned preview again with the picked filter (no AI) | `DoPreview` |
| `do.preview` | `{"plan", "text"?}` — a plan given as JSON, checked and previewed without the AI | `DoPreview` |
| `do.targets` | `{"id"}` — every thread a planned preview would change (all steps, once each), newest first, with its current status: the preview's full list | `{"threads": [{"id", "number", "status", "subject", "last_from", "last_date"}]}` |
| `do.run` | `{"id", "confirm"?, "exclude"?: [id]}` — `exclude`: threads unticked in the list, left alone (the count to confirm a delete is then the ticked ones). Runs a planned preview once, 200 threads per write through the same store changes as `threads.update` / `threads.mark_read` / `feed.snooze` (Gmail write-back included), logging each thread's state first; emits `do_progress`. A plan that deletes needs `confirm` equal to its `total` | `{"id", "done", "total", "undo_label"}` |
| `do.undo` | `{"id"}` — puts every thread the run changed back as it was (Later is lifted) | `{"id", "restored"}` |
| `do.get` | `{"id"}` | `DoOperation` |
| `do.list` | `{"limit"?: 20}` — runs, newest first | `[DoOperation]` |

## Methods

| method               | params                                                                                   | result                                   |
| -------------------- | ---------------------------------------------------------------------------------------- | ---------------------------------------- |
| `hello`              | `{}`                                                                                     | `{"version": "0.1.0"}`                   |
| `accounts.list`      | `{}`                                                                                     | `[Account]`, each with `needs_reconnect` (the user should sign in again) and `sign_in_expired`. `sign_in_expired: true`: Google refused the sign-in (`invalid_grant`: access removed, password changed, or the 7-day expiry while the Google app is in testing); nothing syncs and local changes stay queued until the user signs in again, `sync.error` says so in plain words. Remembered across restarts; set and cleared with `accounts_changed`. `needs_reconnect` alone: signed in through another OAuth client, still syncs |
| `accounts.add_gmail` | `{}`, or the user's own Google client: `{"client_json"}` (the Google Cloud download) or `{"client_id","client_secret"}` | `{"auth_url": "https://…"}`; completes in background, emits `accounts_changed`; an own client is stored with the token and used for the account's later consents (Drive too) |
| `accounts.sign_in_status` | `{}`                                                                               | `{"running","error"?,"problem"?:{"kind","message","client_id","custom"},"app_client_id"?}`; `kind`: `admin_policy`, `unverified`, `org_internal`, `app_blocked`, `declined` |
| `accounts.reconnect` | `{"account_id"}`                                                                       | `{"auth_url"}`; signs the account in again with the app's client (or the account's own client), keeping local mail; emits `accounts_changed` |
| `accounts.remove`    | `{"id"}`                                                                                 | `{}`                                     |
| `accounts.add_demo` | `{}` | `Account` (`provider: "demo"`); adds the local demo mailbox (or returns it), emits `accounts_changed`, `threads_changed`, `feed_changed`. It never syncs, its mail never goes to an AI, and `drafts.send` from it only files the message in its own Sent |
| `accounts.set_hidden` | `{"id", "hidden": bool}`                                                             | `{}`; a hidden account keeps syncing but its mail leaves every list, count, search, feed and suggestion; emits `accounts_changed` and `threads_changed` |
| `statuses.list`      | `{}`                                                                                     | `[Status]`                               |
| `labels.list`        | `{}`                                                                                     | `[{"account_id","id","name","count"}]` user labels ("mailboxes"), nested with `/` |
| `projects.list`      | `{}`                                                                                     | `[Project]`                              |
| `projects.create`    | `{"name", "color"?}` — the Gmail label is created when a thread first gets it            | `Project`                                |
| `projects.update`    | `{"id", "name"?, "color"?}` — renames the Gmail label (and its sub-labels) on every account | `Project`                             |
| `projects.delete`    | `{"id"}` — deletes the Gmail label on every account; the mail stays                       | `{"threads": n}`                         |
| `counts`             | `{"conversations"?: false}` — with `conversations`, one per conversation, plus a `kind` map of open feeds and lists                                                                                    | `{"status": {key: n}, "project": {name: n}, "account": {id: n}}` (open threads only: `done`/`archive`/`deleted` are omitted) |
| `threads.list`       | `{"status"?: [key], "project"?, "account_id"?, "label_id"?, "priority"?: [n], "order"?: "last_date"\|"priority"\|"due"\|"created", "reverse"?: false, "limit"?: 200, "offset"?: 0, "conversations"?: false, "kind"?: ["person"\|"feed"\|"list"], "suggested"?: true \| {"status": key \| null, "projects"?: [name]}}` — `suggested`: only threads with a pending suggestion, or with one going to that destination; `conversations` lists only the thread leading each conversation (its latest open thread, else its latest) | `{"threads": [ThreadSummary], "total": n}`; newest / most urgent / soonest first unless `reverse` (no priority / no due date stay last) |
| `search`             | `{"query", "limit"?: 50, "typing"?: false}` — every word must match as a whole word (accents and case aside); with `typing`, the last word, once 3+ letters and not followed by a space, also as a prefix. Best first: the words as a phrase, then all in subject/sender, then anywhere; newest within each. With search by meaning ready, the threads nearest in meaning (up to 20, cosine ≥ 0.30) are fused in by Reciprocal Rank Fusion (k = 60) | `{"threads": [ThreadSummary], "word_matches": n}` (how many hits hold the words) |
| `smart_search.status` | `{}` | `{"enabled", "phase": "off"\|"starting"\|"downloading"\|"indexing"\|"ready"\|"error", "progress": 0-100, "indexed", "total"}` |
| `smart_search.set` | `{"enabled": bool}` — app only; off frees the model and stops indexing | same as `smart_search.status` |
| `search.gmail`       | `{"query", "limit"?: 20}` — Gmail's own search on every account, for mail Mermoz doesn't hold | `{"results": [{"account_id", "id", "thread_id", "from", "subject", "date", "in_mermoz"}]}` |
| `search.gmail_pull`  | `{"account_id", "ids": [id]}` — stores those Gmail messages | `{"threads": [id]}` |
| `threads.get`        | `{"id", "conversation"?: false, "latest"?: n}` — with `conversation`, every message of the thread's conversation; with `latest` too, only its last n (people, files and links still cover all of it) | `{"thread": ThreadSummary, "messages": [Message], "conversation"?: Conversation, "earlier"?: n, "reply": ReplyAddressing \| null}` (`earlier`: messages left out by `latest`) |
| `conversations.split` | `{"id"}` — takes the thread out of its conversation for good                            | `{"conversation_id"}`                    |
| `conversations.merge` | `{"ids": [id]}` — joins the threads' conversations into the latest one's; a manual join the grouping learns from | `{"conversation_id", "threads": n, "members": [id], "undo"}` |
| `conversations.unmerge` | `{"undo"}` — the `undo` a merge returned: each thread back in its conversation, the join unlearned | `{"threads": n}` |
| `conversations.progress` | `{}` — background grouping                                                           | `{"pending": n, "waiting_for_ai": n}`    |
| `files.list`         | `{"query"?, "limit"?: 200}` — files shared anywhere, newest first                        | `[SharedFile]`                           |
| `threads.update`     | the other open threads of each conversation follow · `{"ids": [id], "status"?, "priority"?, "add_projects"?: [name], "remove_projects"?: [name], "projects"?: [name] (replaces), "project"? (one name or null: replaces), "due"? (null clears)}` | `{"updated": n, "before": [ThreadState]}` (every thread reached, siblings included, for `threads.restore`) |
| `threads.accept_suggestions` | `{"ids": [id]}` — applies each pending suggestion and marks the thread read     | `{"accepted": n}`                        |
| `suggestions.summary` | `{"statuses": [key], "project"?, "account_id"?}` — pending suggestions per column, grouped by target | `{"columns": [{"status", "pending": n, "groups": [{"status": key \| null, "projects": [name], "count": n}]}], "paused": "credits" \| "unavailable" \| null, "calls_today": n, "daily_cap": n}` |
| `suggestions.resort` | `{"stop": true}` — ends a whole-mailbox sort left by an older version and clears what it suggested (as `suggestions.clear`); starting one is refused | `{"stopped": true, "cleared": n}` |
| `suggestions.clear`  | `{}` — deletes suggestions on read threads nobody reviewed (and "no change" verdicts); Unread's, accepted and rejected ones stay; they are suggested again when next shown | `{"cleared": n}` |
| `threads.suggest`    | `{"ids": [id]}` — the Feed's cards on screen and the next few (at most 6): each read thread in an open column gets its suggestion just in time (stored, a learned sender rule, else one AI call); while sorting is held (`paused`: "credits", "unavailable", "off" = daily limit 0, "limit" = today's limit used up) only stored and learned ones come back. Only confident suggestions that would change something are returned | `{"suggestions": [{"id", "suggestion": Suggestion, "why": "…" \| null}], "paused": … \| null, "provider_name": "TypeSafe", "offer": {"provider", "name", "message"} \| null}` |
| `learned.list`       | `{}` — what the answers taught, newest first | `{"learned": [{"sender", "name", "status", "projects", "kind": "rule" \| "avoid", "count"}]}` |
| `learned.forget`     | `{"item"}` (one of `learned.list`) — forgets that rule's yes answers, or that refusal's no answers | `{"forgotten": n}` |
| `sort.answer`        | `{"id", "answer": "yes" \| "no" \| "change", "status"?, "projects"?}` — answers a suggestion shown on a Feed card: yes accepts, no refuses (never offered again for that sender), change refuses and files where given; three consistent yes for a sender make a rule; a status it sets moves the conversation's other open threads too | `{"undo": {"thread", "since", "siblings"?}, "learned": "Learned: mail from X goes to Y" \| null}` |
| `sort.undo`          | `{"undo"}` from `sort.answer` — the thread (and its conversation's other threads) return as they were, the answer is forgotten | `{}` |
| `suggestions.accept` | `{"status", "project"?, "account_id"?, "target"?: {"status": key \| null, "projects": [name]}}` — accepts the column's pending suggestions, or one group | `{"accepted": n, "before": [ThreadSummary]}` (snapshots for `threads.restore`) |
| `threads.reject_suggestions` | `{"ids": [id]}` — turns the pending suggestions down; the threads stay put and are not suggested again until new mail | `{"rejected": n}` |
| `threads.reopen_suggestions` | `{"ids": [id]}` — undoes a rejection: the suggestions are pending again | `{"reopened": n}` |
| `threads.restore`    | `{"threads": [{"id", "unread", "status", "priority", "projects", "due"}]}` — puts each thread back as in its snapshot (conversation siblings do not follow) (a `ThreadSummary` works too) | `{"restored": n}` |
| `router.get`         | `{}` — instant routing with a System One provider (Settings › Developer); top-level fields describe the picked one | `{"provider": "typesafe" \| "subq", "provider_name": "TypeSafe", "configured": bool, "error": "…" \| null, "model": "jev-latest" \| "subq-snap-preview", "daily_cap": n, "calls_today": n, "paused": "credits" \| "unavailable" \| null, "providers": [{"id", "name", "model", "configured", "error", "paused", "calls_today"}], "offer": {"provider", "name", "message"} \| null}` |
| `router.set`         | `{"provider"?: "typesafe" \| "subq", "api_key"?: "…" \| null, "key_provider"?: id, "daily_cap"?: n}` — app only; `provider` picks where mail is sent; `api_key` is stored for `key_provider`, else `provider`, else the picked one (null removes it); `daily_cap` bounds background sorting calls per day (default 20,000) | same as `router.get`                     |
| `router.test`        | `{"api_key"?, "provider"?}` — one tiny call with the given or stored key of the provider (default: the picked one) | `{"ok": true, "ms": n}`                  |
| `threads.mark_read`  | `{"id", "read"?: true}` — reading marks the whole conversation read                                                                  | `{"thread": ThreadSummary}` as reading left it |
| `messages.html`      | `{"id", "full"?: false}` — quoted history removed unless `full`                         | `{"html": "…", "has_quoted": bool}` sanitized, remote content stripped |
| `attachments.get`    | `{"id"}`                                                                                 | `{"path": "/abs/path", "filename", "mime", "size"}` |
| `drafts.list`        | `{"state"?: "review"}`                                                                   | `[Draft]`                                |
| `drafts.get`         | `{"id"}`                                                                                 | `Draft`                                  |
| `drafts.create`      | `{"account_id"?, "thread_id"?, "reply_to_message_id"?, "to"?, "cc"?, "subject"?, "body", "attachments"?: [Attach]}` — with `thread_id`, it is a reply: recipients/subject default from the thread's reply target; with `reply_to_message_id`, a reply to that message (its thread; recipients by the ReplyAddressing rules from it, "Re: " its subject). All attachments are added or none (the draft is not created) | `Draft` |
| `drafts.attach`      | `{"id", "attachments": [Attach]}` or `{"id", ...Attach}`                                 | `Draft`                                  |
| `drafts.detach`      | `{"id", "attachment_id"}` (an id from `Draft.attachments`)                               | `Draft`                                  |
| `drafts.update`      | `{"id", "to"?, "cc"?, "subject"?, "body"?}`                                              | `Draft`                                  |
| `drafts.delete`      | `{"id"}`                                                                                 | `{}`                                     |
| `drafts.send`        | `{"id", "allow_no_reply"?: false}`                                                       | `{"sent": true}` or error `approval_required` |
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
| `feed.list`          | `{"project"?, "account_id"?}` — the Feed: open conversations as cards (card `id` is the conversation id, `thread` its newest thread), in sections | `Feed`                                   |
| `feed.texts`         | `{"ids": [thread id]}` — asks the account's allowed AI to write these cards' words in the background (cards on screen only); emits `feed_changed` per card | `{"queued": n}` |
| `feed.snooze`        | `{"ids": [id], "until": ms, "label"?}` — Later: the threads, with the other open threads of their conversations, leave the feed and their status columns until `until` (or new mail), then come back Unread and first in the feed with "You set this for <label>" | `{"snoozed": n}` |
| `later.options`      | `{}` — the Later picker's quick choices for now: this evening (before 17:00), tomorrow morning, this weekend (weekdays), Friday evening (Mon–Thu), next week | `[{"key": "f", "label": "Friday evening", "until": ms, "when": "Fri 18:00"}]` |
| `later.parse`        | `{"text"}` — plain French or English ("vendredi soir", "friday 7pm", "dans 3 jours", "15 nov"), read locally | `{"until": ms, "when": "Fri 18:00"}` or `null` |
| `feed.learn_archive` | `{"ids": [id]}` — the user archived these automated mails from a Feed card: a yes to "Archive" for each sender, as `sort.answer` records (three make a rule; ruled senders' mail is batched) | `{"learned": n}` |
| `feed.unsnooze`      | `{"ids": [id]}` — with the other open threads of their conversations                       | `{"unsnoozed": n}`                       |
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
| `drive.download` | `{"account_id", "file_id", "format"?: "pdf"\|"docx"\|"xlsx"\|"pptx"}` — a file's bytes saved on this Mac, to attach as a regular file (pass `path` to `drafts.create`/`drafts.attach`): stored files as they are (`alt=media`), Google files exported, to PDF unless `format` names another type the file allows; kept a day. Errors carry `data.reason`: `too_large` (over the 25 MB Gmail sends; nothing is downloaded when Drive gives the size), `no_file_form` (forms, folders), `no_access`, `failed` | `{"file_id", "path", "filename", "mime", "size"}` |
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

A send answers as soon as Gmail has the message; the sent copy is pulled into its
thread for up to 3 s, and the regular sync brings it otherwise. It never waits on
the label write-back queue. A Gmail refusal is the `upstream` error; a network
failure an error whose message says so (connection failures are retried first).

An agent's send to a To address that doesn't accept replies (see ReplyAddressing)
fails with `invalid_params` (`no_reply: … doesn't accept replies …`) unless it
passes `allow_no_reply: true`. Mermoz.app warns in the composer instead.

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
{"jsonrpc": "2.0", "method": "event", "params": {"kind": "do_progress", "id": "bop_…", "done": 400, "total": 1842}}
```

`chat_delta` carries the whole visible answer so far (never the actions block), at
most every 60 ms while `chat.send` streams; the final answer is `chat.send`'s result.
A Claude turn paused by its web search loop (`pause_turn`) is resumed automatically, up
to 3 times, into the same answer; past that, the answer so far is kept.
