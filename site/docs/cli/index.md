<!-- https://mermoz.xyz/docs/cli/ · updated 2026-10-13 -->

# Mermoz CLI reference

`mermoz` is the command line of Mermoz: the same mailbox as the app, instant and offline, for people and agents. Every command is non-interactive and takes `--json` for machine-readable output. Mermoz.app must be running (the CLI talks to its daemon); otherwise commands exit with code 5.

**Install:** the CLI ships inside the app at `/Applications/Mermoz.app/Contents/Resources/mermoz`. In Mermoz, **Settings › Developer › Command line tool › Install in PATH** links it as `mermoz`.

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

```text
Your Mermoz mailbox from the terminal. Instant, offline, agent-friendly.

Usage: mermoz [OPTIONS] <COMMAND>

Commands:
  accounts      List connected accounts and their sync state
  add-account   Connect a Gmail account (opens the browser).
  reconnect     Sign an account in to Google again, keeping its mail (opens the browser). Needed when `mermoz accounts` says "sign in again"
  list          List threads, newest first
  labels        List your Gmail labels ("mailboxes") with thread counts
  contacts      Search the directory built from email signatures (name, email or company)
  contact       Show one person's card and recent threads
  search        Full-text search: every word must match, as a whole word, accents and case aside; from:, to:, subject: restrict to a field. Best matches first
  thread        Show a thread with all messages
  conversation  Split or join conversations (Gmail threads read as one chat)
  files         Files shared in any conversation, newest first, optionally matching a name
  attachments   List a thread's attachments, or save them with --out
  status        Set the status of one or more threads
  priority      Set priority: none, urgent, high, normal, low
  project       Add a thread to a project (a Gmail label, created if missing); "none" clears all
  accept        Accept the AI's suggested status and projects for threads (marks them read), or for a whole column with --column (optionally only one --to/--to-project group)
  reject        Turn down the AI's suggestion for threads: they stay where they are and are not suggested again until new mail arrives
  resort        Stop a whole-mailbox sort and clear the suggestions it left that nobody reviewed. Accepted and turned-down suggestions are kept
  suggestions   The AI's pending suggestions in a status column, grouped by where they go, and how far background sorting of that column has got; with --to/--to-project, the threads of that one group (accept them with `mermoz accept`)
  due           Set a follow-up date (YYYY-MM-DD); "none" clears
  read          Mark a thread read or unread
  draft         Create and manage drafts
  drive         Google Drive files of a thread, their text, search, saving attachments. Connecting Drive and sharing files happen in the app only
  send          Ask to send a draft. Unless enabled by the user, it waits for approval in Mermoz (exit 3)
  statuses      Workflow statuses
  projects      Projects
  feedback      Report friction with this CLI so it can improve
  chat          Ask the account's AI about a thread (kept with the thread, like in the app). Without a question, shows the conversation so far. Proposals are listed, never applied
  feed          The Feed: open mail as cards, one thing to do each, in order (now, today, this week, waiting on others, can wait)
  later         Put a thread off until a time said in plain words ("vendredi soir", "friday 7pm", "dans 3 jours", "15 nov"; default tomorrow morning). It leaves its column and comes back Unread then, or when new mail arrives. "none" brings it back now
  router        Instant sorting: the provider in use, whether it has a key, and today's calls. Keys and the provider are changed in the app (Settings › Developer)
  sync          Trigger an immediate sync
  mcp           Run an MCP server on stdio
  help          Print this message or the help of the given subcommand(s)

Options:
      --json     Machine-readable JSON output
  -h, --help     Print help
  -V, --version  Print version

Examples:
  mermoz list --status todo                 Threads waiting on you
  mermoz search "from:marc audit" --json    Full-text search, JSON output
  mermoz thread M-1287                      Read a whole thread
  mermoz thread M-1287 --conversation       Read every thread of its conversation
  mermoz list --conversations --status todo One row per conversation
  mermoz conversation split M-1290          Take a thread out of its conversation
  mermoz files invoice                      Files shared anywhere, by name
  mermoz attachments M-1287 --out /tmp/a    Save a thread's attachments
  mermoz status M-1287 waiting              Change a thread's status
  mermoz draft reply M-1287 --file r.txt    Draft a reply (never sends)
  mermoz draft new --to a@x.com --body hi --attach plan.pdf --attach photo.jpg
                                            Draft with files (25 MB in total at most)
  mermoz send dft_8f2c                      Ask to send; the user approves in Mermoz
  mermoz chat M-1287 "should I accept?"     Ask the AI about a thread (proposals only)

Exit codes: 0 ok · 1 error · 2 usage · 3 approval required (tell the user) · 4 not found · 5 Mermoz not running
```

