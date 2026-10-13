# Mermoz release plan

From today's private beta to a public download anyone can install and sign in to.
Each phase lists what blocks it, who does what, and how we know it's done.

## Where we are (12 Oct 2026)

- **App:** v0.1.0 (beta) on GitHub Releases as `Mermoz.dmg`, built by
  `scripts/release.py --publish`. Ad-hoc signed, not notarized. Developer ID signing,
  notarization and the CI release are ready in code and wait for the certificate
  ([Developer ID signing](#developer-id-signing)).
- **Site:** https://mermoz.xyz (GitHub Pages, HTTPS) with the film, a private-beta
  banner and a sign-up form. Sign-ups land in the Google Form "Mermoz private beta"
  (account clement0walter@gmail.com) → Responses.
- **Gmail access:** Google Cloud project `mermoz-mail`, OAuth consent screen in
  **Testing**: only listed test users (max 100) can sign in, and their sign-in
  expires after 7 days.
- **Distribution blockers:** macOS shows "unverified developer" on first open;
  Gmail sign-in is limited to test users.

## Phase 1 — Private beta (now → ~4 weeks)

Goal: 30–100 real users, real mailboxes, a steady weekly release.

**Admitting a tester** (5 minutes per batch):
1. Open the form's Responses (or the linked Sheet) and pick the new addresses.
2. Google Cloud Console → `mermoz-mail` → Google Auth Platform → **Audience** →
   **Test users** → Add users → paste the Gmail addresses → Save.
3. Send the invitation email (template below).

**Invitation template**

> Subject: Your Mermoz boarding pass ✈
>
> Hi, you're in. Download Mermoz at https://mermoz.xyz (Already invited? › Download
> for Mac), drag it to Applications, then right-click › Open the first time (macOS
> asks because the beta isn't notarized yet). Sign in with this Gmail address.
> During the beta Google asks you to sign in again once a week. Reply to this email
> with anything that feels slow, confusing or broken.

**Release cadence:** bump `VERSION` in `scripts/build_app.py`, then push the tag
`v<VERSION>` (CI) or run `scripts/release.py --publish`. The site's button always
serves the newest DMG.

**What we watch:**
- Import of large mailboxes and the time to a first usable inbox.
- Sign-in failures (the weekly expiry must show a friendly "Sign in again").
- Feedback by email and `mermoz feedback` (agents), triaged weekly into issues.

**Exit criteria:** a week without data-loss or sync bugs on 20+ active testers;
onboarding works without help.

## Phase 2 — Trusted install on macOS (in parallel, ~1–2 weeks of setup)

Goal: double-click to open, no warning; automatic updates.

1. **Apple Developer Program** (Clément, $99/year): paid ✓. Developer ID Application
   certificate: see [Developer ID signing](#developer-id-signing) below.
2. **Sign with Hardened Runtime**: done in code. `scripts/build_app.py --sign
   developer-id` signs `mermozd`, `mermoz`, then the app, inside-out, with Hardened
   Runtime and a secure timestamp; `ant` keeps Anthropic's own Developer ID signature.
3. **Notarize and staple**: done in code. `scripts/release.py` notarizes the app and
   the DMG with `notarytool`, staples both and checks them as Gatekeeper does.
4. **Auto-update** with Sparkle: an appcast feed on mermoz.xyz, EdDSA-signed
   updates, "Check for updates" in the app.
5. **CI release**: done in code. `.github/workflows/release.yml` runs on a `v*` tag,
   signs, notarizes and publishes from GitHub secrets (listed below).

**Done when:** a fresh Mac downloads, opens and updates Mermoz with no warning.

## Developer ID signing

### How it works

- `scripts/build_app.py` signs ad-hoc by default (local builds, no Apple account).
  `--sign developer-id` uses the "Developer ID Application" identity from
  `MERMOZ_SIGN_IDENTITY` (name or SHA-1), or the only one in the keychain. It signs
  `Contents/Resources/mermozd` (identifier `ai.mermoz.mermozd`), `mermoz`
  (`ai.mermoz.cli`), any other nested binary or framework, then the app, never with
  `--deep`; then runs `codesign --verify --strict --deep`. `--dry-run` logs every
  command without running it.
- `ant` (Anthropic's CLI, MIT, behind "Sign in with Anthropic") already carries
  Anthropic's Developer ID signature with Hardened Runtime and a timestamp; it is kept
  as is, which notarization accepts. It must ship, or Anthropic sign-in disappears.
- `app/Mermoz.entitlements` is empty on purpose: the app is not sandboxed, and nothing
  it does (network, the daemon's socket, WKWebView, spawning `mermozd`/`ant`, the
  login Keychain) needs a Hardened Runtime exception. The file says why, key by key.
- `scripts/release.py` (default `--sign developer-id`): build → zip and notarize the
  app → staple it → DMG → sign the DMG → notarize it → staple it → verify with
  `codesign --verify --strict --deep`, `spctl -a -vvv` and `stapler validate`. A
  rejected submission prints Apple's notarization log. `--dry-run` lists every command.
  Notary credentials: `MERMOZ_NOTARY_KEY_ID` + `MERMOZ_NOTARY_ISSUER` +
  `MERMOZ_NOTARY_KEY_PATH` (API key), else the keychain profile `mermoz-notary`.

### Secrets move into the Keychain

Ad-hoc builds (every beta so far) keep Google sign-ins, Drive grants, provider API
keys and ChatGPT sessions in owner-only files under
`~/Library/Application Support/Mermoz/tokens`, because an ad-hoc identity changes on
every build and the Keychain would ask for the password each time. Developer ID builds
are compiled with `MERMOZ_USE_KEYCHAIN` and use the login Keychain: an item's access
list trusts the daemon's designated requirement (identifier `ai.mermoz.mermozd` + Team
ID), which stays the same across updates and certificate renewals, so there is no
prompt.

Carry-over for testers: on the first read after the update, the signed daemon finds
the secret in the file store, writes it to the Keychain and deletes the file. Nobody
signs in again. If the Keychain refuses (locked), the file stays and keeps working.
Mail, settings and everything else in `~/Library/Application Support/Mermoz` do not
depend on the signature.

No keychain access group is used: those need the data protection keychain and a
provisioning profile. Never change the daemon's identifier or the Team ID, or every
saved secret will ask for the password once.

Local development once the signed app is installed: an ad-hoc build reading a secret
the signed daemon saved triggers a Keychain password prompt, since its identity
differs. Build locally with `--sign developer-id` too, or use a separate
`MERMOZ_HOME`.

### What Clément does, once

1. **Developer ID Application certificate** (needs the Account Holder role, which the
   person who paid has).
   1. Xcode › Settings… › Accounts. If the developer account isn't listed: + › Apple
      Account › sign in.
   2. Select the account, then the team on the right › **Manage Certificates…**
   3. Bottom-left **+** › **Developer ID Application**. Xcode creates the key and the
      certificate in the login keychain.
   4. Check in Terminal: `security find-identity -v -p codesigning` lists
      `"Developer ID Application: <Name> (<TEAMID>)"`.

   Without Xcode: Keychain Access › Certificate Assistant › Request a Certificate From
   a Certificate Authority (your email, "Saved to disk"); then developer.apple.com ›
   Account › Certificates, IDs & Profiles › Certificates › + › Developer ID Application
   › G2 Sub-CA › upload the request › Download › double-click the `.cer`.
2. **Team ID**: developer.apple.com › Account › Membership details › Team ID (10
   characters). It is also the part in parentheses of the certificate name. The
   scripts read it from the certificate; it is not stored in the repo.
3. **Notarization credentials**, either or both:
   - **API key** (required for CI): App Store Connect › Users and Access ›
     Integrations › App Store Connect API › Team Keys (the first time: Request Access,
     then accept) › **+** › name `mermoz-notary`, Access **Developer** › Generate.
     Download `AuthKey_<KEYID>.p8` (only possible once) and note the **Key ID** and
     the **Issuer ID** shown above the table.
   - **Keychain profile** (local releases), made once:
     `xcrun notarytool store-credentials mermoz-notary --key ~/Downloads/AuthKey_<KEYID>.p8 --key-id <KEYID> --issuer <ISSUER_ID>`.
     Alternative without an API key: `xcrun notarytool store-credentials mermoz-notary
     --apple-id <apple-account-email> --team-id <TEAMID>`, with an app-specific
     password from account.apple.com › Sign-In and Security › App-Specific Passwords.
4. **Export the certificate for CI**: Keychain Access › login › My Certificates ›
   "Developer ID Application: …" (unfold it: the private key must be under it) ›
   right-click › Export… › format Personal Information Exchange (.p12) › save as
   `DeveloperID.p12` › set a strong password.
5. **GitHub secrets** (repo Settings › Secrets and variables › Actions, or `gh`):

   | Secret | Value |
   | --- | --- |
   | `MACOS_CERTIFICATE_P12_BASE64` | `base64 -i DeveloperID.p12` |
   | `MACOS_CERTIFICATE_PASSWORD` | the .p12 export password |
   | `MERMOZ_NOTARY_KEY_ID` | API Key ID |
   | `MERMOZ_NOTARY_ISSUER` | API Issuer ID |
   | `MERMOZ_NOTARY_KEY_P8` | contents of `AuthKey_<KEYID>.p8` |
   | `GOOGLE_OAUTH_CLIENT_JSON` | contents of `~/.config/mermoz/google_oauth_client.json` |

   ```
   base64 -i DeveloperID.p12 | gh secret set MACOS_CERTIFICATE_P12_BASE64 -R ClementWalter/mermoz
   gh secret set MACOS_CERTIFICATE_PASSWORD -R ClementWalter/mermoz
   gh secret set MERMOZ_NOTARY_KEY_ID -R ClementWalter/mermoz --body <KEYID>
   gh secret set MERMOZ_NOTARY_ISSUER -R ClementWalter/mermoz --body <ISSUER_ID>
   gh secret set MERMOZ_NOTARY_KEY_P8 -R ClementWalter/mermoz < ~/Downloads/AuthKey_<KEYID>.p8
   gh secret set GOOGLE_OAUTH_CLIENT_JSON -R ClementWalter/mermoz < ~/.config/mermoz/google_oauth_client.json
   ```

   Then delete `DeveloperID.p12`, and keep the `.p8` in a password manager. Neither
   file ever goes in the repo (it is public).

### First signed release

1. Bump `VERSION` in `scripts/build_app.py` (v0.1.0 is already published), commit.
2. Rehearse: `uv run scripts/release.py --dry-run` lists every command, with the
   identity it found.
3. Release, either way:
   - **Locally**: `uv run scripts/release.py --publish`. Notarization takes a few
     minutes per submission (two: the app, then the DMG).
   - **From CI**: `git tag v<VERSION> && git push origin v<VERSION>`. A trial without
     publishing: Actions › release › Run workflow (publish unchecked), then download
     the DMG artifact.
4. Check on a Mac that never ran Mermoz: download from mermoz.xyz, open the DMG,
   double-click Mermoz: no warning. `spctl -a -vvv -t install Mermoz.dmg` says
   `source=Notarized Developer ID`.
5. Remove the right-click sentence from the invitation template above.

Tests: `uv run --with pytest --with click pytest scripts/tests` (signing order,
identity choice, notary flow). `MERMOZ_SIGNING_E2E=1` adds a real `codesign` run with
a throwaway self-signed identity in a temporary keychain (CI runs it).

## Phase 3 — Google verification (start now, the longest pole: weeks)

Goal: anyone with a Gmail account can sign in, without the 7-day expiry.

1. **Domain:** verify mermoz.xyz in Google Search Console with the same Google
   account as the Cloud project.
2. **Public pages on mermoz.xyz:** Privacy policy and Terms (what is stored
   locally, what goes to AI providers and only when the user enables it, how to
   delete data, contact), linked from the homepage.
3. **Consent screen:** app name, logo, support email, homepage, privacy and terms
   links, authorized domain mermoz.xyz.
4. **Scopes:** request only what the app uses and justify each one. Gmail read and
   modify scopes are **restricted** scopes.
   Google Drive adds two more, asked only when the user clicks "Connect Google
   Drive" (a separate consent; mail access never depends on it):
   - `drive.readonly` — **restricted**. Justification: previews and AI context for
     documents linked in mail, files shared with a thread's people, search when
     attaching. It joins the same CASA assessment the Gmail scopes already need.
   - `drive.file` — non-sensitive. Saving attachments into Mermoz folders, and
     sharing a file the user picked in Google's file picker.
   Fallback if Google refuses `drive.readonly`: ship `drive.file` alone. Saving and
   attach-by-picker keep working; linked-document previews need one "Allow" click per
   document (Google's picker filtered to it), and the "shared with these people"
   list disappears. The app already handles a `drive.file`-only grant.
   **Before the first Drive sign-in** (Clément, Cloud console › `mermoz-mail`):
   APIs & Services › Library › enable **Google Drive API** and **Google Picker API**;
   Google Auth Platform › Data Access › Add or remove scopes › add
   `…/auth/drive.readonly` and `…/auth/drive.file` › Save.
5. **Demo video:** the OAuth consent flow and every feature that uses each scope.
6. **Security assessment (CASA):** restricted Gmail scopes normally require an
   annual third-party assessment by a Google-approved lab (paid). To confirm with
   Google during review: Mermoz keeps mail on the device, but optional AI features
   send message content to AI providers, which likely keeps the assessment in scope.
7. Submit, answer reviewer questions, iterate.

**Done when:** the consent screen is "In production" and verified.

### Company accounts (Google Workspace)

Workspace admins commonly block unverified external apps from asking for
restricted Gmail scopes, so a work account may be refused at sign-in
(`admin_policy_enforced`, "has not completed the Google verification process",
`access_denied` from App access control). Mermoz explains the refusal in plain words
and shows its client ID with a copy button. Two ways in:

- **Admin trusts Mermoz.** In the Google Admin console › Security › API controls ›
  App access control › Manage third-party app access › Configure new app, the admin
  searches by Mermoz's client ID and sets it to **Trusted**. Works today, even while
  Mermoz is unverified.
- **Own sign-in client.** Someone in the company creates an OAuth client of type
  **Desktop app** in a Google Cloud project of the company's organization, with an
  **Internal** consent screen and the Gmail API enabled (plus Drive API and Picker
  API for Drive). Internal apps need no Google verification for accounts of that
  organization. The user adds the account in Settings › Developer › "Your own Google
  sign-in client" (pick the downloaded `client_secret_….json` or paste ID and
  secret), or `mermoz add-account --client-json PATH`. The client is stored with the
  account's token (`custom_client: true`), used for its refreshes and for its Drive
  consent, and never flagged "Reconnect". An `org_internal` refusal means the
  account is outside that client's organization.

For the public beta, the first path only scales once Mermoz is verified: restricted
Gmail scopes need Google's verification **and** the CASA assessment above. Until
then, company users rely on their admin or their own client.

## Phase 4 — Public launch

Prerequisites: Phases 2 and 3 done; Phase 1 exit criteria met.

- Site: replace the waitlist with **Download for Mac** as the primary action; keep a
  newsletter field.
- Launch posts with the film (16:9 and 9:16 in `marketing/video/out`): X, LinkedIn,
  Hacker News (Show HN), Product Hunt, r/macapps.
- Support: a public issue tracker and a support email on the site.

## Markdown for agents: Cloudflare in front (decision for Clément)

mermoz.xyz is plain GitHub Pages, DNS at GoDaddy (`ns13/ns14.domaincontrol.com`).
Everything agents need is already published as files: `/llms.txt`, `/llms-full.txt`,
`/sitemap.xml`, `/server.json`, `/.well-known/mcp.json` and a Markdown twin of every
page (`/index.md`, `/docs/mcp/index.md`, `/404.md`, …), all built by
`scripts/site_build.py`. What GitHub Pages cannot do is **content negotiation**:
answer `Accept: text/markdown` on `/` with Markdown and `Vary: Accept`, or give a
Markdown body to a 404. That needs an edge in front of Pages. The Worker is ready in
`site/edge/` (`worker.js`, `wrangler.toml`, tests); it proxies everything to Pages and
only swaps in the Markdown twin when the client prefers Markdown.

It is optional: it costs a DNS move and adds Cloudflare to the request path (and to the
privacy page's list of hosts). Steps, if wanted:

1. **Cloudflare account** (free plan) › Add a domain › `mermoz.xyz` › Free. Cloudflare
   imports the current records; check it lists the four GitHub Pages `A` records
   (`185.199.108–111.153`), the `www` CNAME if any, and **the Zoho `MX`, SPF/DKIM `TXT`
   records** (mail from clement@mermoz.xyz must keep working).
2. Set the `A`/`CNAME` records for `mermoz.xyz` to **Proxied** (orange cloud); leave MX
   and TXT as DNS only. SSL/TLS mode: **Full**.
3. **GoDaddy** › My Products › mermoz.xyz › DNS › Nameservers › Change › "I'll use my
   own nameservers" › paste the two Cloudflare nameservers. Propagation: minutes to
   24 h; Cloudflare emails when the zone is active.
4. GitHub › repo Settings › Pages: the custom domain stays `mermoz.xyz`; keep "Enforce
   HTTPS" (GitHub's certificate renews through the proxy with SSL mode Full).
5. Deploy the Worker (route `mermoz.xyz/*` is in `wrangler.toml`; no account id is
   committed, `wrangler login` picks the account):
   ```
   cd site/edge && npx wrangler login && npx wrangler deploy
   ```
6. Check (the audit's own commands):
   ```
   curl -sS -L -i -H 'Accept: text/markdown' https://mermoz.xyz/          # 200 text/markdown, Vary: Accept
   curl -sS -L -i -H 'Accept: text/html' https://mermoz.xyz/              # 200 text/html, Vary: Accept
   curl -sS -L -i -H 'Accept: text/markdown' https://mermoz.xyz/nope      # 404 text/markdown
   ```

Local test before deploying: `python3 -m http.server 8765 -d site`, then
`cd site/edge && npx wrangler dev --var ORIGIN:http://localhost:8765` and the same curls
on `http://127.0.0.1:8787`. Unit tests: `node --test site/edge/worker.test.mjs`.

## Legal and trust (before Phase 3 submission)

- Privacy policy and terms (EU/GDPR: data stays on the device; AI providers act on
  the user's request with the user's own account or key).
- AI disclosure in the app's first run: which features send content, to whom, and
  that they are off until turned on.
- Open-source license file and third-party notices (the bundled `ant` CLI is MIT).

## Owners

| Item | Clément | Claude |
| --- | --- | --- |
| Add testers, send invitations | ✓ | invitation automation if wanted |
| Apple Developer account, certificates | ✓ | signing, notarization, Sparkle, CI |
| Search Console, consent screen, video | ✓ | privacy/terms pages, scope audit, video script |
| CASA assessment | ✓ (vendor, payment) | evidence, fixes |
| Weekly releases | approve | build, test, publish |
