#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["click"]
# ///
"""Package Mermoz for download: a Developer ID signed, notarized and stapled build, a
drag-to-Applications DMG, and a GitHub release whose `Mermoz.dmg` asset the website
links to directly.

The website's download button points at
`https://github.com/ClementWalter/mermoz/releases/latest/download/Mermoz.dmg`, which
GitHub redirects to the newest release's file, so every release keeps it working.

Steps (--sign developer-id, the default):
  1. Build the app (scripts/build_app.py --sign developer-id) with the Google OAuth
     client compiled in, read from --google-client (an installed-app client JSON).
     Installed-app client secrets are not confidential per Google, so embedding them
     is expected.
  2. Notarize the app (zipped) and staple its ticket, so it opens offline once copied.
  3. Make build/Mermoz.dmg (the app next to an Applications shortcut), sign it,
     notarize it and staple it.
  4. Verify: codesign --verify --strict --deep, spctl -a -vvv, stapler validate.
  5. With --publish: create the GitHub release vVERSION and upload the DMG.

Notary credentials, first match wins:
  - App Store Connect API key: MERMOZ_NOTARY_KEY_ID, MERMOZ_NOTARY_ISSUER and
    MERMOZ_NOTARY_KEY_PATH (the AuthKey_XXXX.p8 file). Used by CI.
  - A notarytool keychain profile (default `mermoz-notary`), created once with
    `xcrun notarytool store-credentials mermoz-notary`.

Usage:
  scripts/release.py                         # signed, notarized build/Mermoz.dmg
  scripts/release.py --publish               # ... and publish vVERSION
  scripts/release.py --dry-run               # log every command, run none
  scripts/release.py --sign adhoc            # local, un-notarized DMG (no Apple account)
  scripts/release.py --google-client PATH    # another OAuth client file
"""

from __future__ import annotations

import json
import logging
import os
import shutil
import sys
import tempfile
from pathlib import Path

import click

sys.path.insert(0, str(Path(__file__).resolve().parent))
import signing  # sibling module, importable from any working directory

log = logging.getLogger("release")

REPO = Path(__file__).resolve().parent.parent
APP = REPO / "build" / "Mermoz.app"
DMG = REPO / "build" / "Mermoz.dmg"
NOTARY_ZIP = REPO / "build" / "Mermoz-notarize.zip"
DEFAULT_CLIENT = Path.home() / ".config" / "mermoz" / "google_oauth_client.json"
DEFAULT_PROFILE = "mermoz-notary"
NOTARY_ENV = ("MERMOZ_NOTARY_KEY_ID", "MERMOZ_NOTARY_ISSUER", "MERMOZ_NOTARY_KEY_PATH")


def version() -> str:
    """The app version, as declared in build_app.py."""
    for line in (REPO / "scripts" / "build_app.py").read_text().splitlines():
        if line.startswith("VERSION = "):
            return line.split('"')[1]
    raise SystemExit("VERSION not found in scripts/build_app.py")


def build(run: signing.Runner, client_file: Path, mode: str, keychain: Path | None) -> None:
    data = json.loads(client_file.read_text())
    client = data.get("installed", data)
    env = dict(os.environ)
    env["MERMOZ_GOOGLE_CLIENT_ID"] = client["client_id"]
    env["MERMOZ_GOOGLE_CLIENT_SECRET"] = client["client_secret"]
    # A fresh Rust build so the credentials are compiled in (option_env! at build time).
    run(["cargo", "clean", "--release", "-p", "mermoz-core"], cwd=REPO, env=env)
    cmd = [sys.executable, str(REPO / "scripts" / "build_app.py"), "--sign", mode]
    if keychain:
        cmd += ["--keychain", str(keychain)]
    if run.dry_run:
        # The build script's own dry run lists its commands.
        signing.Runner()([*cmd, "--dry-run"], cwd=REPO, env=env, inspect=True, capture=False)
        return
    run(cmd, cwd=REPO, env=env, capture=False)


# ---- notarization ----


def notary_auth(env: dict, profile: str) -> list[str]:
    """notarytool credentials: the API key from the environment if set, else the profile."""
    values = {k: (env.get(k) or "").strip() for k in NOTARY_ENV}
    present = [k for k, v in values.items() if v]
    if present and len(present) < len(NOTARY_ENV):
        missing = ", ".join(k for k in NOTARY_ENV if k not in present)
        raise signing.SigningError(f"Incomplete notary API key: also set {missing}")
    if present:
        return [
            "--key",
            values["MERMOZ_NOTARY_KEY_PATH"],
            "--key-id",
            values["MERMOZ_NOTARY_KEY_ID"],
            "--issuer",
            values["MERMOZ_NOTARY_ISSUER"],
        ]
    return ["--keychain-profile", profile]


def parse_submission(stdout: str) -> tuple[str | None, str]:
    """(submission id, status) from `notarytool submit --output-format json`."""
    try:
        data = json.loads(stdout)
    except json.JSONDecodeError:
        return None, "Unknown"
    return data.get("id"), data.get("status", "Unknown")


def notarize(run: signing.Runner, path: Path, auth: list[str]) -> None:
    """Submits to Apple's notary service and waits; on rejection logs Apple's report."""
    proc = run(
        ["xcrun", "notarytool", "submit", str(path), *auth, "--wait", "--output-format", "json"],
        check=False,
    )
    if run.dry_run:
        return
    sub_id, status = parse_submission(proc.stdout)
    log.info("notarization of %s: %s (id %s)", path.name, status, sub_id)
    if status == "Accepted":
        return
    if sub_id:
        report = run(["xcrun", "notarytool", "log", sub_id, *auth], check=False)
        log.error("notarization log for %s:\n%s%s", path.name, report.stdout, report.stderr)
    else:
        log.error("notarytool output:\n%s%s", proc.stdout, proc.stderr)
    raise signing.SigningError(f"notarization of {path.name} ended with status {status}")


