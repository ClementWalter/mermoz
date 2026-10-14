#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["click"]
# ///
"""Build Mermoz.app: Rust daemon + CLI, SwiftUI app, bundled and signed.

Steps:
  1. `cargo build --release` at the repo root (produces `mermozd` and `mermoz`).
  2. `swift build -c release` in `app/` (produces the `Mermoz` executable).
  3. Assemble `build/Mermoz.app` with the daemon, the CLI and `ant` in Contents/Resources.
  4. Sign it inside-out (scripts/signing.py), then verify the signature.

Signing modes (--sign):
  adhoc         default; local development, no Apple account needed. Secrets are kept
                in owner-only files, since an ad-hoc identity changes on every build.
  developer-id  releases: Developer ID Application identity, Hardened Runtime, secure
                timestamp, app/Mermoz.entitlements. The daemon is compiled with
                MERMOZ_USE_KEYCHAIN so secrets live in the Keychain. The identity is
                MERMOZ_SIGN_IDENTITY (name or SHA-1) or the only Developer ID identity
                in the keychain search list (or in --keychain).

Usage:
  scripts/build_app.py                          # ad-hoc build
  scripts/build_app.py --open                   # build, then launch the app
  scripts/build_app.py --skip-rust              # reuse existing target/release binaries
  scripts/build_app.py --sign developer-id      # release signing
  scripts/build_app.py --sign developer-id --dry-run   # log every command, run none
  scripts/build_app.py --check-ant              # print the verified Anthropic ant, build nothing

Anthropic's `ant` is bundled only once verified: a Mach-O whose code signature is a valid
Developer ID from Anthropic's team. The Homebrew cask's own copy is preferred over PATH,
where Apache Ant (also named `ant`) may shadow it. A release build fails without it.
"""

from __future__ import annotations

import logging
import os
import plistlib
import shutil
import subprocess
import sys
from pathlib import Path

import click

sys.path.insert(0, str(Path(__file__).resolve().parent))
import signing  # sibling module, importable from any working directory

log = logging.getLogger("build_app")

# Resolved from this file so the script works from any checkout location.
REPO = Path(__file__).resolve().parent.parent
APP_PKG = REPO / "app"
OUT = REPO / "build" / "Mermoz.app"
ENTITLEMENTS = APP_PKG / "Mermoz.entitlements"
RUST_BINS = ("mermozd", "mermoz")
# Anthropic PBC's Apple team: the leaf certificate OU of `ant`'s Developer ID signature.
ANTHROPIC_TEAM = "Q6L2SF6YDW"
ANTHROPIC_REQUIREMENT = f'anchor apple generic and certificate leaf[subject.OU] = "{ANTHROPIC_TEAM}"'
VERSION = "0.1.1"


def build_rust(run: signing.Runner, mode: str) -> None:
    if not (REPO / "Cargo.toml").exists():
        raise click.ClickException("No Cargo.toml at the repo root: the Rust daemon/CLI crates are missing.")
    env = dict(os.environ)
    if mode == signing.DEVELOPER_ID:
        # A stable signature lets the Keychain trust the daemon across updates.
        env["MERMOZ_USE_KEYCHAIN"] = "1"
    else:
        env.pop("MERMOZ_USE_KEYCHAIN", None)
    run(["cargo", "build", "--release", "--bins"], cwd=REPO, env=env)


def rust_binaries(dry_run: bool) -> list[Path]:
    paths = [REPO / "target" / "release" / name for name in RUST_BINS]
    missing = [p.name for p in paths if not p.exists()]
    if missing and not dry_run:
        raise click.ClickException(f"Missing Rust binaries in target/release: {', '.join(missing)}")
    return paths


def build_swift(run: signing.Runner) -> Path:
    run(["swift", "build", "-c", "release", "--product", "Mermoz"], cwd=APP_PKG)
    bin_dir = Path(run(["swift", "build", "-c", "release", "--show-bin-path"], cwd=APP_PKG, inspect=True).stdout.strip())
    exe = bin_dir / "Mermoz"
    if not exe.exists() and not run.dry_run:
        raise click.ClickException(f"Swift build produced no executable at {exe}")
    return exe


def ant_candidates(run: signing.Runner) -> list[Path]:
    """Cask installs of anthropics/tap/ant (newest version first), then `ant` on PATH."""
    found: list[Path] = []
    brew = shutil.which("brew")
    if brew:
        proc = run([brew, "--prefix"], check=False, inspect=True)
        if proc.returncode == 0 and proc.stdout.strip():
            caskroom = Path(proc.stdout.strip()) / "Caskroom" / "ant"
            versions = sorted(caskroom.glob("*/ant"), key=lambda p: _version_key(p.parent.name), reverse=True)
            found += versions
    on_path = shutil.which("ant")
    if on_path:
        found.append(Path(on_path).resolve())
    unique: list[Path] = []
    for p in found:
        if p not in unique:
            unique.append(p)
    return unique


def _version_key(version: str) -> tuple:
    # Numeric parts compare as numbers so 1.10.0 sorts after 1.9.0.
    return tuple(int(x) if x.isdigit() else -1 for x in version.split("."))


