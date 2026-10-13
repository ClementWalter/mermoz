---
path: /docs/mcp/
title: Mermoz MCP server: Gmail on Mac for Claude, Codex and other agents
description: Add the Mermoz MCP server (mermoz mcp, stdio) to Claude Code, Claude Desktop or Codex and let agents search, read, sort and draft your local Gmail mailbox.
updated: 2026-10-13
---
# Mermoz MCP server

`mermoz mcp` runs an MCP server on **stdio** that gives AI agents the user's Mermoz mailbox as {{mcp_tool_count}} tools: search, read, sort, draft, Drive files and more. It is local: it runs on the user's Mac, next to Mermoz.app, and talks to the app's daemon over a Unix socket. There is no hosted or remote (Streamable HTTP) endpoint: Mermoz has no server of its own that holds mail.

- **Requires:** Mermoz.app installed and running (macOS 26 or later), signed in to Gmail. If the app isn't running, tools answer "Mermoz isn't running. Ask the user to open Mermoz.app."
- **Command:** `mermoz mcp` once the CLI is in PATH (Mermoz › Settings › Developer › Install in PATH), or the full path `{{bundled_cli}} mcp`.
- **Protocol version:** {{mcp_protocol_version}}; capability `tools`.
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
      "command": "{{bundled_cli}}",
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

Launch `mermoz mcp` (or `{{bundled_cli}} mcp`) as a stdio server: newline-delimited JSON-RPC 2.0 on stdin and stdout. Discovery documents: [server.json](/server.json) (official MCP registry format) and [/.well-known/mcp.json](/.well-known/mcp.json).

## Tools

{{mcp_tools}}
