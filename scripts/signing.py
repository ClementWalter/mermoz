"""Code signing for Mermoz.app: signing identity lookup, the inside-out signing plan and
the exact `codesign` commands.

Imported by build_app.py (signs the bundle) and release.py (signs the DMG). Two modes:

- `adhoc` (default, local development): `codesign -s -`, no Apple account needed.
- `developer-id` (releases): a "Developer ID Application" identity, Hardened Runtime,
  a secure timestamp and `app/Mermoz.entitlements`, as notarization requires.

Every command goes through `Runner`, so a dry run logs the exact commands instead of
running them. Signing never uses `--deep`: each nested binary and bundle is signed on its
own, deepest first, then the app itself.
"""

from __future__ import annotations

import logging
import plistlib
import re
import shlex
import subprocess
from dataclasses import dataclass
from pathlib import Path

log = logging.getLogger("signing")

ADHOC = "adhoc"
DEVELOPER_ID = "developer-id"
MODES = (ADHOC, DEVELOPER_ID)

IDENTITY_ENV = "MERMOZ_SIGN_IDENTITY"
DEVELOPER_ID_PREFIX = "Developer ID Application:"

# The daemon's Keychain items trust its designated requirement (identifier + Team ID):
# changing an identifier makes every saved secret ask for the password again.
IDENTIFIERS = {"mermozd": "ai.mermoz.mermozd", "mermoz": "ai.mermoz.cli"}
# Our own binaries get the entitlements; vendor binaries keep what their vendor chose.
OWN_BINARIES = frozenset(IDENTIFIERS)
# Among siblings: daemon, CLI, other loose binaries (helpers, vendor tools), then bundles.
PRIORITY = {"mermozd": 0, "mermoz": 1}

MACHO_MAGICS = frozenset(
    {
        b"\xcf\xfa\xed\xfe",  # 64-bit, little endian
        b"\xce\xfa\xed\xfe",  # 32-bit, little endian
        b"\xfe\xed\xfa\xcf",
        b"\xfe\xed\xfa\xce",
        b"\xca\xfe\xba\xbe",  # universal
        b"\xbe\xba\xfe\xca",
    }
)
BUNDLE_SUFFIXES = (".framework", ".app", ".appex", ".xpc", ".bundle", ".plugin")


class SigningError(Exception):
    """A signing step failed or its inputs are missing; the message says what to do."""


@dataclass
class Runner:
    """Runs (or, in a dry run, only logs) external commands."""

    dry_run: bool = False

    def __call__(
        self,
        cmd: list[str],
        *,
        cwd: Path | None = None,
        check: bool = True,
        inspect: bool = False,
        env: dict | None = None,
        capture: bool = True,
    ) -> subprocess.CompletedProcess:
        """`inspect` marks read-only commands, which run even in a dry run so the plan is
        real. `capture=False` streams a long step's output (a nested build) live."""
        log.info("$ %s", shlex.join(cmd))
        if self.dry_run and not inspect:
            return subprocess.CompletedProcess(cmd, 0, "", "")
        proc = subprocess.run(cmd, check=False, cwd=cwd, capture_output=capture, text=True, env=env)
        if check and proc.returncode != 0:
            if capture:
                log.error("%s\n%s", proc.stdout[-4000:], proc.stderr[-4000:])
            raise SigningError(f"command failed ({proc.returncode}): {shlex.join(cmd)}")
        return proc


# ---- identity ----

_IDENTITY_LINE = re.compile(r'^\s*\d+\)\s+([0-9A-F]{40})\s+"([^"]+)"')


def developer_id_identities(find_identity_output: str) -> dict[str, str]:
    """SHA-1 → name of each "Developer ID Application" identity in `security find-identity`."""
    found: dict[str, str] = {}
    for line in find_identity_output.splitlines():
        m = _IDENTITY_LINE.match(line)
        if m and m.group(2).startswith(DEVELOPER_ID_PREFIX):
            found[m.group(1)] = m.group(2)
    return found


def choose_identity(env_value: str | None, find_identity_output: str) -> str:
    """The identity to sign with: `MERMOZ_SIGN_IDENTITY` if set, else the only Developer ID
    identity available. A SHA-1 is returned rather than a name, since names can repeat
    (an expired and a renewed certificate share one)."""
    if env_value and env_value.strip():
        return env_value.strip()
    ids = developer_id_identities(find_identity_output)
    if not ids:
        raise SigningError(
            "No 'Developer ID Application' signing identity found. Create one "
            "(docs/release-plan.md, 'Developer ID signing') or set "
            f"{IDENTITY_ENV}."
        )
    if len(ids) > 1:
        listed = "; ".join(f"{h} {n}" for h, n in sorted(ids.items()))
        raise SigningError(f"Several Developer ID identities; set {IDENTITY_ENV} to one of: {listed}")
    return next(iter(ids))


def find_identity(run: Runner, env_value: str | None, keychain: Path | None = None) -> str:
    """Looks up the signing identity in the keychain search list (or one keychain)."""
    if env_value and env_value.strip():
        return env_value.strip()
    cmd = ["security", "find-identity", "-v", "-p", "codesigning"]
    if keychain:
        cmd.append(str(keychain))
    return choose_identity(None, run(cmd, inspect=True).stdout)


DRY_RUN_IDENTITY = "<Developer ID Application identity>"


