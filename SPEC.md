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
  polling (15–30 s foreground, 2–5 min background). No server.
- **Optional later:** stateless push relay (Gmail Pub/Sub → client). Pub/Sub
  payloads only carry `{email, historyId}`, so no mail content touches it.
- **Generic IMAP:** only if it meets the speed bar (CONDSTORE/QRESYNC/IDLE
  required). Otherwise not supported.
- Credentials and API keys in the macOS Keychain.

### Rendering & compose

- HTML: sanitized, single reused pre-warmed `WKWebView`; remote images blocked
  by default (privacy + speed).
- Composer: plain text. An LLM "magnify" action polishes/expands the draft on
  request. Sent as plain text (+ minimal HTML alternative when magnified output
  has structure).

## Issue model

- **Unit:** a thread is an issue.
- **Statuses:** Linear-style, configurable. Default set: Triage, Backlog, Todo,
  In Progress, Waiting, Review (outgoing mail pending approval), Done, Canceled.
- **Reopen rule:** a reply from someone else on a Done/Waiting thread moves it
  to Todo automatically.
- **Fields:** status, priority (Urgent/High/Normal/Low), project/tags,
  due/follow-up date (Waiting past due resurfaces the thread), agent summary +
  next action.
- **Server mapping:** status/priority/project map to Gmail labels
  (`mermoz/status/todo`, …) so state survives reinstall and is visible on
  other devices. Done = archive.

## UI

- **Sidebar:** statuses, projects, saved views (Linear-style); raw
  labels/folders tucked below.
- **Middle:** issue list or board (toggle), grouped/filtered by any field.
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
- **Account → provider mapping**: e.g. work mail → company provider, personal
  mail → personal key. An account with no provider never leaves the device.
- Runs: on arrival (summary, next action, suggested status/priority/project,
  optional draft), on demand (draft/summarize/ask on current thread), follow-up
  sweeps (Waiting past due → draft a nudge), chat over mailbox (⌘K).
- Agent drafts follow the same send gate.

## MVP scope

1. Gmail sync + read + search + status board.
2. Compose / reply / drafts + send approval (Review lane).
3. CLI + MCP for external agents, including `feedback`.

Post-MVP: built-in agent, IMAP (if fast), push relay.

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
- **IDLE-as-doorbell reliability** for Gmail; fall back to polling, relay
  later if needed.
