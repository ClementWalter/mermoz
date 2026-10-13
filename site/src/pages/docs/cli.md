---
path: /docs/cli/
title: Mermoz CLI reference: the mermoz command for Gmail on Mac
description: Every command of the mermoz command-line tool: search, list, read, sort and draft your Gmail from the terminal, with --json output for agents.
updated: 2026-10-13
---
# Mermoz CLI reference

`mermoz` is the command line of Mermoz: the same mailbox as the app, instant and offline, for people and agents. Every command is non-interactive and takes `--json` for machine-readable output. Mermoz.app must be running (the CLI talks to its daemon); otherwise commands exit with code 5.

**Install:** the CLI ships inside the app at `{{bundled_cli}}`. In Mermoz, **Settings › Developer › Command line tool › Install in PATH** links it as `mermoz`.

**Sending:** `mermoz send` never sends by itself. Unless the user turned on agent sending, the draft moves to the Review lane and the command exits with code 3 (`approval_required`): tell the user, do not retry. There is no `--yes`.

## Exit codes

| code | meaning |
| --- | --- |
| 0 | ok |
| 1 | error |
| 2 | usage |
| 3 | approval required: tell the user |
| 4 | not found |
| 5 | Mermoz is not running |

## Overview

{{cli_overview}}

## Commands

{{cli_commands}}
