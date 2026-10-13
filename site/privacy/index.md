<!-- https://mermoz.xyz/privacy/ · updated 2026-10-13 -->

# Mermoz privacy

Mermoz is an email app that keeps your mailbox on your Mac. There is no Mermoz server: nothing you read or write in Mermoz passes through a computer we run. This page describes what the app and this website actually do, checked against the [source code](https://github.com/ClementWalter/mermoz).

> **Needs legal review.** This page describes how the software behaves. It is not yet a reviewed privacy policy: the data controller's identity, the legal bases (GDPR), retention periods and the Google API Limited Use statement below must be confirmed by counsel before the public launch.

## Your mail stays on your Mac

- Mermoz downloads your Gmail messages, labels and attachments into `~/Library/Application Support/Mermoz` on your Mac (a local database and a files folder). Search, reading and sorting run there, offline.
- A single background process, `mermozd`, talks to Google on your behalf. The app, the `mermoz` command line and the MCP server talk to it over a local socket on your Mac.
- Remote images in emails are not loaded unless you ask for them, since they can tell the sender you opened the mail.
- Your Google sign-ins, Drive grants and AI provider keys are kept on your Mac: in the macOS Keychain for signed releases, or in files only your user account can read for beta builds.
- The app has no analytics or tracking. Feedback sent with `mermoz feedback` is stored in your local mailbox database and shown in Settings › Developer; it is not uploaded.
- Removing an account in Mermoz deletes its messages from the local database and its Google sign-in from your Mac; deleting the folder above removes everything Mermoz stored. Your mail in Gmail is not affected.

## What Mermoz asks Google for

- **Gmail:** the `gmail.modify` scope, to read your mail, apply your labels and statuses (archive, trash, read state) and send the drafts you approve. Mermoz does not ask for permission to delete mail permanently.
- **Google Drive (optional):** only when you click "Connect Google Drive", in a separate consent: `drive.readonly` (preview documents linked in your mail, files shared with a thread's people, search) and `drive.file` (save attachments into a Mermoz folder, share a file you picked). You can untick either; mail never depends on Drive. Drive file text Mermoz reads is cached on your Mac.
- Companies can sign in through their own Google client instead of Mermoz's; that client is stored with the account on your Mac.

> **Needs legal review.** Google requires apps using Gmail data to state that their use of information received from Google APIs adheres to the Google API Services User Data Policy, including the Limited Use requirements. Confirm the exact wording before submitting for Google verification.

## AI providers: off until you turn them on

Mermoz works without any AI. Email content is sent to an AI provider only after you set one up, and only to the provider you chose:

- **Instant sorting (TypeSafe's Jev, or SubQ).** Only if you paste a TypeSafe or SubQ API key in Settings › Developer. New and open threads are then sent to that provider (`api.typesafe.ai` or `api.subq.ai`) to suggest a status, project and priority, and to rank the Feed. Each request carries the thread plus two hints built on your Mac: where you filed earlier mail from the same people, and a short description of your projects. Removing the key stops it.
- **Your account's AI (Claude by Anthropic, ChatGPT or another OpenAI-compatible service).** Only for the accounts where you allow a provider, connected with your own API key, "Sign in with Anthropic" or "Continue with ChatGPT" (billed to your own plan). It is then used when you:
  - ask "Chat about this" (the thread, its people, its status and your earlier mail with them; with web research on, the provider may search the web for the answer);
  - turn on the reading assistant, which writes the Optimized reading version and grows your contact directory from signatures;
  - open the Feed, whose cards' summaries and draft words it writes, and when Mermoz groups threads into conversations and needs a judgment on an uncertain pair.
- **Agents you run yourself.** The CLI and MCP server give your own agents (for example Claude Code or Codex) the mail they ask for. What those agents send to their AI provider is governed by that agent and its provider, not by Mermoz. Agents cannot send email without your approval in the app, unless you turn on agent sending.

Each provider handles the content under its own terms: [TypeSafe](https://typesafe.ai), [SubQ](https://subq.ai), [Anthropic](https://www.anthropic.com/legal/privacy), [OpenAI](https://openai.com/policies/privacy-policy/).

> **Needs legal review.** Whether each provider acts as your processor or as an independent controller, and the data-transfer terms that apply, depend on the account or key you use with it.

## This website and the beta sign-up

- mermoz.xyz is a static site hosted on GitHub Pages. It sets no cookies and runs no analytics. GitHub, as the host, receives your IP address and request details; fonts are loaded from Google Fonts, which also sees your IP address.
- **The beta sign-up form** sends only the email address you type to a Google Form owned by Mermoz's maintainer (Google Forms, operated by Google). The address is used to add you to the beta: it is added as a test user of Mermoz's Google Cloud project (so you can sign in during Google's review) and receives the invitation email, sent from clement@mermoz.xyz through Zoho Mail. It is not sold or shared with anyone else.
- The download button links to the latest release on GitHub, which serves the file.

> **Needs legal review.** Retention period for sign-up addresses, the legal basis (consent), and how to ask for deletion must be stated; until then, write to clement@mermoz.xyz to have your address removed.

## Contact

Questions or deletion requests: clement@mermoz.xyz. See also the [contact page](/contact/).
