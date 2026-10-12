#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["click"]
# ///
"""Render the welcome email's pictures (the landing's sky: clouds, paper balls, plane)
from marketing/emails/sky.html into site/assets/, where the email loads them from
https://mermoz.xyz/assets/ once the site is deployed.

Email clients run neither SVG filters nor JavaScript, hence baked PNGs. Uses the
installed Google Chrome in headless mode.

Usage:
  scripts/email_art.py
  scripts/email_art.py --chrome /path/to/chrome
"""

from __future__ import annotations

import logging
import subprocess
from pathlib import Path

import click

log = logging.getLogger("email_art")

REPO = Path(__file__).resolve().parent.parent
SOURCE = REPO / "marketing" / "emails" / "sky.html"
ASSETS = REPO / "site" / "assets"
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
# part → (output file, height in px); every picture is 1120 px wide (2x of the 560 px card).
PARTS = {"header": ("email-sky.png", 420), "footer": ("email-clouds.png", 200)}


def render(chrome: str, part: str, out: Path, height: int) -> None:
    subprocess.run(
        [
            chrome,
            "--headless=new",
            "--hide-scrollbars",
            "--force-device-scale-factor=1",
            f"--window-size=1120,{height}",
            # Lets the Caveat web font load before the shot is taken.
            "--virtual-time-budget=4000",
            f"--screenshot={out}",
            f"{SOURCE.as_uri()}?part={part}",
        ],
        check=True,
        capture_output=True,
    )
    log.info("wrote %s (%d KB)", out.relative_to(REPO), out.stat().st_size // 1024)


@click.command()
@click.option("--chrome", default=CHROME, help="Chrome or Chromium executable.")
def main(chrome: str) -> None:
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    for part, (name, height) in PARTS.items():
        render(chrome, part, ASSETS / name, height)


if __name__ == "__main__":
    main()
