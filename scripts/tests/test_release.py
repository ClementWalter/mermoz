"""Unit tests for scripts/release.py: notary credentials, notarytool results, and the
order of the signed release's commands in a dry run."""

from __future__ import annotations

import logging
import subprocess

import pytest
import release
import signing
from click.testing import CliRunner

API_KEY_ENV = {
    "MERMOZ_NOTARY_KEY_ID": "KEY123",
    "MERMOZ_NOTARY_ISSUER": "issuer-uuid",
    "MERMOZ_NOTARY_KEY_PATH": "/keys/AuthKey_KEY123.p8",
}


def test_api_key_credentials_from_the_environment():
    assert release.notary_auth(API_KEY_ENV, "mermoz-notary") == [
        "--key",
        "/keys/AuthKey_KEY123.p8",
        "--key-id",
        "KEY123",
        "--issuer",
        "issuer-uuid",
    ]


def test_keychain_profile_without_api_key():
    assert release.notary_auth({}, "mermoz-notary") == ["--keychain-profile", "mermoz-notary"]


def test_incomplete_api_key_names_what_is_missing():
    with pytest.raises(signing.SigningError, match="MERMOZ_NOTARY_ISSUER"):
        release.notary_auth({"MERMOZ_NOTARY_KEY_ID": "KEY123", "MERMOZ_NOTARY_KEY_PATH": "/k.p8"}, "p")


def test_submission_result_is_parsed():
    assert release.parse_submission('{"id": "abc-123", "status": "Accepted", "message": "ok"}') == (
        "abc-123",
        "Accepted",
    )


def test_unreadable_submission_result_is_unknown():
    assert release.parse_submission("Error: HTTP 401") == (None, "Unknown")


class NotaryStub(signing.Runner):
    """notarytool answering `status` for the submission and a fixed log."""

    def __init__(self, status: str):
        super().__init__(dry_run=False)
        self.status = status
        self.commands: list[list[str]] = []

    def __call__(self, cmd, **kwargs):
        self.commands.append(cmd)
        if cmd[:3] == ["xcrun", "notarytool", "submit"]:
            return subprocess.CompletedProcess(cmd, 0, f'{{"id": "sub-1", "status": "{self.status}"}}', "")
        return subprocess.CompletedProcess(cmd, 0, '{"issues": [{"message": "The binary is not signed."}]}', "")


def test_rejected_notarization_raises(tmp_path):
    with pytest.raises(signing.SigningError, match="Invalid"):
        release.notarize(NotaryStub("Invalid"), tmp_path / "Mermoz.dmg", ["--keychain-profile", "p"])


def test_rejected_notarization_logs_apples_report(tmp_path, caplog):
    caplog.set_level(logging.ERROR, logger="release")
    with pytest.raises(signing.SigningError):
        release.notarize(NotaryStub("Invalid"), tmp_path / "Mermoz.dmg", ["--keychain-profile", "p"])
    assert "The binary is not signed." in caplog.text


def test_accepted_notarization_fetches_no_log(tmp_path):
    run = NotaryStub("Accepted")
    release.notarize(run, tmp_path / "Mermoz.dmg", ["--keychain-profile", "p"])
    assert len(run.commands) == 1


def test_publishing_an_adhoc_build_is_refused():
    result = CliRunner().invoke(release.main, ["--sign", "adhoc", "--publish", "--skip-build", "--dry-run"])
    assert "Developer ID" in result.output


def logged_commands(caplog) -> list[str]:
    """`program subcommand` of each command the dry run logged, in order."""
    out = []
    for r in caplog.records:
        msg = r.getMessage()
        if msg.startswith("$ "):
            words = msg[2:].split()
            out.append(" ".join(words[:3] if words[0] == "xcrun" else words[:2]))
    return out


def test_signed_release_dry_run_order(monkeypatch, caplog):
    monkeypatch.setenv("MERMOZ_SIGN_IDENTITY", "Developer ID Application: Test (ABCDE12345)")
    for k, v in API_KEY_ENV.items():
        monkeypatch.setenv(k, v)
    caplog.set_level(logging.INFO)
    result = CliRunner().invoke(release.main, ["--skip-build", "--dry-run", "--publish"])
    assert (result.exit_code, logged_commands(caplog)) == (
        0,
        [
            "ditto -c",
            "xcrun notarytool submit",
            "xcrun stapler staple",
            "xcrun stapler validate",
            "hdiutil create",
            "codesign --force",
            "xcrun notarytool submit",
            "xcrun stapler staple",
            "xcrun stapler validate",
            "codesign --verify",
            "spctl -a",
            "xcrun stapler validate",
            "codesign --verify",
            "spctl -a",
            "xcrun stapler validate",
            "gh release",
        ],
    )
