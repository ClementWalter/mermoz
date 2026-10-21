# Outlook accounts

Mermoz syncs Outlook mailboxes (outlook.com, Hotmail, Live and Microsoft 365 work or
school accounts) through Microsoft Graph, next to Gmail. This page covers the design
and how to register the Microsoft sign-in app that Outlook sign-in needs.

## Registering the Microsoft sign-in app

Outlook sign-in needs one Microsoft Entra app registration, made once by whoever ships
Mermoz. It is a public client: there is no secret, only an Application (client) ID,
which is not confidential and is compiled into release builds.

1. Sign in to the Microsoft Entra admin center, <https://entra.microsoft.com>, with an
   account that has a Microsoft Entra directory (any Azure account has one; a personal
   Microsoft account gets one by creating a free Azure account). The Azure portal,
   <https://portal.azure.com>, › **App registrations** works the same.
2. Go to **Entra ID › App registrations › New registration** (older menus:
   **Identity › Applications › App registrations**).
3. **Name**: `Mermoz` (users see it on Microsoft's consent page).
4. **Supported account types**: **Accounts in any organizational directory (Any
   Microsoft Entra ID tenant - Multitenant) and personal Microsoft accounts (e.g.
   Skype, Xbox)**. This is the `common` authority Mermoz signs in through, so both
   personal and work or school accounts work.
5. **Redirect URI**: platform **Public client/native (mobile & desktop)**, value
   `http://localhost` (exactly that: no port, no path). Click **Register**.
   - Mermoz listens on a random free port and sends `http://localhost:<port>`;
     Microsoft ignores the port when matching `localhost` redirects, so this one entry
     covers every sign-in.
   - Added later instead: **Authentication › Add a platform › Mobile and desktop
     applications › Custom redirect URIs** › `http://localhost`.
   - Don't use `http://127.0.0.1`: the portal refuses `http` with an IP address.
6. **Authentication**: leave **Allow public client flows** at **No**; the sign-in is
   the authorization code flow with PKCE, which a "Mobile and desktop" redirect
   already allows without a secret. (If a sign-in ever fails with `AADSTS7000218`,
   "client_assertion or client_secret required", set it to **Yes**.)
7. **API permissions › Add a permission › Microsoft Graph › Delegated permissions**,
   add:
   - `offline_access` (stay signed in: refresh tokens)
   - `User.Read` (already there: the account's address)
   - `Mail.ReadWrite` (read mail, move it, read state, categories)
   - `Mail.Send` (send mail)
   - `MailboxSettings.Read` (the mailbox's categories)

   None needs an administrator: each user consents for themselves. **Grant admin
   consent** is only needed for organizations that turned user consent off.
8. **Certificates & secrets**: add nothing. Mermoz never sends a secret.
9. Optional, recommended before inviting work accounts: **Branding & properties ›
   Publisher verification**. Many organizations only let their users consent to
   apps from a verified publisher (it needs a Microsoft AI Cloud Partner Program ID).
10. **Overview**: copy the **Application (client) ID** (a GUID).

### Where the client ID goes

- **Release builds**: `scripts/release.py --microsoft-client <ID>`, or set it once as
  the GitHub secret the release workflow reads:
  `gh secret set MICROSOFT_CLIENT_ID --body <ID>`. A build without an ID offers no
  Outlook sign-in: "Add Outlook account" is greyed with "Outlook sign-in isn't set up
  in this version of Mermoz yet."
- **Local builds**: `MERMOZ_MICROSOFT_CLIENT_ID=<ID> scripts/build_app.py` compiles it
  in, or, without rebuilding, either `MERMOZ_MICROSOFT_CLIENT_ID=<ID>` in the
  daemon's environment or a file `microsoft_client.json` in Mermoz's data folder
  (`~/Library/Application Support/Mermoz/`, or `MERMOZ_HOME`):
  `{"client_id": "<ID>"}`.

### When an organization blocks the sign-in

Microsoft then says the app needs approval. An administrator of that organization
approves Mermoz for everyone once, at
`https://login.microsoftonline.com/<their-tenant>/adminconsent?client_id=<ID>`;
Mermoz tells the user to ask their IT team.

## Design

### Sign-in and tokens

- Authorization code + PKCE on `https://login.microsoftonline.com/common/oauth2/v2.0`,
  the user picking the account (`prompt=select_account`), with the scopes above.
  The loopback listener answers on `127.0.0.1` and `::1`, since browsers may resolve
  `localhost` to either.
- Microsoft replaces the refresh token on every refresh: each new one is saved where
  Gmail's are (the Keychain in Developer ID builds, owner-only files otherwise).
- The account's address is Graph's `mail`, else its sign-in name (personal accounts
  often have no `mail`).

### Sync

- **Folders imported**: Inbox, Sent Items, Archive and every folder the user made
  (wherever it sits, Inbox subfolders included). Deleted Items, Junk Email, Drafts,
  Outbox and Sync Issues are not imported, nor folders inside them.
- **Import**, newest first and resumable: the mailbox listing (`/me/messages` by
  received date, the page link saved as the cursor), then one delta walk per
  imported folder, which catches what changed meanwhile and leaves one delta link per
  folder, saved as it goes. While it runs, each poll also checks the newest Inbox
  mail, so new mail shows during a long import.
- **Incremental**: every poll (20 s, or at once after a local edit) follows each
  folder's delta link. A message that left a folder and showed up in no other
  imported folder is looked up once: in Deleted Items it is deleted, gone for good it
  is deleted, in Junk it is archived. An expired delta link walks that folder again.
  Changes the store already shows (echoes of Mermoz's own write-backs) fetch nothing.
- **Message source**: `GET /me/messages/{id}/$value` (MIME), parsed and stored exactly
  like Gmail's. Requests use immutable ids (`Prefer: IdType="ImmutableId"`), so a
  message keeps its id when it moves.
- **Threads** are Outlook conversations (`conversationId`).
- **Throttling**: at most four requests in flight per mailbox (Exchange's limit);
  `429`/`503` wait as long as `Retry-After` says (capped at 5 minutes), other transient
  failures back off. `$batch` is not used: Graph counts each request inside a batch
  against the same per-mailbox limits, so it would not import faster.

### Folders and categories as labels

| Outlook | Label in Mermoz |
| --- | --- |
| Inbox | `INBOX` |
| Sent Items | `SENT` |
| Deleted Items | `TRASH` (status Deleted) |
| Junk Email | `SPAM` |
| Drafts | `DRAFT` |
| Archive | none (archived) |
| A folder the user made | user label `folder:<id>`, named by its path ("Clients/Acme") |
| A category | user label `cat:<name>` |
| `isRead: false` | `UNREAD` |
| Flagged | `STARRED` |

User folders and categories are both projects. Mail filed in folders shows in the
projects of those names; categories work like Gmail labels (any number per message).

### Write-back

| Change in Mermoz | In Outlook |
| --- | --- |
| Read / unread | `isRead` |
| Archive (or Done) | moved to Archive; to the project's folder when the thread is in a project that is a folder |
| Back to an open status | moved back to the Inbox (received mail; sent mail stays in Sent Items) |
| Delete | moved to Deleted Items (Outlook empties it on its own schedule) |
| Undo a delete | moved back to the Inbox, or Archive |
| A status (To do, Waiting…) | category `Mermoz/<Status>` |
| Add a project | a category of that name; on archived mail, a move into the folder of that name when there is one |
| Remove a project | the category comes off; mail in the folder of that name moves to Archive |
| Rename a project | the folder is renamed, or the category is replaced on every message |
| Delete a project | the category comes off every message; a folder's mail moves to Archive, then the emptied folder is deleted |

Nothing deletes a message for good. Failed write-backs stay queued, as for Gmail.

### Sending

The composed MIME message (the same bytes as for Gmail, `In-Reply-To` and
`References` included) goes to `POST /me/sendMail` as base64 with `Content-Type:
text/plain`. Outlook files the copy in Sent Items; it comes back with the next delta
and joins the conversation it answers.

### Not on Outlook accounts

- Google Drive (linked document cards, Save to Drive, Add from Drive): hidden in the
  app; Drive calls refuse Outlook accounts. OneDrive is not supported.
- "Search all of Gmail" (Gmail's own search): Outlook accounts add no results.
- Outlook sign-in on iPhone: not yet.

### Verified so far

Everything above runs against an in-process stub of Microsoft Graph and Microsoft's
token endpoint (`mermoz_core::outlook::stub`): unit tests for the client, labels,
import, delta, echoes, write-back, send, token rotation and throttling, and an
end-to-end test of the daemon (`crates/mermozd/tests/outlook_e2e.rs`). Nothing has
yet run against Microsoft's real service: that starts once the app registration
above exists.