## Commands

### mermoz accounts

List connected accounts and their sync state

```text
List connected accounts and their sync state

Usage: mermoz accounts [OPTIONS]

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz add-account

Connect a Gmail account (opens the browser).

```text
Connect a Gmail account (opens the browser).

A company Google Workspace that blocks Mermoz can sign in through its own
Google Cloud OAuth client (Desktop app) instead:
  mermoz add-account --client-json ~/Downloads/client_secret_….json
  mermoz add-account --client-id ID --client-secret SECRET
That client is stored with the account and used for its Drive too.

Usage: mermoz add-account [OPTIONS]

Options:
      --client-json <CLIENT_JSON>
          The client_secret_*.json downloaded from Google Cloud

      --json
          Machine-readable JSON output

      --client-id <CLIENT_ID>
          OAuth client ID (with --client-secret)

      --client-secret <CLIENT_SECRET>
          OAuth client secret (with --client-id)

      --wait
          Wait for the browser sign-in to finish and say how it went

  -h, --help
          Print help (see a summary with '-h')
```

### mermoz reconnect

Sign an account in to Google again, keeping its mail (opens the browser). Needed when `mermoz accounts` says "sign in again"

```text
Sign an account in to Google again, keeping its mail (opens the browser). Needed when `mermoz accounts` says "sign in again"

Usage: mermoz reconnect [OPTIONS] <ACCOUNT>

Arguments:
  <ACCOUNT>  Account id or email

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz list

List threads, newest first

```text
List threads, newest first

Usage: mermoz list [OPTIONS]

Options:
      --json               Machine-readable JSON output
      --status <STATUS>    Status key(s), comma separated (unread, todo, waiting, archive, ...)
      --project <PROJECT>  Project, i.e. Gmail label name (e.g. "Clients/Acme")
      --account <ACCOUNT>  Account email or id
      --label <LABEL>      Gmail label ("mailbox") name, e.g. "Clients/Acme"
      --conversations      One row per conversation (the thread leading it), as in the app
      --feeds              Only automated senders and mailing lists (implies --conversations)
      --limit <LIMIT>      [default: 50]
  -h, --help               Print help
```

### mermoz labels

List your Gmail labels ("mailboxes") with thread counts

```text
List your Gmail labels ("mailboxes") with thread counts

Usage: mermoz labels [OPTIONS]

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz contacts

Search the directory built from email signatures (name, email or company)

```text
Search the directory built from email signatures (name, email or company)

Usage: mermoz contacts [OPTIONS] [QUERY]...

Arguments:
  [QUERY]...  

Options:
      --json           Machine-readable JSON output
      --limit <LIMIT>  [default: 50]
  -h, --help           Print help
```

### mermoz contact

Show one person's card and recent threads

```text
Show one person's card and recent threads

Usage: mermoz contact [OPTIONS] <EMAIL>

Arguments:
  <EMAIL>  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz search

Full-text search: every word must match, as a whole word, accents and case aside; from:, to:, subject: restrict to a field. Best matches first

```text
Full-text search: every word must match, as a whole word, accents and case aside; from:, to:, subject: restrict to a field. Best matches first

Usage: mermoz search [OPTIONS] [QUERY]...

Arguments:
  [QUERY]...  

Options:
      --json           Machine-readable JSON output
      --limit <LIMIT>  [default: 20]
      --gmail          Search all of Gmail instead (Gmail's own syntax), for mail Mermoz doesn't hold
  -h, --help           Print help
```

### mermoz thread

Show a thread with all messages

```text
Show a thread with all messages

Usage: mermoz thread [OPTIONS] <ID>

Arguments:
  <ID>  

Options:
      --conversation  Every message of the thread's conversation (Gmail threads read as one chat)
      --json          Machine-readable JSON output
  -h, --help          Print help
```

### mermoz conversation

Split or join conversations (Gmail threads read as one chat)

```text
Split or join conversations (Gmail threads read as one chat)

Usage: mermoz conversation [OPTIONS] <COMMAND>

Commands:
  split  Take a thread out of its conversation for good
  merge  Join the conversations of these threads into one
  help   Print this message or the help of the given subcommand(s)

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz conversation split

Take a thread out of its conversation for good

```text
Take a thread out of its conversation for good

