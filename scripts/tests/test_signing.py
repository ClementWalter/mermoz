"""Unit tests for scripts/signing.py: identity lookup, inside-out signing order and the
codesign commands, with external commands replaced by a recording stub."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest
import signing

DEV_ID = "Developer ID Application: Clement Walter (ABCDE12345)"
DEV_HASH = "1111111111111111111111111111111111111111"
FIND_IDENTITY = f"""
  1) 2222222222222222222222222222222222222222 "Apple Development: clement@example.com (ZZZZZ99999)"
  2) {DEV_HASH} "{DEV_ID}"
     2 valid identities found
"""
TWO_DEV_IDS = (
    FIND_IDENTITY
    + '  3) 3333333333333333333333333333333333333333 "Developer ID Application: Other (QQQQQ11111)"\n'
)
ENTITLEMENTS = Path("app/Mermoz.entitlements")


class RecordingRunner(signing.Runner):
    """Records commands; `codesign -dvv` answers like a vendor-signed binary for `vendor`."""

    def __init__(self, vendor: set[str] | None = None):
        super().__init__(dry_run=False)
        self.vendor = vendor or set()
        self.commands: list[list[str]] = []

    def __call__(self, cmd, **kwargs):
        self.commands.append(cmd)
        if cmd[:2] == ["codesign", "-dvv"] and Path(cmd[-1]).name in self.vendor:
            info = f"Authority={DEV_ID}\nflags=0x10000(runtime)\nTimestamp=9 Oct 2026\n"
            return subprocess.CompletedProcess(cmd, 0, "", info)
        if cmd[:2] == ["codesign", "-dvv"]:
            return subprocess.CompletedProcess(cmd, 1, "", "code object is not signed at all")
        return subprocess.CompletedProcess(cmd, 0, "", "")


def names(targets: list[signing.Target]) -> list[str]:
    return [t.path.name for t in targets]


def signed_paths(run: RecordingRunner) -> list[str]:
    return [Path(c[-1]).name for c in run.commands if c[:2] == ["codesign", "--force"]]


# ---- identity ----


def test_the_only_developer_id_identity_is_chosen():
    assert signing.choose_identity(None, FIND_IDENTITY) == DEV_HASH


def test_env_identity_wins_over_the_keychain():
    assert signing.choose_identity("  My Identity ", FIND_IDENTITY) == "My Identity"


def test_apple_development_identities_are_ignored():
    assert list(signing.developer_id_identities(FIND_IDENTITY).values()) == [DEV_ID]


def test_repeated_listing_counts_once():
    assert len(signing.developer_id_identities(FIND_IDENTITY + FIND_IDENTITY)) == 1


def test_no_developer_id_identity_is_an_error():
    with pytest.raises(signing.SigningError, match="No 'Developer ID Application'"):
        signing.choose_identity(None, "     0 valid identities found\n")


def test_several_developer_id_identities_ask_for_the_env_variable():
    with pytest.raises(signing.SigningError, match="MERMOZ_SIGN_IDENTITY"):
        signing.choose_identity(None, TWO_DEV_IDS)


def test_team_id_is_read_from_the_identity_name():
    assert signing.team_id(DEV_ID) == "ABCDE12345"


def test_dry_run_without_identity_shows_a_placeholder(monkeypatch):
    monkeypatch.setattr(
        signing.Runner,
        "__call__",
        lambda self, cmd, **kw: subprocess.CompletedProcess(cmd, 0, "     0 valid identities found\n", ""),
    )
    assert signing.signing_identity(signing.Runner(dry_run=True), None) == signing.DRY_RUN_IDENTITY


# ---- signing order ----


def test_plan_is_inside_out(fake_app: Path):
    assert names(signing.signing_plan(fake_app)) == [
        "kit-tool",
        "mermozd",
        "mermoz",
        "ant",
        "Kit.framework",
        "Mermoz.app",
    ]


def test_plan_gives_the_daemon_its_stable_identifier(fake_app: Path):
    daemon = next(t for t in signing.signing_plan(fake_app) if t.path.name == "mermozd")
    assert daemon.identifier == "ai.mermoz.mermozd"


def test_plan_gives_the_cli_its_stable_identifier(fake_app: Path):
    cli = next(t for t in signing.signing_plan(fake_app) if t.path.name == "mermoz")
    assert cli.identifier == "ai.mermoz.cli"


@pytest.mark.parametrize(
    ("name", "expected"),
    [("mermozd", True), ("mermoz", True), ("Mermoz.app", True), ("ant", False), ("Kit.framework", False)],
)
def test_entitlements_go_to_our_own_code_only(fake_app: Path, name: str, expected: bool):
    target = next(t for t in signing.signing_plan(fake_app) if t.path.name == name)
    assert target.entitlements is expected


# ---- codesign commands ----


def test_developer_id_command_has_runtime_timestamp_and_entitlements(fake_app: Path):
    target = signing.Target(fake_app, entitlements=True)
    assert signing.codesign_cmd(target, signing.DEVELOPER_ID, DEV_HASH, ENTITLEMENTS) == [
        "codesign",
        "--force",
        "--sign",
        DEV_HASH,
        "--options",
        "runtime",
        "--timestamp",
        "--entitlements",
        str(ENTITLEMENTS),
        str(fake_app),
    ]


def test_developer_id_command_can_name_a_keychain(fake_app: Path):
    target = signing.Target(fake_app / "Contents" / "Resources" / "ant")
    cmd = signing.codesign_cmd(target, signing.DEVELOPER_ID, DEV_HASH, ENTITLEMENTS, Path("ci.keychain-db"))
    assert cmd[cmd.index("--keychain") + 1] == "ci.keychain-db"


def test_adhoc_command_keeps_the_identifier(fake_app: Path):
    daemon = fake_app / "Contents" / "Resources" / "mermozd"
    target = signing.Target(daemon, identifier="ai.mermoz.mermozd", entitlements=True)
    assert signing.codesign_cmd(target, signing.ADHOC, None, ENTITLEMENTS) == [
        "codesign",
        "--force",
        "--sign",
        "-",
        "--identifier",
        "ai.mermoz.mermozd",
        str(daemon),
    ]


def test_signing_never_uses_deep(fake_app: Path):
    run = RecordingRunner()
    signing.sign_app(run, fake_app, signing.DEVELOPER_ID, DEV_HASH, ENTITLEMENTS)
    assert not any("--deep" in c for c in run.commands if c[:2] == ["codesign", "--force"])


def test_developer_id_without_identity_is_an_error(fake_app: Path):
    with pytest.raises(signing.SigningError):
        signing.codesign_cmd(signing.Target(fake_app), signing.DEVELOPER_ID, None, ENTITLEMENTS)


def test_sign_app_keeps_a_vendor_developer_id_signature(fake_app: Path):
    run = RecordingRunner(vendor={"ant"})
    signing.sign_app(run, fake_app, signing.DEVELOPER_ID, DEV_HASH, ENTITLEMENTS)
    assert signed_paths(run) == ["kit-tool", "mermozd", "mermoz", "Kit.framework", "Mermoz.app"]


def test_sign_app_signs_an_unsigned_vendor_binary(fake_app: Path):
    run = RecordingRunner()
    signing.sign_app(run, fake_app, signing.DEVELOPER_ID, DEV_HASH, ENTITLEMENTS)
    assert "ant" in signed_paths(run)


def test_sign_app_ends_with_a_deep_strict_verification(fake_app: Path):
    run = RecordingRunner()
    signing.sign_app(run, fake_app, signing.ADHOC, None, ENTITLEMENTS)
    assert run.commands[-1] == ["codesign", "--verify", "--strict", "--deep", "--verbose=2", str(fake_app)]


def test_dry_run_runs_nothing(tmp_path: Path):
    marker = tmp_path / "touched"
    signing.Runner(dry_run=True)(["touch", str(marker)])
    assert not marker.exists()


def test_dry_run_still_runs_inspections():
    assert signing.Runner(dry_run=True)(["echo", "hi"], inspect=True).stdout == "hi\n"
