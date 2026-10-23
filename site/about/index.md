<!-- https://mermoz.xyz/about/ · updated 2026-10-13 -->

# About Mermoz

Mermoz is an email app for the Mac. It connects to Gmail and Outlook, keeps your whole mailbox on your Mac, and treats every thread like a task with a status: Unread, To do, Waiting, Done. New mail arrives with a suggested status and project, one key moves a thread on, and search answers instantly, even offline.

It is also built for the agents you already use. Everything the app does is available from the `mermoz` command line and from a built-in MCP server, so Claude, Codex or your own scripts can search, read, sort and draft mail. Sending stays with you: an agent's draft waits in your Review lane until you approve it.

## Facts

- **Platform:** macOS 26 or later, downloaded from [GitHub Releases](https://github.com/ClementWalter/mermoz/releases).
- **Mail:** Gmail and Google Workspace accounts, Outlook, Hotmail, Live and Microsoft 365 accounts, several at once; Google Drive optional.
- **Status:** private beta (version 0.1.3) while Google reviews Mermoz's Gmail access. [Join the list](/#beta) and you'll be invited.
- **Price:** free. **License:** open source (MIT), [source on GitHub](https://github.com/ClementWalter/mermoz).
- **Built with:** a Rust daemon (`mermozd`) that owns sync and storage, a Rust CLI and MCP server, and a SwiftUI app.
- **Privacy:** no Mermoz server, no analytics; AI features are off until you turn them on. Details on the [privacy page](/privacy/).

## Who makes it

Mermoz is made by Clément Walter and developed in the open on GitHub, where every change, issue and release is public. Reach us through the [contact page](/contact/).

## The name

In 1930, Jean Mermoz flew the Aéropostale mail across the South Atlantic, from Senegal to Brazil, in a little over twenty hours. Letters that used to take weeks by sea arrived in days. The Aéropostale pilots had a motto: « Le courrier doit passer », the mail must go through. Mermoz keeps that spirit: mail that moves, light and fast, without the heavy machinery around it.

## For developers and agents

- [Developer docs](/docs/): the [CLI](/docs/cli/), the [MCP server](/docs/mcp/) and the [JSON-RPC protocol](/docs/protocol/).
- [llms.txt](/llms.txt): a summary of Mermoz for language models.
