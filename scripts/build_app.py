#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["click"]
# ///
"""Build Mermoz.app: Rust daemon + CLI, SwiftUI app, bundled and ad-hoc signed.

Steps:
  1. `cargo build --release` at the repo root (produces `mermozd` and `mermoz`).
  2. `swift build -c release` in `app/` (produces the `Mermoz` executable).
  3. Assemble `build/Mermoz.app` with the daemon and CLI in Contents/Resources.
  4. Ad-hoc codesign the bundle so macOS launches it locally.

Usage:
  scripts/build_app.py            # build everything
  scripts/build_app.py --open     # build, then launch the app
  scripts/build_app.py --skip-rust  # reuse existing target/release binaries
"""

from __future__ import annotations

import logging
import plistlib
import shutil
import subprocess
import sys
from pathlib import Path

import click

log = logging.getLogger("build_app")

# Resolved from this file so the script works from any checkout location.
REPO = Path(__file__).resolve().parent.parent
APP_PKG = REPO / "app"
OUT = REPO / "build" / "Mermoz.app"
RUST_BINS = ("mermozd", "mermoz")
VERSION = "0.1.0"


def run(cmd: list[str], cwd: Path) -> str:
    """Runs a build step, streaming nothing, and fails loudly with its output."""
    log.info("$ %s  (in %s)", " ".join(cmd), cwd.relative_to(REPO) if cwd != REPO else ".")
    proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    if proc.returncode != 0:
        log.error("%s\n%s", proc.stdout[-4000:], proc.stderr[-4000:])
        raise click.ClickException(f"step failed: {' '.join(cmd)}")
    return proc.stdout


def build_rust() -> None:
    if not (REPO / "Cargo.toml").exists():
        raise click.ClickException("No Cargo.toml at the repo root: the Rust daemon/CLI crates are missing.")
    run(["cargo", "build", "--release", "--bins"], REPO)


def rust_binaries() -> list[Path]:
    paths = [REPO / "target" / "release" / name for name in RUST_BINS]
    missing = [p.name for p in paths if not p.exists()]
    if missing:
        raise click.ClickException(f"Missing Rust binaries in target/release: {', '.join(missing)}")
    return paths


def build_swift() -> Path:
    run(["swift", "build", "-c", "release", "--product", "Mermoz"], APP_PKG)
    bin_dir = Path(run(["swift", "build", "-c", "release", "--show-bin-path"], APP_PKG).strip())
    exe = bin_dir / "Mermoz"
    if not exe.exists():
        raise click.ClickException(f"Swift build produced no executable at {exe}")
    return exe


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


def assemble(app_exe: Path, extras: list[Path]) -> None:
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
    ant = shutil.which("ant")
    if ant:
        shutil.copy2(Path(ant).resolve(), res / "ant")
        shutil.copy2(REPO / "third_party" / "ant-LICENSE", res / "ant-LICENSE.txt")
    else:
        log.warning("ant CLI not found (brew install anthropics/tap/ant): Sign in with Anthropic disabled")
    icon = APP_PKG / "Resources" / "AppIcon.icns"
    if icon.exists():
        shutil.copy2(icon, res / "AppIcon.icns")
    else:
        log.warning("no app icon; run scripts/make_icon.py")
    with open(OUT / "Contents" / "Info.plist", "wb") as f:
        plistlib.dump(info_plist(), f)
    log.info("assembled %s", OUT.relative_to(REPO))


def sign() -> None:
    run(["codesign", "--force", "--deep", "-s", "-", str(OUT)], REPO)


@click.command()
@click.option("--open", "open_app", is_flag=True, help="Launch the app after building.")
@click.option("--skip-rust", is_flag=True, help="Reuse binaries already in target/release.")
def main(open_app: bool, skip_rust: bool) -> None:
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    if sys.platform != "darwin":
        raise click.ClickException("Mermoz.app can only be built on macOS.")
    if not skip_rust:
        build_rust()
    extras = rust_binaries()
    app_exe = build_swift()
    assemble(app_exe, extras)
    sign()
    log.info("built %s", OUT)
    if open_app:
        subprocess.run(["open", str(OUT)], check=True)


if __name__ == "__main__":
    main()
