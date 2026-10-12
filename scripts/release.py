#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["click"]
# ///
"""Package Mermoz for download: a release build, a drag-to-Applications DMG, and a
GitHub release whose `Mermoz.dmg` asset the website links to directly.

The website's download button points at
`https://github.com/ClementWalter/mermoz/releases/latest/download/Mermoz.dmg`, which
GitHub redirects to the newest release's file, so every release keeps it working.

Steps:
  1. Build the app (scripts/build_app.py) with the Google OAuth client compiled in,
     read from --google-client (an installed-app client JSON). Installed-app client
     secrets are not confidential per Google, so embedding them is expected.
  2. Make build/Mermoz.dmg: the app next to an Applications shortcut.
  3. With --publish: create the GitHub release vVERSION and upload the DMG.

Usage:
  scripts/release.py                       # build build/Mermoz.dmg only
  scripts/release.py --publish             # build and publish vVERSION
  scripts/release.py --google-client PATH  # another OAuth client file
"""

from __future__ import annotations

import json
import logging
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import click

log = logging.getLogger("release")

REPO = Path(__file__).resolve().parent.parent
APP = REPO / "build" / "Mermoz.app"
DMG = REPO / "build" / "Mermoz.dmg"
DEFAULT_CLIENT = Path.home() / ".config" / "mermoz" / "google_oauth_client.json"


def run(cmd: list[str], env: dict | None = None) -> None:
    log.info("$ %s", " ".join(cmd))
    subprocess.run(cmd, cwd=REPO, env=env, check=True)


def version() -> str:
    """The app version, as declared in build_app.py."""
    for line in (REPO / "scripts" / "build_app.py").read_text().splitlines():
        if line.startswith("VERSION = "):
            return line.split('"')[1]
    raise SystemExit("VERSION not found in scripts/build_app.py")


def build(client_file: Path) -> None:
    data = json.loads(client_file.read_text())
    client = data.get("installed", data)
    env = dict(os.environ)
    env["MERMOZ_GOOGLE_CLIENT_ID"] = client["client_id"]
    env["MERMOZ_GOOGLE_CLIENT_SECRET"] = client["client_secret"]
    # A fresh Rust build so the credentials are compiled in (option_env! at build time).
    run(["cargo", "clean", "--release", "-p", "mermoz-core"], env)
    run([sys.executable, str(REPO / "scripts" / "build_app.py")], env)


def make_dmg() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        stage = Path(tmp) / "Mermoz"
        stage.mkdir()
        shutil.copytree(APP, stage / "Mermoz.app", symlinks=True)
        (stage / "Applications").symlink_to("/Applications")
        DMG.unlink(missing_ok=True)
        run(["hdiutil", "create", "-volname", "Mermoz", "-srcfolder", str(stage), "-ov", "-format", "UDZO", str(DMG)])
    log.info("built %s (%.1f MB)", DMG, DMG.stat().st_size / 1e6)


def create_release(tag: str) -> None:
    notes = (
        "**Early beta.** Mermoz for macOS 26.\n\n"
        "- Open the DMG and drag Mermoz to Applications.\n"
        "- Not notarized yet: the first time, right-click Mermoz › Open "
        "(or System Settings › Privacy & Security › Open Anyway).\n"
        "- Gmail sign-in works for Google accounts added as testers while Google reviews the app.\n"
    )
    # Not a pre-release: GitHub's /releases/latest/download/ link (the site's button)
    # skips pre-releases.
    run(["gh", "release", "create", tag, str(DMG), "--title", f"Mermoz {tag} (beta)", "--notes", notes, "--latest"])


@click.command()
@click.option("--google-client", type=click.Path(exists=True, path_type=Path), default=DEFAULT_CLIENT)
@click.option("--publish/--no-publish", default=False, help="Create the GitHub release and upload the DMG.")
@click.option("--skip-build", is_flag=True, help="Reuse build/Mermoz.app.")
def main(google_client: Path, publish: bool, skip_build: bool) -> None:
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    if not skip_build:
        build(google_client)
    make_dmg()
    if publish:
        create_release(f"v{version()}")


if __name__ == "__main__":
    main()