def signing_identity(run: Runner, env_value: str | None, keychain: Path | None = None) -> str:
    """`find_identity`, except that a dry run without an identity yet shows a placeholder."""
    try:
        return find_identity(run, env_value, keychain)
    except SigningError as e:
        if not run.dry_run:
            raise
        log.warning("(dry run) %s", e)
        return DRY_RUN_IDENTITY


def team_id(identity_name: str) -> str | None:
    """The Team ID in "Developer ID Application: Name (TEAMID)"."""
    m = re.search(r"\(([A-Z0-9]{10})\)\s*$", identity_name)
    return m.group(1) if m else None


# ---- what to sign, in which order ----


@dataclass(frozen=True)
class Target:
    """One piece of code to sign."""

    path: Path
    identifier: str | None = None
    entitlements: bool = False


def is_macho(path: Path) -> bool:
    if path.is_symlink() or not path.is_file():
        return False
    with open(path, "rb") as f:
        return f.read(4) in MACHO_MAGICS


def bundle_executable(bundle: Path) -> Path | None:
    """The main executable a bundle's own signature covers."""
    for info in (bundle / "Contents" / "Info.plist", bundle / "Resources" / "Info.plist"):
        if info.exists():
            name = plistlib.loads(info.read_bytes()).get("CFBundleExecutable")
            if name:
                return (info.parent / "MacOS" / name) if info.parent.name == "Contents" else (info.parent.parent / name)
    return None


def _main_executables(root: Path) -> set[Path]:
    mains: set[Path] = set()
    for b in [root, *(p for p in root.rglob("*") if p.suffix in BUNDLE_SUFFIXES and p.is_dir())]:
        exe = bundle_executable(b)
        if exe:
            mains.add(exe.resolve())
    return mains


def signing_plan(app: Path) -> list[Target]:
    """Every piece of code in `app`, inside-out: nested binaries and bundles deepest first,
    then the app. Bundles are signed as a whole; loose Mach-O files (the daemon, the CLI,
    helpers in Resources, extra tools inside frameworks) are signed one by one."""
    mains = _main_executables(app)
    nested: list[Path] = []
    for p in app.rglob("*"):
        if p.is_symlink():
            continue
        bundle = p.is_dir() and p.suffix in BUNDLE_SUFFIXES
        if bundle or (p.resolve() not in mains and is_macho(p)):
            nested.append(p)

    def order(p: Path) -> tuple:
        rank = PRIORITY.get(p.name, 3 if p.is_dir() else 2)
        return (-len(p.relative_to(app).parts), rank, str(p))

    targets = [
        Target(p, identifier=IDENTIFIERS.get(p.name), entitlements=p.name in OWN_BINARIES)
        for p in sorted(nested, key=order)
    ]
    targets.append(Target(app, entitlements=True))
    return targets


# ---- codesign ----


def vendor_signed(run: Runner, path: Path) -> bool:
    """True when a third-party binary already carries a timestamped Developer ID signature
    with Hardened Runtime (notarization accepts it as is, and it keeps its vendor's
    provenance). `ant` from Anthropic ships that way."""
    proc = run(["codesign", "-dvv", str(path)], check=False, inspect=True)
    info = proc.stderr + proc.stdout
    return (
        proc.returncode == 0
        and f"Authority={DEVELOPER_ID_PREFIX}" in info
        and "(runtime)" in info
        and "Timestamp=" in info
        and run(["codesign", "--verify", "--strict", str(path)], check=False, inspect=True).returncode == 0
    )


def codesign_cmd(
    target: Target,
    mode: str,
    identity: str | None,
    entitlements: Path,
    keychain: Path | None = None,
) -> list[str]:
    """The codesign invocation for one target."""
    if mode == DEVELOPER_ID:
        if not identity:
            raise SigningError("developer-id signing needs an identity")
        cmd = ["codesign", "--force", "--sign", identity, "--options", "runtime", "--timestamp"]
        if keychain:
            cmd += ["--keychain", str(keychain)]
        if target.entitlements:
            cmd += ["--entitlements", str(entitlements)]
    elif mode == ADHOC:
        cmd = ["codesign", "--force", "--sign", "-"]
    else:
        raise SigningError(f"unknown signing mode {mode!r}; expected one of {', '.join(MODES)}")
    if target.identifier:
        cmd += ["--identifier", target.identifier]
    cmd.append(str(target.path))
    return cmd


def sign_app(
    run: Runner,
    app: Path,
    mode: str,
    identity: str | None,
    entitlements: Path,
    keychain: Path | None = None,
) -> list[Target]:
    """Signs `app` inside-out and verifies it; returns what was signed."""
    signed: list[Target] = []
    for t in signing_plan(app):
        if t.path != app and t.path.name not in OWN_BINARIES and vendor_signed(run, t.path):
            log.info("keeping the vendor's Developer ID signature on %s", t.path.relative_to(app))
            continue
        run(codesign_cmd(t, mode, identity, entitlements, keychain))
        signed.append(t)
    verify_signature(run, app)
    return signed


def sign_file(run: Runner, path: Path, identity: str, keychain: Path | None = None) -> None:
    """Signs a DMG (or any flat file) with a Developer ID identity and a secure timestamp."""
    cmd = ["codesign", "--force", "--sign", identity, "--timestamp"]
    if keychain:
        cmd += ["--keychain", str(keychain)]
    run([*cmd, str(path)])


def verify_signature(run: Runner, path: Path) -> None:
    """`--deep` is right for verifying (never for signing): it checks every nested signature."""
    run(["codesign", "--verify", "--strict", "--deep", "--verbose=2", str(path)])
