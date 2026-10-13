<!-- https://mermoz.xyz/docs/ · updated 2026-10-13 -->

# Mermoz developer docs

Mermoz keeps your Gmail mailbox on your Mac and lets your own scripts and AI agents work with it. Everything the app does, the command line does, and an MCP server gives agents the same mailbox as native tools. Search and reading are instant and offline. **Agents never send mail on their own**: a send waits in the Review lane until you approve it in Mermoz.

## How the pieces fit

- **`mermozd`**, the daemon inside Mermoz.app, is the only process that touches the network and the mailbox database.
- **`mermoz`**, the command-line tool, and **`mermoz mcp`**, the MCP server, are clients of the daemon over a local Unix socket. Mermoz.app must be running.
- The CLI ships inside the app at `/Applications/Mermoz.app/Contents/Resources/mermoz`. In Mermoz, open **Settings › Developer › Command line tool › Install in PATH** to link it as `mermoz` (into `/usr/local/bin` or `~/.local/bin`).

## Pages

- [Mermoz CLI reference](/docs/cli/): every `mermoz` command, with its `--help` text and exit codes.
- [Mermoz MCP server](/docs/mcp/): add Mermoz to Claude Code, Claude Desktop or Codex; the 29 tools and their parameters.
- [Mermoz JSON-RPC protocol](/docs/protocol/): the `mermozd` socket protocol, for building your own client.

## Quick start for agents

```sh
mermoz list --status todo --json          # threads waiting on the user
mermoz search "from:marc audit" --json    # instant, offline full-text search
mermoz thread M-1287 --json               # read a whole thread
mermoz draft reply M-1287 --file r.txt    # draft a reply (never sends)
mermoz send dft_8f2c                      # exit 3: waits for the user's approval
claude mcp add mermoz -- mermoz mcp       # or plug the MCP server into Claude Code
```

Machine-readable summaries: [llms.txt](/llms.txt), [llms-full.txt](/llms-full.txt), [server.json](/server.json) (MCP registry format) and [/.well-known/mcp.json](/.well-known/mcp.json).
