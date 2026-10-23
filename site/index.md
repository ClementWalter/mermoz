<!-- https://mermoz.xyz/ · updated 2026-10-13 -->

# Mermoz: fast, agent-first email for Mac (Gmail)

**The mail must go through.** Mermoz is a light, fast email app for macOS, named after the Aéropostale pilot Jean Mermoz. New mail is sorted before you read it. One key handles a thread. Your whole mailbox lives on your Mac, quiet and instant.

Private beta · free and open source · macOS 26 or later · Gmail · version 0.1.3

- Join the private beta: [mermoz.xyz/#beta](https://mermoz.xyz/#beta) (leave the Gmail address you'll use).
- Already invited? [Download Mermoz for Mac](https://github.com/ClementWalter/mermoz/releases/latest/download/Mermoz.dmg).
- Source: [https://github.com/ClementWalter/mermoz](https://github.com/ClementWalter/mermoz).

## Built for a lighter workflow

An inbox that behaves like a flight plan: everything has a status, a destination, and a key to get it there.

- **Sorted before you read it.** Each new email arrives with a suggested status and project. Read it and the suggestion applies, or press `y`.
- **One key, done.** `e` done, `a` archive, `⌫` delete, `⌘Z` undo. Select several with `⇧` and move them together.
- **Flows, not folders.** Unread, To do, Waiting, Done. A list or a board, grouped and ordered your way. Your Gmail labels are your projects.
- **Read what matters.** Original, exactly as sent. Or Optimized: the same words without signatures, disclaimers and quoted history. Made once, kept.
- **Your whole mailbox, instant.** Every message lives on your Mac. Search, read and sort offline, with `⌘K` for anything else.
- **Agent first.** A first-class command line and MCP server. Your agents search, sort and draft; you approve what leaves.

## Your agents fly Mermoz too

Everything the app does, the command line does: search, read, sort, draft. Every command speaks `--json`, and an MCP server plugs Mermoz into Claude, Codex and any agent you run.

- One command per action, scriptable end to end.
- MCP server built in: `mermoz mcp` (stdio). See the [MCP docs](https://mermoz.xyz/docs/mcp/).
- Agents can draft anything. Nothing is sent until you approve it in Mermoz. There is no `--yes`.
- Agents report friction with `mermoz feedback`, so the tool keeps improving.

```text
$ mermoz list --status todo --json
{ "threads": [{ "number": "M-1287", "subject": "Q4 audit — sign by Friday?" }], "total": 1 }
$ mermoz draft reply M-1287 --file reply.md
dft_42  [draft]
$ mermoz send dft_42
error: approval_required: draft dft_42 is waiting for the user to approve it in Mermoz.   # exit 3
```

## The spirit of Mermoz

In 1930, Jean Mermoz flew the Aéropostale mail across the South Atlantic, from Senegal to Brazil, in a little over twenty hours. The Aéropostale pilots had a motto: « Le courrier doit passer », the mail must go through. Mermoz keeps that spirit: mail that moves, light and fast, without the heavy machinery around it.

## More

- [Developer docs](https://mermoz.xyz/docs/) · [CLI](https://mermoz.xyz/docs/cli/) · [MCP server](https://mermoz.xyz/docs/mcp/) · [Protocol](https://mermoz.xyz/docs/protocol/)
- [About](https://mermoz.xyz/about/) · [Contact](https://mermoz.xyz/contact/) · [Privacy](https://mermoz.xyz/privacy/)
- [llms.txt](https://mermoz.xyz/llms.txt)
