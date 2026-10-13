"""End-to-end signing with real `codesign`: a throwaway "Developer ID Application"
identity (self-signed) in a temporary keychain signs a small app bundle laid out like
Mermoz.app, through the same code path as `build_app.py --sign developer-id`.

Opt-in (MERMOZ_SIGNING_E2E=1) and macOS only: codesign only finds identities in keychains
on the search list, so the temporary keychain joins the user search list for the test and
the previous list is restored afterwards. The login keychain itself is never written.
Notarization is out of reach: Apple only notarizes real Developer ID signatures.
"""

from __future__ import annotations

import os
import plistlib
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest
import signing

pytestmark = pytest.mark.skipif(
    sys.platform != "darwin" or os.environ.get("MERMOZ_SIGNING_E2E") != "1",
    reason="real codesign run: macOS with MERMOZ_SIGNING_E2E=1",
)

REPO = Path(__file__).resolve().parents[2]
ENTITLEMENTS = REPO / "app" / "Mermoz.entitlements"
TEST_IDENTITY = "Developer ID Application: Mermoz Signing Test (TEST000000)"
PASSWORD = "mermoz-test"  # throwaway keychain, deleted after the test


def sh(*cmd: str, cwd: Path | None = None) -> str:
    return subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True).stdout


def search_list() -> list[str]:
    return re.findall(r'"([^"]+)"', sh("security", "list-keychains", "-d", "user"))


@pytest.fixture(scope="module")
def keychain(tmp_path_factory) -> tuple[Path, str]:
    """(keychain path, identity SHA-1) of a self-signed code signing identity."""
    d = tmp_path_factory.mktemp("kc")
    (d / "cert.cnf").write_text(
        "[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n"
        f"[dn]\nCN={TEST_IDENTITY}\nOU=TEST000000\n"
        "[ext]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n"
        "extendedKeyUsage=critical,codeSigning\n"
    )
    # /usr/bin/openssl (LibreSSL) writes a PKCS#12 that `security import` reads.
    sh("/usr/bin/openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "key.pem",
       "-out", "cert.pem", "-days", "1", "-config", "cert.cnf", cwd=d)  # fmt: skip
    sh("/usr/bin/openssl", "pkcs12", "-export", "-out", "id.p12", "-inkey", "key.pem", "-in", "cert.pem",
       "-passout", f"pass:{PASSWORD}", cwd=d)  # fmt: skip
    kc = d / "signing-test.keychain-db"
    sh("security", "create-keychain", "-p", PASSWORD, str(kc))
    sh("security", "set-keychain-settings", "-lut", "600", str(kc))
    sh("security", "unlock-keychain", "-p", PASSWORD, str(kc))
    sh("security", "import", str(d / "id.p12"), "-k", str(kc), "-P", PASSWORD, "-T", "/usr/bin/codesign")
    # Lets codesign use the key without a password dialog.
    sh("security", "set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", PASSWORD, str(kc))
    listed = sh("security", "find-identity", "-p", "codesigning", str(kc))
    sha1 = re.search(r"([0-9A-F]{40}) \"" + re.escape(TEST_IDENTITY), listed).group(1)
    before = search_list()
    sh("security", "list-keychains", "-d", "user", "-s", *before, str(kc))
    try:
        yield kc, sha1
    finally:
        sh("security", "list-keychains", "-d", "user", "-s", *before)
        subprocess.run(["security", "delete-keychain", str(kc)], check=False, capture_output=True)


def compile_tool(src_dir: Path, name: str, out: Path) -> None:
    src = src_dir / f"{name}.c"
    src.write_text(f'#include <stdio.h>\nint main(void) {{ puts("{name}"); return 0; }}\n')
    out.parent.mkdir(parents=True, exist_ok=True)
    sh("xcrun", "clang", "-O2", "-o", str(out), str(src))


@pytest.fixture(scope="module")
def signed_app(keychain, tmp_path_factory) -> Path:
    kc, sha1 = keychain
    d = tmp_path_factory.mktemp("app")
    app = d / "Mermoz.app"
    contents = app / "Contents"
    compile_tool(d, "Mermoz", contents / "MacOS" / "Mermoz")
    compile_tool(d, "mermozd", contents / "Resources" / "mermozd")
    compile_tool(d, "mermoz", contents / "Resources" / "mermoz")
    ant = shutil.which("ant")
    if ant:  # the real vendor-signed binary, when installed
        shutil.copy2(Path(ant).resolve(), contents / "Resources" / "ant")
    (contents / "Info.plist").write_bytes(
        plistlib.dumps(
            {"CFBundleIdentifier": "ai.mermoz.app", "CFBundleExecutable": "Mermoz", "CFBundlePackageType": "APPL"}
        )
    )
    signing.sign_app(signing.Runner(), app, signing.DEVELOPER_ID, sha1, ENTITLEMENTS, kc)
    return app


def details(path: Path) -> str:
    p = subprocess.run(["codesign", "-dvv", str(path)], capture_output=True, text=True)
    return p.stderr


def test_bundle_verifies_strict_and_deep(signed_app: Path):
    p = subprocess.run(["codesign", "--verify", "--strict", "--deep", "--verbose=2", str(signed_app)])
    assert p.returncode == 0


def test_app_is_signed_by_the_identity(signed_app: Path):
    assert f"Authority={TEST_IDENTITY}" in details(signed_app)


def test_app_has_hardened_runtime(signed_app: Path):
    assert "(runtime)" in details(signed_app)


def test_app_has_a_secure_timestamp(signed_app: Path):
    assert "Timestamp=" in details(signed_app)


def test_daemon_is_signed_by_the_identity(signed_app: Path):
    assert f"Authority={TEST_IDENTITY}" in details(signed_app / "Contents" / "Resources" / "mermozd")


def test_daemon_has_its_stable_identifier(signed_app: Path):
    assert "Identifier=ai.mermoz.mermozd" in details(signed_app / "Contents" / "Resources" / "mermozd")


def test_daemon_has_hardened_runtime(signed_app: Path):
    assert "(runtime)" in details(signed_app / "Contents" / "Resources" / "mermozd")


def test_cli_has_its_stable_identifier(signed_app: Path):
    assert "Identifier=ai.mermoz.cli" in details(signed_app / "Contents" / "Resources" / "mermoz")


def test_signed_tools_still_run(signed_app: Path):
    assert sh(str(signed_app / "Contents" / "Resources" / "mermozd")) == "mermozd\n"


@pytest.mark.skipif(shutil.which("ant") is None, reason="ant CLI not installed")
def test_ant_keeps_anthropics_signature(signed_app: Path):
    assert "Authority=Developer ID Application: Anthropic PBC" in details(signed_app / "Contents" / "Resources" / "ant")


def test_entitlements_carry_no_debugger_exception(signed_app: Path):
    p = subprocess.run(
        ["codesign", "-d", "--entitlements", "-", "--xml", str(signed_app)], capture_output=True, text=True
    )
    assert "get-task-allow" not in p.stdout
