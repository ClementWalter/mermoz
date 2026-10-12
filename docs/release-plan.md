# Mermoz release plan

From today's private beta to a public download anyone can install and sign in to.
Each phase lists what blocks it, who does what, and how we know it's done.

## Where we are (12 Oct 2026)

- **App:** v0.1.0 (beta) on GitHub Releases as `Mermoz.dmg`, built by
  `scripts/release.py --publish`. Ad-hoc signed, not notarized.
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

**Release cadence:** bump `VERSION` in `scripts/build_app.py`, run
`scripts/release.py --publish`. The site's button always serves the newest DMG.

**What we watch:**
- Import of large mailboxes and the time to a first usable inbox.
- Sign-in failures (the weekly expiry must show a friendly "Sign in again").
- Feedback by email and `mermoz feedback` (agents), triaged weekly into issues.

**Exit criteria:** a week without data-loss or sync bugs on 20+ active testers;
onboarding works without help.

## Phase 2 — Trusted install on macOS (in parallel, ~1–2 weeks of setup)

Goal: double-click to open, no warning; automatic updates.

1. **Apple Developer Program** (Clément, $99/year) → Developer ID Application
   certificate.
2. **Sign with Hardened Runtime**: app, `mermozd`, `mermoz` and the bundled `ant`;
   then the DMG. (Claude: switch `build_app.py` from ad-hoc to Developer ID.)
3. **Notarize** with `notarytool` (App Store Connect API key) and **staple** the
   ticket to the DMG.
4. **Auto-update** with Sparkle: an appcast feed on mermoz.xyz, EdDSA-signed
   updates, "Check for updates" in the app.
5. **CI release**: a GitHub Actions macOS workflow on tag `v*` building, signing,
   notarizing and publishing, with secrets for the certificate, the notary key and
   the Google client.

**Done when:** a fresh Mac downloads, opens and updates Mermoz with no warning.

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

## Phase 4 — Public launch

Prerequisites: Phases 2 and 3 done; Phase 1 exit criteria met.

- Site: replace the waitlist with **Download for Mac** as the primary action; keep a
  newsletter field.
- Launch posts with the film (16:9 and 9:16 in `marketing/video/out`): X, LinkedIn,
  Hacker News (Show HN), Product Hunt, r/macapps.
- Support: a public issue tracker and a support email on the site.

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