def staple(run: signing.Runner, path: Path) -> None:
    run(["xcrun", "stapler", "staple", str(path)])
    run(["xcrun", "stapler", "validate", str(path)])


def notarize_app(run: signing.Runner, auth: list[str]) -> None:
    """notarytool takes a zip, not a bare .app; the ticket is then stapled onto the app."""
    NOTARY_ZIP.unlink(missing_ok=True)
    run(["ditto", "-c", "-k", "--keepParent", str(APP), str(NOTARY_ZIP)])
    notarize(run, NOTARY_ZIP, auth)
    staple(run, APP)
    if not run.dry_run:
        NOTARY_ZIP.unlink(missing_ok=True)


def verify(run: signing.Runner) -> None:
    """What Gatekeeper checks on a tester's Mac, for the app and the DMG."""
    signing.verify_signature(run, APP)
    run(["spctl", "-a", "-vvv", "-t", "exec", str(APP)])
    run(["xcrun", "stapler", "validate", str(APP)])
    run(["codesign", "--verify", "--strict", "--verbose=2", str(DMG)])
    run(["spctl", "-a", "-vvv", "-t", "install", str(DMG)])
    run(["xcrun", "stapler", "validate", str(DMG)])


# ---- packaging ----


def make_dmg(run: signing.Runner) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        stage = Path(tmp) / "Mermoz"
        if not run.dry_run:
            stage.mkdir()
            shutil.copytree(APP, stage / "Mermoz.app", symlinks=True)
            (stage / "Applications").symlink_to("/Applications")
            DMG.unlink(missing_ok=True)
        run(["hdiutil", "create", "-volname", "Mermoz", "-srcfolder", str(stage), "-ov", "-format", "UDZO", str(DMG)])
    if not run.dry_run:
        log.info("built %s (%.1f MB)", DMG, DMG.stat().st_size / 1e6)


def release_notes(mode: str) -> str:
    first_open = (
        "- Signed and notarized by Apple: double-click to open.\n"
        if mode == signing.DEVELOPER_ID
        else "- Not notarized: the first time, right-click Mermoz › Open "
        "(or System Settings › Privacy & Security › Open Anyway).\n"
    )
    return (
        "**Early beta.** Mermoz for macOS 26.\n\n"
        "- Open the DMG and drag Mermoz to Applications.\n"
        f"{first_open}"
        "- Gmail sign-in works for Google accounts added as testers while Google reviews the app.\n"
    )


def create_release(run: signing.Runner, tag: str, mode: str) -> None:
    # Not a pre-release: GitHub's /releases/latest/download/ link (the site's button)
    # skips pre-releases.
    run(
        [
            "gh",
            "release",
            "create",
            tag,
            str(DMG),
            "--title",
            f"Mermoz {tag} (beta)",
            "--notes",
            release_notes(mode),
            "--latest",
        ],
        cwd=REPO,
    )


@click.command()
@click.option("--google-client", type=click.Path(path_type=Path), default=DEFAULT_CLIENT, show_default=True)
@click.option("--publish/--no-publish", default=False, help="Create the GitHub release and upload the DMG.")
@click.option("--skip-build", is_flag=True, help="Reuse build/Mermoz.app (already signed).")
@click.option(
    "--sign",
    "mode",
    type=click.Choice(signing.MODES),
    default=signing.DEVELOPER_ID,
    show_default=True,
    help="developer-id signs, notarizes and staples; adhoc makes a local, un-notarized DMG.",
)
@click.option(
    "--keychain",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    help="Look up the signing identity in this keychain only (CI).",
)
@click.option("--notary-profile", default=DEFAULT_PROFILE, show_default=True, help="notarytool keychain profile.")
@click.option("--dry-run", is_flag=True, help="Log every command without running it.")
def main(
    google_client: Path,
    publish: bool,
    skip_build: bool,
    mode: str,
    keychain: Path | None,
    notary_profile: str,
    dry_run: bool,
) -> None:
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    run = signing.Runner(dry_run=dry_run)
    try:
        if publish and mode != signing.DEVELOPER_ID:
            raise click.ClickException("Releases are Developer ID signed: drop --sign adhoc to publish.")
        # Credentials first, so a missing one fails before a long build.
        auth, identity = [], ""
        if mode == signing.DEVELOPER_ID:
            auth = notary_auth(dict(os.environ), notary_profile)
            identity = signing.signing_identity(run, os.environ.get(signing.IDENTITY_ENV), keychain)
        if not skip_build:
            if not google_client.exists():
                raise click.ClickException(f"Google OAuth client file not found: {google_client}")
            build(run, google_client, mode, keychain)
        if mode == signing.DEVELOPER_ID:
            notarize_app(run, auth)
            make_dmg(run)
            signing.sign_file(run, DMG, identity, keychain)
            notarize(run, DMG, auth)
            staple(run, DMG)
            verify(run)
        else:
            make_dmg(run)
        if publish:
            create_release(run, f"v{version()}", mode)
    except signing.SigningError as e:
        raise click.ClickException(str(e)) from e


if __name__ == "__main__":
    main()
