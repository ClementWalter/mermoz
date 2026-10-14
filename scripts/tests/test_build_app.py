"""Unit tests for build_app.py's choice of the `ant` CLI to bundle: only a Mach-O signed by
Anthropic's team qualifies, the Homebrew cask copy wins over PATH, and a release build
fails without one. `brew`, PATH and codesign are stubbed; nothing on the system changes."""

from __future__ import annotations

import subprocess
from pathlib import Path

import build_app
import click
import pytest
import signing
from conftest import write_macho

ANTHROPIC = f"Developer ID Application: Anthropic PBC ({build_app.ANTHROPIC_TEAM})"


class CodesignStub(signing.Runner):
    """Answers `brew --prefix` and codesign; `anthropic` holds the paths Anthropic signed."""

    def __init__(self, prefix: Path, anthropic: set[Path]):
        super().__init__(dry_run=False)
        self.prefix = prefix
        self.anthropic = anthropic

    def __call__(self, cmd, **kwargs):
        if cmd[1:] == ["--prefix"]:
            return subprocess.CompletedProcess(cmd, 0, f"{self.prefix}\n", "")
        signed = Path(cmd[-1]) in self.anthropic
        if cmd[:2] == ["codesign", "--verify"]:
            return subprocess.CompletedProcess(cmd, 0 if signed else 3, "", "")
        if cmd[:2] == ["codesign", "-dv"]:
            authority = ANTHROPIC if signed else "Developer ID Application: Someone Else (ZZZZZ99999)"
            return subprocess.CompletedProcess(cmd, 0, "", f"Authority={authority}\n")
        raise AssertionError(f"unexpected command {cmd}")


@pytest.fixture
def prefix(tmp_path: Path) -> Path:
    return tmp_path / "homebrew"


@pytest.fixture
def cask_ant(prefix: Path) -> Path:
    return write_macho(prefix / "Caskroom" / "ant" / "1.40.0" / "ant")


@pytest.fixture
def path_ant(tmp_path: Path) -> Path:
    return write_macho(tmp_path / "bin" / "ant")


@pytest.fixture
def which(monkeypatch: pytest.MonkeyPatch, path_ant: Path):
    """PATH holds `brew` and `path_ant`."""
    tools = {"brew": "/stub/brew", "ant": str(path_ant)}
    monkeypatch.setattr(build_app.shutil, "which", tools.get)
    return tools


def test_a_script_named_ant_is_rejected(tmp_path: Path, prefix: Path):
    script = tmp_path / "ant"
    script.write_text("#!/bin/sh\nexec java org.apache.tools.ant.launch.Launcher \"$@\"\n")
    assert build_app.ant_identity(CodesignStub(prefix, {script}), script) is None


def test_a_mach_o_signed_by_another_team_is_rejected(prefix: Path, path_ant: Path):
    assert build_app.ant_identity(CodesignStub(prefix, set()), path_ant) is None


def test_an_anthropic_signed_mach_o_is_accepted(prefix: Path, path_ant: Path):
    assert build_app.ant_identity(CodesignStub(prefix, {path_ant}), path_ant) == ANTHROPIC


def test_the_cask_ant_is_preferred_over_path(which, prefix: Path, cask_ant: Path, path_ant: Path):
    run = CodesignStub(prefix, {cask_ant, path_ant})
    assert build_app.find_anthropic_ant(run, signing.DEVELOPER_ID) == cask_ant


def test_the_newest_cask_version_comes_first(which, prefix: Path, cask_ant: Path):
    newer = write_macho(prefix / "Caskroom" / "ant" / "1.100.0" / "ant")
    assert build_app.ant_candidates(CodesignStub(prefix, set()))[0] == newer


def test_a_verified_ant_on_path_is_used_without_a_cask(which, prefix: Path, path_ant: Path):
    assert build_app.find_anthropic_ant(CodesignStub(prefix, {path_ant}), signing.DEVELOPER_ID) == path_ant


def test_release_mode_fails_without_a_verified_ant(which, prefix: Path, cask_ant: Path):
    with pytest.raises(click.ClickException, match="Anthropic-signed ant"):
        build_app.find_anthropic_ant(CodesignStub(prefix, set()), signing.DEVELOPER_ID)


def test_adhoc_mode_skips_ant_without_a_verified_one(which, prefix: Path, cask_ant: Path):
    assert build_app.find_anthropic_ant(CodesignStub(prefix, set()), signing.ADHOC) is None