Usage: mermoz conversation split [OPTIONS] <THREAD>

Arguments:
  <THREAD>  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz conversation merge

Join the conversations of these threads into one

```text
Join the conversations of these threads into one

Usage: mermoz conversation merge [OPTIONS] <THREADS> <THREADS>...

Arguments:
  <THREADS> <THREADS>...  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz files

Files shared in any conversation, newest first, optionally matching a name

```text
Files shared in any conversation, newest first, optionally matching a name

Usage: mermoz files [OPTIONS] [QUERY]...

Arguments:
  [QUERY]...  

Options:
      --json           Machine-readable JSON output
      --limit <LIMIT>  [default: 50]
  -h, --help           Print help
```

### mermoz attachments

List a thread's attachments, or save them with --out

```text
List a thread's attachments, or save them with --out

Usage: mermoz attachments [OPTIONS] <THREAD>

Arguments:
  <THREAD>  

Options:
      --json       Machine-readable JSON output
      --out <OUT>  
  -h, --help       Print help
```

### mermoz status

Set the status of one or more threads

```text
Set the status of one or more threads

Usage: mermoz status [OPTIONS] <ARGS> <ARGS>...

Arguments:
  <ARGS> <ARGS>...  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz priority

Set priority: none, urgent, high, normal, low

```text
Set priority: none, urgent, high, normal, low

Usage: mermoz priority [OPTIONS] <THREAD> <PRIORITY>

Arguments:
  <THREAD>    
  <PRIORITY>  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz project

Add a thread to a project (a Gmail label, created if missing); "none" clears all

```text
Add a thread to a project (a Gmail label, created if missing); "none" clears all

Usage: mermoz project [OPTIONS] <THREAD> <PROJECT>

Arguments:
  <THREAD>   
  <PROJECT>  

Options:
      --json    Machine-readable JSON output
      --remove  Take the thread out of the project instead
  -h, --help    Print help
```

### mermoz accept

Accept the AI's suggested status and projects for threads (marks them read), or for a whole column with --column (optionally only one --to/--to-project group)

```text
Accept the AI's suggested status and projects for threads (marks them read), or for a whole column with --column (optionally only one --to/--to-project group)

Usage: mermoz accept [OPTIONS] [THREADS]...

Arguments:
  [THREADS]...  

Options:
      --column <COLUMN>           Every pending suggestion of this status column
      --json                      Machine-readable JSON output
      --to <TO>                   Only the group going to this status ("none": groups that only add projects)
      --to-project <TO_PROJECTS>  Only the group adding exactly these projects (repeat for several)
  -h, --help                      Print help

Examples:
  mermoz accept M-12 M-14
  mermoz accept --column backlog
  mermoz accept --column backlog --to archive --to-project Receipts
```

### mermoz reject

Turn down the AI's suggestion for threads: they stay where they are and are not suggested again until new mail arrives

```text
Turn down the AI's suggestion for threads: they stay where they are and are not suggested again until new mail arrives

Usage: mermoz reject [OPTIONS] <THREADS>...

Arguments:
  <THREADS>...  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz resort

Stop a whole-mailbox sort and clear the suggestions it left that nobody reviewed. Accepted and turned-down suggestions are kept

```text
Stop a whole-mailbox sort and clear the suggestions it left that nobody reviewed. Accepted and turned-down suggestions are kept

Usage: mermoz resort [OPTIONS] --stop

Options:
      --json  Machine-readable JSON output
      --stop  
  -h, --help  Print help

Examples:
  mermoz resort --stop
```

### mermoz suggestions

The AI's pending suggestions in a status column, grouped by where they go, and how far background sorting of that column has got; with --to/--to-project, the threads of that one group (accept them with `mermoz accept`)

```text
The AI's pending suggestions in a status column, grouped by where they go, and how far background sorting of that column has got; with --to/--to-project, the threads of that one group (accept them with `mermoz accept`)

Usage: mermoz suggestions [OPTIONS] [STATUS]

Arguments:
  [STATUS]  

Options:
      --clear                     Clear every suggestion on read threads that nobody reviewed (Unread mail keeps its own); accepted and turned-down ones are kept
      --json                      Machine-readable JSON output
      --to <TO>                   List the threads going to this status ("none": groups that only add projects)
      --to-project <TO_PROJECTS>  List the threads going to exactly these projects (repeat for several)
      --limit <LIMIT>             Most threads to list [default: 50]
  -h, --help                      Print help

Examples:
  mermoz suggestions backlog
  mermoz suggestions backlog --to archive
  mermoz suggestions backlog --to none --to-project Receipts
```