def ant_identity(run: signing.Runner, path: Path) -> str | None:
    """The Anthropic signing authority of `path`, or None when it is not Anthropic's `ant`."""
    if not signing.is_macho(path):
        log.warning("skipping %s: not a Mach-O binary (Apache Ant's launcher is a script)", path)
        return None
    verify = run(
        ["codesign", "--verify", "--strict", f"-R={ANTHROPIC_REQUIREMENT}", str(path)], check=False, inspect=True
    )
    if verify.returncode != 0:
        log.warning("skipping %s: not signed by Anthropic (team %s)", path, ANTHROPIC_TEAM)
        return None
    info = run(["codesign", "-dv", "--verbose=2", str(path)], check=False, inspect=True)
    for line in info.stderr.splitlines():
        if line.startswith("Authority=") and f"({ANTHROPIC_TEAM})" in line:
            return line.removeprefix("Authority=")
    log.warning("skipping %s: no Anthropic authority in its signature", path)
    return None


def find_anthropic_ant(run: signing.Runner, mode: str) -> Path | None:
    """The first verified Anthropic `ant`. A release (developer-id) build cannot ship without it."""
    for path in ant_candidates(run):
        identity = ant_identity(run, path)
        if identity:
            log.info("ant: %s (%s)", path, identity)
            return path
    hint = "brew install --cask anthropics/tap/ant"
    if mode == signing.DEVELOPER_ID:
        raise click.ClickException(f"No Anthropic-signed ant CLI found ({hint}): a release must bundle it.")
    log.warning("Anthropic's ant CLI not found (%s): Sign in with Anthropic disabled", hint)
    return None


def info_plist() -> dict:
    return {
        "CFBundleIdentifier": "ai.mermoz.app",
        "CFBundleName": "Mermoz",
        "CFBundleDisplayName": "Mermoz",
        "CFBundleExecutable": "Mermoz",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": VERSION,
        "CFBundleVersion": VERSION,
        "LSMinimumSystemVersion": "26.0",
        "NSHighResolutionCapable": True,
        "LSApplicationCategoryType": "public.app-category.productivity",
        "NSPrincipalClass": "NSApplication",
        "CFBundleIconFile": "AppIcon",
    }


def assemble(app_exe: Path, extras: list[Path], ant: Path | None) -> None:
    if OUT.exists():
        shutil.rmtree(OUT)
    macos = OUT / "Contents" / "MacOS"
    res = OUT / "Contents" / "Resources"
    macos.mkdir(parents=True)
    res.mkdir(parents=True)
    shutil.copy2(app_exe, macos / "Mermoz")
    for p in extras:
        shutil.copy2(p, res / p.name)
    # Anthropic's `ant` CLI (MIT) performs "Sign in with Anthropic"; ship it with its license.
    if ant:
        shutil.copy2(ant, res / "ant")
        shutil.copy2(REPO / "third_party" / "ant-LICENSE", res / "ant-LICENSE.txt")
    icon = APP_PKG / "Resources" / "AppIcon.icns"
    if icon.exists():
        shutil.copy2(icon, res / "AppIcon.icns")
    else:
        log.warning("no app icon; run scripts/make_icon.py")
    with open(OUT / "Contents" / "Info.plist", "wb") as f:
        plistlib.dump(info_plist(), f)
    log.info("assembled %s", OUT.relative_to(REPO))


def sign(run: signing.Runner, mode: str, keychain: Path | None) -> None:
    identity = None
    if mode == signing.DEVELOPER_ID:
        identity = signing.signing_identity(run, os.environ.get(signing.IDENTITY_ENV), keychain)
        log.info("signing with %s", identity)
    if run.dry_run and not OUT.exists():
        log.info("(dry run) no %s yet: the signing plan is listed once it has been built", OUT.relative_to(REPO))
        return
    signing.sign_app(run, OUT, mode, identity, ENTITLEMENTS, keychain)


@click.command()
@click.option("--open", "open_app", is_flag=True, help="Launch the app after building.")
@click.option("--skip-rust", is_flag=True, help="Reuse binaries already in target/release.")
@click.option(
    "--sign",
    "mode",
    type=click.Choice(signing.MODES),
    default=signing.ADHOC,
    show_default=True,
    help="adhoc for local builds, developer-id for releases.",
)
@click.option(
    "--keychain",
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    help="Look up the signing identity in this keychain only (CI, tests).",
)
@click.option("--sign-only", is_flag=True, help="Re-sign the existing build/Mermoz.app; build nothing.")
@click.option("--dry-run", is_flag=True, help="Log every command without running it; build/Mermoz.app is left as is.")
@click.option("--check-ant", is_flag=True, help="Find and verify Anthropic's ant CLI, then exit; build nothing.")
def main(
    open_app: bool, skip_rust: bool, mode: str, keychain: Path | None, sign_only: bool, dry_run: bool, check_ant: bool
) -> None:
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    if sys.platform != "darwin":
        raise click.ClickException("Mermoz.app can only be built on macOS.")
    run = signing.Runner(dry_run=dry_run)
    if check_ant:
        # Strict, whatever --sign says: the caller asked for a verified ant.
        find_anthropic_ant(run, signing.DEVELOPER_ID)
        return
    try:
        if not sign_only:
            # Before any compilation, so a release without ant fails in seconds.
            ant = find_anthropic_ant(run, mode)
            if not skip_rust:
                build_rust(run, mode)
            extras = rust_binaries(dry_run)
            app_exe = build_swift(run)
            if dry_run:
                log.info("(dry run) would assemble %s", OUT.relative_to(REPO))
            else:
                assemble(app_exe, extras, ant)
        elif not OUT.exists():
            raise click.ClickException(f"--sign-only: no {OUT.relative_to(REPO)} to sign")
        sign(run, mode, keychain)
    except signing.SigningError as e:
        raise click.ClickException(str(e)) from e
    log.info("built %s (%s)", OUT, mode)
    if open_app and not dry_run:
        subprocess.run(["open", str(OUT)], check=True)


if __name__ == "__main__":
    main()
