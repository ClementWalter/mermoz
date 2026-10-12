# Mermoz — Product & Technical Spec

A fast, local-first, agent-native email client for macOS. Email threads are
tracked as issues (Linear-style), not as a pile in an inbox.

## Principles

1. **Speed above features.** If a feature can't meet the speed bar, it doesn't
   ship. No support beats slow support.
2. **Local first.** The full mailbox (bodies + attachments) lives on disk. Every
   read is local; the network only syncs.
3. **Agent native.** Anything the UI can do, the CLI/MCP can do — except send
   without human authorization.
4. **Nothing gets lost.** Every thread has a status. Mail doesn't stack, it
   flows through a workflow.
5. **Minimal.** Three panes, keyboard-first, Liquid Glass. No chrome that
   doesn't earn its place.
6. **Plain language.** The app is for everyday users: the UI never shows
   terminals, CLI/MCP, model ids or endpoints. Agents are just "agents";
   providers are "Work AI", "Personal AI". Developer surfaces (CLI install,
   MCP setup, agent permissions, agent feedback log) live only in a separate
   Settings › Developer section.

## Target

- macOS 26 Tahoe only (native Liquid Glass, no fallback paths).
- Built for everyday users: a single `Mermoz.app` download, nothing to configure
  beyond signing in.
- Open source. Direct download, Developer ID signed + notarized, Sparkle updates.
  Not on the Mac App Store (the sandbox conflicts with daemon/CLI/MCP).

## Speed bar (enforced as CI benchmarks; regressions fail the build)

| Operation                              | Budget              |
| -------------------------------------- | ------------------- |
| Cold launch → interactive list         | < 300 ms            |
| Open thread                            | < 16 ms (one frame) |
| Local full-text search, 500k messages  | < 50 ms             |
| Any CLI read command                   | < 30 ms             |
| Status change / archive / label        | instant, optimistic |

## Architecture

```
 Mermoz.app (SwiftUI)        mermoz CLI         mermoz mcp
        │                        │                   │
        └──────────── Unix socket (JSON-RPC) ────────┘
                              │
                     mermozd (Rust, launchd agent)
          sync engine · SQLite store · FTS index · agent runner
                              │
                Gmail API  ·  (IMAP, only if fast enough)
```

- **mermozd** — Rust daemon registered via `SMAppService`. Owns sync, the
  SQLite store (WAL), full-text index, attachment blob store, the agent runner,
  and the send gate. Runs while the app is closed so the CLI is always live.
- **Mermoz.app** — thin SwiftUI client. Never touches the network or DB
  directly; subscribes to daemon change events for live updates.
- **mermoz CLI** — Rust binary shipped inside the app bundle; "Install command
  line tool" in settings symlinks it into `PATH`.
- **mermoz mcp** — MCP server (stdio) exposing the same operations as tools.

### Accounts & sync

- **Gmail / Workspace (MVP):** Gmail API, incremental sync via `history.list`.
  New-mail detection: IMAP IDLE connection used purely as a doorbell + history
  polling (15–30 s foreground, 2–5 min background).
- **No server.** Mermoz has no hosted component of any kind.
- **Generic IMAP:** only if it meets the speed bar (CONDSTORE/QRESYNC/IDLE
  required). Otherwise not supported.
- Credentials and API keys in the macOS Keychain.

### Search

