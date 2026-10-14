#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["click"]
# ///
"""Send the Mermoz beta welcome email ("boarding pass") from clement@mermoz.xyz.

The message is marketing/emails/welcome.html (HTML) plus a plain-text version, sent
one recipient at a time through Zoho Mail's SMTP server with an app password. The
password comes from MERMOZ_SMTP_PASSWORD, else the Claudine vault through
claudine-secret; it is never printed.

Add recipients as Google OAuth test users first (docs/release-plan.md): the email
tells them they can sign in.

Usage:
  scripts/invite.py --to me@example.com                 # send one (e.g. a preview to yourself)
  scripts/invite.py --to a@x.com --to b@y.com           # several
  scripts/invite.py --to a@x.com --dry-run              # build the message, don't send
  scripts/invite.py --check-login                       # verify the SMTP login only
"""

from __future__ import annotations

import logging
import os
import smtplib
import ssl
import subprocess
import time
from email.message import EmailMessage
from email.utils import formataddr, make_msgid
from pathlib import Path

import click

log = logging.getLogger("invite")

REPO = Path(__file__).resolve().parent.parent
TEMPLATE = REPO / "marketing" / "emails" / "welcome.html"
SENDER = "clement@mermoz.xyz"
SENDER_NAME = "Clément from Mermoz"
SUBJECT = "Your Mermoz boarding pass ✈"
DOWNLOAD_URL = "https://github.com/ClementWalter/mermoz/releases/latest/download/Mermoz.dmg"
SECRET_REF = "op://Claudine/Zoho Mermoz SMTP/password"
# The account lives in Zoho's international data center (zoho.com).
SMTP_HOST = "smtp.zoho.com"
SMTP_PORT = 465

TEXT = f"""You're on board ✈

Thanks for joining the Mermoz beta. Your Google account is on the passenger list, so you can sign in today.

Download Mermoz for Mac: {DOWNLOAD_URL}
(requires macOS 26)

1. Open the DMG and drag Mermoz into Applications.
2. Open Mermoz from Applications. It's signed and checked by Apple, so it opens with a double-click.
3. Sign in with this Google account. Google will say the app isn't verified yet: click Continue.
4. During the beta, Google asks you to sign in again about once a week.

If your company's Google account blocks outside apps, tell me and we'll sort it out. Anything slow, confusing or broken: just reply to this email.

Clément
The mail must go through.
https://mermoz.xyz
"""


def password() -> str:
    if p := os.environ.get("MERMOZ_SMTP_PASSWORD"):
        return p
    out = subprocess.run(["claudine-secret", "read", SECRET_REF], capture_output=True, text=True)
    if out.returncode != 0 or not out.stdout.strip():
        raise SystemExit(f"no SMTP password: set MERMOZ_SMTP_PASSWORD or add {SECRET_REF}")
    return out.stdout.strip()


def message(to: str) -> EmailMessage:
    msg = EmailMessage()
    msg["From"] = formataddr((SENDER_NAME, SENDER))
    msg["To"] = to
    msg["Subject"] = SUBJECT
    msg["Message-ID"] = make_msgid(domain="mermoz.xyz")
    msg.set_content(TEXT)
    msg.add_alternative(TEMPLATE.read_text().replace("{{DOWNLOAD_URL}}", DOWNLOAD_URL), subtype="html")
    return msg


def connect() -> smtplib.SMTP_SSL:
    smtp = smtplib.SMTP_SSL(SMTP_HOST, SMTP_PORT, context=ssl.create_default_context(), timeout=30)
    smtp.login(SENDER, password())
    return smtp


@click.command()
@click.option("--to", "recipients", multiple=True, help="Recipient address (repeatable).")
@click.option("--dry-run", is_flag=True, help="Build the messages without sending.")
@click.option("--check-login", is_flag=True, help="Only check that the SMTP login works.")
def main(recipients: tuple[str, ...], dry_run: bool, check_login: bool) -> None:
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    if check_login:
        connect().quit()
        log.info("SMTP login ok for %s", SENDER)
        return
    if not recipients:
        raise SystemExit("give at least one --to")
    if dry_run:
        for to in recipients:
            log.info("would send %r to %s (%d bytes)", SUBJECT, to, len(message(to).as_bytes()))
        return
    smtp = connect()
    try:
        for to in recipients:
            smtp.send_message(message(to))
            log.info("sent to %s", to)
            # A short pause keeps us far from the provider's sending rate limits.
            time.sleep(2)
    finally:
        smtp.quit()


if __name__ == "__main__":
    main()