### mermoz due

Set a follow-up date (YYYY-MM-DD); "none" clears

```text
Set a follow-up date (YYYY-MM-DD); "none" clears

Usage: mermoz due [OPTIONS] <THREAD> <DATE>

Arguments:
  <THREAD>  
  <DATE>    

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz read

Mark a thread read or unread

```text
Mark a thread read or unread

Usage: mermoz read [OPTIONS] <THREAD>

Arguments:
  <THREAD>  

Options:
      --json    Machine-readable JSON output
      --unread  
  -h, --help    Print help
```

### mermoz draft

Create and manage drafts

```text
Create and manage drafts

Usage: mermoz draft [OPTIONS] <COMMAND>

Commands:
  reply   Draft a reply; recipients and subject default from the thread
  new     Draft a new message
  attach  Add files to a draft
  detach  Remove an attachment from a draft (ids are shown by `draft show`)
  list    List drafts (optionally by state: draft, review, rejected)
  show    Show a draft
  edit    Replace a draft's body
  delete  Delete a draft
  help    Print this message or the help of the given subcommand(s)

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz draft reply

Draft a reply; recipients and subject default from the thread

```text
Draft a reply; recipients and subject default from the thread

Usage: mermoz draft reply [OPTIONS] <THREAD>

Arguments:
  <THREAD>  

Options:
      --body <BODY>    
      --json           Machine-readable JSON output
      --file <FILE>    Read the body from a file ("-" for stdin)
      --attach <PATH>  Attach a file; repeat for several
  -h, --help           Print help
```

### mermoz draft new

Draft a new message

```text
Draft a new message

Usage: mermoz draft new [OPTIONS] --to <TO>

Options:
      --json               Machine-readable JSON output
      --to <TO>            
      --cc <CC>            
      --subject <SUBJECT>  [default: ""]
      --body <BODY>        
      --file <FILE>        
      --account <ACCOUNT>  Account email or id to send from
      --attach <PATH>      Attach a file; repeat for several
  -h, --help               Print help
```

### mermoz draft attach

Add files to a draft

```text
Add files to a draft

Usage: mermoz draft attach [OPTIONS] <ID> <PATH>...

Arguments:
  <ID>       
  <PATH>...  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz draft detach

Remove an attachment from a draft (ids are shown by `draft show`)

```text
Remove an attachment from a draft (ids are shown by `draft show`)

Usage: mermoz draft detach [OPTIONS] <ID> <ATTACHMENT>

Arguments:
  <ID>          
  <ATTACHMENT>  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz draft list

List drafts (optionally by state: draft, review, rejected)

```text
List drafts (optionally by state: draft, review, rejected)

Usage: mermoz draft list [OPTIONS]

Options:
      --json           Machine-readable JSON output
      --state <STATE>  
  -h, --help           Print help
```

### mermoz draft show

Show a draft

```text
Show a draft

Usage: mermoz draft show [OPTIONS] <ID>

Arguments:
  <ID>  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz draft edit

Replace a draft's body

```text
Replace a draft's body

Usage: mermoz draft edit [OPTIONS] <ID>

Arguments:
  <ID>  

Options:
      --body <BODY>  
      --json         Machine-readable JSON output
      --file <FILE>  
  -h, --help         Print help
```

### mermoz draft delete

Delete a draft

```text
Delete a draft

Usage: mermoz draft delete [OPTIONS] <ID>

Arguments:
  <ID>  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz drive

Google Drive files of a thread, their text, search, saving attachments. Connecting Drive and sharing files happen in the app only

```text
Google Drive files of a thread, their text, search, saving attachments. Connecting Drive and sharing files happen in the app only

Usage: mermoz drive [OPTIONS] <COMMAND>

Commands:
  files   Drive files linked in a thread, shared with its people, and attachments saved
  read    A Drive file's text (Docs as Markdown, Sheets as CSV, Slides as text)
  search  Search Drive by name and content; no words lists recently opened files
  save    Save an attachment to Drive, in Mermoz/<thread's project>
  help    Print this message or the help of the given subcommand(s)

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz drive files

Drive files linked in a thread, shared with its people, and attachments saved

```text
Drive files linked in a thread, shared with its people, and attachments saved

