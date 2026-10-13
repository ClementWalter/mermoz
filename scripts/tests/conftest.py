"""Shared fixtures for the release-script tests: the scripts directory on the import path
and a fake Mermoz.app laid out like build_app.py assembles it.

Run: uv run --with pytest --with click pytest scripts/tests
"""

from __future__ import annotations

import plistlib
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

# A minimal Mach-O header: what signing.is_macho looks at.
MACHO = b"\xcf\xfa\xed\xfe" + b"\0" * 28


def write_macho(path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(MACHO)
    return path


@pytest.fixture
def fake_app(tmp_path: Path) -> Path:
    """Mermoz.app with the app executable, daemon, CLI, ant, a framework and plain resources."""
    app = tmp_path / "Mermoz.app"
    contents = app / "Contents"
    contents.mkdir(parents=True)
    (contents / "Info.plist").write_bytes(plistlib.dumps({"CFBundleExecutable": "Mermoz"}))
    write_macho(contents / "MacOS" / "Mermoz")
    for name in ("ant", "mermoz", "mermozd"):
        write_macho(contents / "Resources" / name)
    (contents / "Resources" / "AppIcon.icns").write_bytes(b"icns")
    (contents / "Resources" / "ant-LICENSE.txt").write_text("MIT")
    fw = contents / "Frameworks" / "Kit.framework"
    versions = fw / "Versions" / "A"
    (versions / "Resources").mkdir(parents=True)
    (versions / "Resources" / "Info.plist").write_bytes(plistlib.dumps({"CFBundleExecutable": "Kit"}))
    write_macho(versions / "Kit")
    write_macho(versions / "Helpers" / "kit-tool")
    (fw / "Versions" / "Current").symlink_to("A")
    (fw / "Kit").symlink_to("Versions/Current/Kit")
    (fw / "Resources").symlink_to("Versions/Current/Resources")
    return app
