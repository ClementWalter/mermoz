---
path: /contact/
title: Contact Mermoz: email, GitHub issues and agent feedback
description: How to reach the people behind Mermoz, the email app for Mac: email, GitHub issues for bugs and ideas, and mermoz feedback for agents.
updated: 2026-10-13
---
# Contact Mermoz

Mermoz is a small, independent project in private beta. The fastest way to reach us depends on what you need.

## Email

Write to **[{{contact_email}}](mailto:{{contact_email}})** for anything personal: beta access, an invitation that didn't arrive, a privacy question or a request to delete your sign-up address. Beta testers can also simply reply to their invitation email. Every message is read by a person.

## Bugs and ideas

Open an issue on [GitHub]({{repo_url}}/issues). Mermoz is open source, so issues, discussions and fixes are public. Please say which version you run (Mermoz › About Mermoz, or `mermoz --version`) and what you expected to happen. Never paste the content of private emails or an API key into a public issue.

## Feedback from agents

If you drive Mermoz from an AI agent, the agent can report friction itself:

```sh
mermoz feedback "search ignores accents in subject" --command search --agent claude-code
```

The MCP server offers the same as the `submit_feedback` tool. Feedback is stored in your local mailbox and listed in Mermoz › Settings › Developer; share it with us by email or in an issue when you want us to see it.

## Joining the beta

Leave the Gmail or Outlook address you'll use in the form on the [homepage](/#beta). Mermoz is in private beta while Google reviews its Gmail access, so each address is added by hand; you'll get an email when you can sign in.

## Elsewhere

- Source code and releases: [{{repo_url}}]({{repo_url}})
- Developer docs: [mermoz.xyz/docs](/docs/)
- How Mermoz handles your data: [privacy](/privacy/)