Local full-text index owned by mermozd. SQLite FTS5 and Tantivy are both
benchmarked against the 50 ms / 500k-message budget; the faster one that meets
it wins (Tantivy if FTS5 can't, e.g. for fuzzy/prefix queries).

### Rendering & compose

- HTML: sanitized, single reused pre-warmed `WKWebView`; remote images blocked
  by default (privacy + speed).
- Composer: plain text. An LLM "magnify" action polishes/expands the draft on
  request. Sent as plain text (+ minimal HTML alternative when magnified output
  has structure).
- Recipient check: every address gets a mark as it's typed — green tick when
  it looks legitimate, red tick (with a one-line reason) for random-looking or
  newly registered domains, disposable/spam domains, malformed addresses and
  lookalikes of a known contact (e.g. a familiar name on an unfamiliar
  domain). Fast local signals first (contact history, edit distance to known
  domains, MX lookup cache), AI only for the ambiguous rest, so the tick never
  waits on the network. A red tick on a send triggers a confirmation.

## Issue model

- **Unit:** a thread is an issue.
- **Statuses:** Linear-style, configurable. Default set: Triage, Backlog, Todo,
  In Progress, Waiting, Review (outgoing mail pending approval), Done, Canceled.
- **Reopen rule:** a reply from someone else on a Done/Waiting thread moves it
  to Todo automatically.
- **Fields:** status, priority (Urgent/High/Normal/Low), project/tags,
  due/follow-up date (Waiting past due resurfaces the thread), agent summary +
  next action.
- **Multi-account:** one unified issue space across all accounts. Account is
  just another field: group by / filter by account like any other field.
- **Server mapping:** status/priority/project map to Gmail labels
  (`mermoz/status/todo`, …) so state survives reinstall and is visible on
  other devices. Done = archive.

## Workflow v2 (decided 2026-10-11, supersedes the issue model above)

Two philosophies were competing: the mailbox (read/unread, inbox/archive, labels)
and the tracker (statuses, projects). v2 merges them into one axis.

### One status axis

```
Unread ──► (read) To do · Waiting · In progress · Backlog · Done · Archive
                                                             ╲── Deleted
```

- **Unread** is the entry status; every other status is a refinement of "read".
- **Done** = "I handled it" (reopens when someone else replies). **Archive** = "filed,
  nothing to do". Both are kept forever; nothing moves automatically between them.
- **Deleted** = Gmail Trash (Gmail purges it after 30 days, its own rule).

| Status | Gmail |
| --- | --- |
| Unread | `UNREAD` + `INBOX` |
| To do / Waiting / In progress / Backlog | `INBOX` + `Mermoz/<Status>` |
| Done | archived + `Mermoz/Done` |
| Archive | archived, no status label |
| Deleted | Trash |

### Labels are the projects

The user's Gmail labels *are* the projects: no `Mermoz/Project/…` prefix, no
parallel taxonomy. Colors, boards and counts apply to existing labels. A thread
can carry several labels, so it belongs to several projects and shows in each
project's column.

### Instant AI routing

- On arrival a fast **decision model** predicts, per thread: post-read status,
  project among the user's labels, priority and "automated", with probabilities.
  It is TypeSafe Jev with the user's own key (Settings › Developer); OpenAI's
  Decisions API stays a candidate once access is confirmed.
- Jev sees the thread plus two hints built locally: where earlier threads from
  the same senders were filed, and a one-line description of every project
  (frequent correspondents, recent subjects). Measured on a real mailbox (300
  filed threads, 68 labels): 78% exact project, 90% exact on the 80% of threads
  where confidence ≥ 0.7, ~250 ms per thread. Only confident fields are suggested.
- Apple's on-device model was evaluated and rejected for routing: 4,096-token
  context (project descriptions don't fit), ~2 s per call, and barely better
  than the free sender-history rule (58% vs 54%).
- The prediction is a **suggestion**, never applied silently: reading the thread
  confirms it, one key corrects it. Automated mail (newsletters, receipts,
  notifications) follows the same rule, confirmable in batches.
- Every correction is a labeled example; the learning loop turns them into
  per-sender shortcuts and few-shot context.
- A thread read before its prediction arrives defaults to the sender's most
  frequent status, else To do.
- The big LLM keeps the slow jobs: clean reading view, drafts and directory.

### Built (2026-10-11)

Statuses, Gmail mapping (Trash both ways), labels as projects (explicit add/remove
pushes so labels set elsewhere survive), the read transition, Jev routing with
suggestions shown on rows and cards, `y` / "Accept n" to accept, an Undo bar after
reading applied a suggestion, and the key in Settings › Developer.

### Background sorting of open columns (built 2026-10-12)

Jev also works through every already-read thread in To do, Waiting, In progress
and Backlog that has no current verdict, in the background: 12 requests in
flight, the column on screen first, then newest first, after new Unread mail.
A thread is asked again only when new mail arrives or it lands in another column
without the user deciding; a thread the user moved after its suggestion is left
alone. Only confident (≥ 0.7) changes are suggested — a different status, or
projects the thread is not in yet; "no change" is recorded and never shown.

Each column carries a bar on top: "✦ 312 suggestions · Accept all", one button
per destination ("→ Archive · Receipts 240"), and "Sorting… 1,240 / 7,256" while
the pass is still going. Clicking "n suggestions · Review" lists every suggested
thread of the column grouped by destination; clicking a destination chip lists only
its threads ("Reviewing → Archive · 758", × or Esc to leave). The reading pane shows
the selected thread; `y` accepts it, `x` (or × on its chip) dismisses it — it stays
put and is not suggested again — and both move to the next row. "Accept these n",
"Accept n selected" and the quicker "Accept all" are each one ⌘Z away. Nothing moves
until accepted. Cost: ~7,500 input tokens per call (~$2.30 for a 7,000-thread
Backlog); a daily allowance (default 20,000 calls, Settings › Developer) is a
safety net, 429s honour Retry-After, outages back off exponentially, and token
use is logged. This replaces the big-LLM column triage button.

## UI

- **Sidebar:** statuses, projects, saved views (Linear-style); raw
  labels/folders tucked below.
- **Middle:** issue list or board (toggle), group by / filter by any field
  (status, priority, project, account, sender, due date…). Saved views persist
  a grouping + filter combination.
- **Right:** thread content.
- **Keyboard-first:** j/k navigation, single-key status/priority changes, ⌘K
  command palette (also hosts "ask your mailbox").

## Agents

### External harnesses (MVP)

CLI with `--json` everywhere, non-interactive, and MCP. Core commands:

```
mermoz accounts | search <query> | thread <id> | message <id>
mermoz attachments <id> [--out DIR]     # agents can read all attachments
mermoz status <thread> <status> | priority | project | due
mermoz draft new|reply <thread> [--file F]
mermoz send <draft>                     # gated, see below
mermoz feedback <json>                  # agent reports CLI friction
```

### Send gate

- No CLI flag can authorize a send. There is no `--yes`.
- Default: `mermoz send` moves the draft to the **Review** status and exits
  with a distinct code and an explicit error (`approval_required: draft <id>
  is waiting for the user to approve it in Mermoz — tell the user`), so the
  agent bubbles it up.
- The user approves/rejects from the Review lane (or a notification).
- A global UI toggle "Allow agents to send without review" enables direct
  sending for users who want it. Only settable from the app UI.

### `mermoz feedback`

Agents submit structured friction reports (missing flag, unclear error, slow
command). Logged locally, viewable/exportable from settings, to drive CLI
improvements.

### Built-in agent (post-MVP)

- Providers: Anthropic + any OpenAI-compatible endpoint (OpenRouter, Ollama,
  company gateways). Multiple providers configured at once.
- **Account → allowed providers** (multi-select): e.g. Work → only the
  company's Claude and Codex; Personal → any provider. The agent picks and
  falls back only within an account's allowed set — mail never reaches a
  provider not allowed for its account. An account with none never leaves the
  device.
- Runs: on arrival (summary, next action, suggested status/priority/project,
  optional draft), on demand (draft/summarize/ask on current thread), follow-up
  sweeps (Waiting past due → draft a nudge), chat over mailbox (⌘K).
- Agent drafts follow the same send gate.
- **Clean view**: "Show formatted" can ask the account's allowed provider to
  rewrite a message for reading — drop client cruft, useless signatures and
  disclaimers, fix broken formatting — never changing meaning. Cached per
  message; the original is one click away. Deterministic trimming (quoted
  history, `-- ` signatures, mobile footers, disclaimers) already ships.
- **No hand-written rules.** Newsletters, notifications and other noise are
  handled by the built-in agent's on-arrival triage, not by a rules engine.

### Learning loop

The built-in agent learns from the user, locally:

- Every user override of an agent suggestion (status, priority, project,
  ignore/keep) is recorded as a labeled example.
- Diffs between an agent draft and what the user actually sent are recorded
  as style examples.
- Examples are distilled into a per-account preferences file (readable and
  editable in settings) that is injected into every agent prompt; recent
  relevant examples are retrieved as few-shot context.
- Senders/patterns with consistent user decisions become cheap local
  shortcuts (no LLM call), keeping triage fast and token-light.

## MVP scope

1. Gmail sync + read + search + status board.
2. Compose / reply / drafts + send approval (Review lane).
3. CLI + MCP for external agents, including `feedback`.

Post-MVP: built-in agent + learning loop, IMAP (if fast).

## Open risks

- **Google OAuth verification.** Gmail read/modify scopes are *restricted*. A
  public app needs Google verification, and Google's current docs state an
  annual security assessment (CASA) for restricted scopes. Google's 2018 policy
  exempted apps storing data only on-device from the full assessment; whether
  that still holds today is unconfirmed — must be verified with Google before
  launch. Built-in agent sending mail to LLM providers may affect any
  local-only exemption.
- **Disk usage.** Full mirror incl. attachments can reach tens of GB; need a
  visible storage indicator and possibly an attachment age cap setting.
- **IDLE-as-doorbell reliability** for Gmail; polling is the fallback (no
  server, by design).

## MVP status (2026-10-11)

Shipped: Gmail sync (resumable import, history-based incremental, offline
label write-back), issue workflow + reopen rule, search, list/board, thread
view, compose + recipient check, Review lane send gate, CLI (`--json`
everywhere, exit 3 = approval required), MCP server, Developer settings.
Speed bar met at 500k messages (`cargo run --release -p mermoz-core --example bench`).

Deviations / next:
- New mail is detected by history polling (20 s); the IMAP IDLE doorbell is next.
- Search returns most recent matches first (date-ordered FTS) rather than
  bm25 rank — it keeps every query under budget at 500k messages.
- Built-in agent v1 ships: providers (Anthropic + OpenAI-compatible, keys in Keychain),
  per-account allowed providers, and per-message clean view + People directory built
  from signatures (opt-in per account; new mail + last 30 days). Magnify, triage
  suggestions and the learning loop are next.
- The app starts `mermozd` itself; SMAppService login item comes with signing.
- Build: `scripts/build_app.py --open`. Dev: `MERMOZ_HOME`, `MERMOZ_SOCKET`,
  `MERMOZ_NO_SYNC=1` with `--example seed_demo` for fictional demo data.