Usage: mermoz drive files [OPTIONS] <THREAD>

Arguments:
  <THREAD>  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz drive read

A Drive file's text (Docs as Markdown, Sheets as CSV, Slides as text)

```text
A Drive file's text (Docs as Markdown, Sheets as CSV, Slides as text)

Usage: mermoz drive read [OPTIONS] <FILE_ID>

Arguments:
  <FILE_ID>  

Options:
      --account <ACCOUNT>  Account id or email (default: the first account)
      --json               Machine-readable JSON output
  -h, --help               Print help
```

### mermoz drive search

Search Drive by name and content; no words lists recently opened files

```text
Search Drive by name and content; no words lists recently opened files

Usage: mermoz drive search [OPTIONS] [WORDS]...

Arguments:
  [WORDS]...  

Options:
      --account <ACCOUNT>  
      --json               Machine-readable JSON output
      --limit <LIMIT>      [default: 20]
  -h, --help               Print help
```

### mermoz drive save

Save an attachment to Drive, in Mermoz/&lt;thread's project&gt;

```text
Save an attachment to Drive, in Mermoz/<thread's project>

Usage: mermoz drive save [OPTIONS] <ATTACHMENT>

Arguments:
  <ATTACHMENT>  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz send

Ask to send a draft. Unless enabled by the user, it waits for approval in Mermoz (exit 3)

```text
Ask to send a draft. Unless enabled by the user, it waits for approval in Mermoz (exit 3)

Usage: mermoz send [OPTIONS] <DRAFT>

Arguments:
  <DRAFT>  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz statuses

Workflow statuses

```text
Workflow statuses

Usage: mermoz statuses [OPTIONS]

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz projects

Projects

```text
Projects

Usage: mermoz projects [OPTIONS]

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz feedback

Report friction with this CLI so it can improve

```text
Report friction with this CLI so it can improve

Usage: mermoz feedback [OPTIONS] [TEXT]...

Arguments:
  [TEXT]...  

Options:
      --command <COMMAND>  The command that caused trouble
      --json               Machine-readable JSON output
      --agent <AGENT>      Who is reporting (e.g. claude-code)
  -h, --help               Print help
```

### mermoz chat

Ask the account's AI about a thread (kept with the thread, like in the app). Without a question, shows the conversation so far. Proposals are listed, never applied

```text
Ask the account's AI about a thread (kept with the thread, like in the app). Without a question, shows the conversation so far. Proposals are listed, never applied

Usage: mermoz chat [OPTIONS] <THREAD> [QUESTION]...

Arguments:
  <THREAD>       
  [QUESTION]...  

Options:
      --json         Machine-readable JSON output
      --no-research  Don't let the AI search the web for this answer
      --clear        Forget this thread's conversation
  -h, --help         Print help
```

### mermoz feed

The Feed: open mail as cards, one thing to do each, in order (now, today, this week, waiting on others, can wait)

```text
The Feed: open mail as cards, one thing to do each, in order (now, today, this week, waiting on others, can wait)

Usage: mermoz feed [OPTIONS]

Options:
      --json               Machine-readable JSON output
      --project <PROJECT>  Only this project's cards
  -h, --help               Print help
```

### mermoz later

Put a thread off until a time said in plain words ("vendredi soir", "friday 7pm", "dans 3 jours", "15 nov"; default tomorrow morning). It leaves its column and comes back Unread then, or when new mail arrives. "none" brings it back now

```text
Put a thread off until a time said in plain words ("vendredi soir", "friday 7pm", "dans 3 jours", "15 nov"; default tomorrow morning). It leaves its column and comes back Unread then, or when new mail arrives. "none" brings it back now

Usage: mermoz later [OPTIONS] <THREAD> [WHEN]...

Arguments:
  <THREAD>   
  [WHEN]...  

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz router

Instant sorting: the provider in use, whether it has a key, and today's calls. Keys and the provider are changed in the app (Settings › Developer)

```text
Instant sorting: the provider in use, whether it has a key, and today's calls. Keys and the provider are changed in the app (Settings › Developer)

Usage: mermoz router [OPTIONS]

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz sync

Trigger an immediate sync

```text
Trigger an immediate sync

Usage: mermoz sync [OPTIONS]

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```

### mermoz mcp

Run an MCP server on stdio

```text
Run an MCP server on stdio

Usage: mermoz mcp [OPTIONS]

Options:
      --json  Machine-readable JSON output
  -h, --help  Print help
```
