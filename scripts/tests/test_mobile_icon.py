"""The iPhone icon is a full-bleed opaque master suitable for asset catalog compilation."""

from pathlib import Path
import struct
import subprocess

import pytest


@pytest.fixture(scope="module")
def mobile_icon(tmp_path_factory):
    path = tmp_path_factory.mktemp("mobile-icon") / "AppIcon.png"
    script = Path(__file__).resolve().parents[1] / "make_icon.py"
    subprocess.run(["uv", "run", str(script), "--ios-icon", str(path)], check=True, capture_output=True)
    return path.read_bytes()


def test_the_mobile_master_has_the_required_size(mobile_icon):
    assert struct.unpack(">II", mobile_icon[16:24]) == (1024, 1024)


def test_the_mobile_master_has_no_alpha_channel(mobile_icon):
    assert mobile_icon[25] == 2
