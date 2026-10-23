"""Checks the generated agent-facing files of mermoz.xyz against their formats and
against the real CLI/MCP surface.

Run: uv run --with pytest --with jsonschema --with xmlschema --with markdown --with click pytest site/tests
"""

from __future__ import annotations

import json
import re
import sys
import xml.etree.ElementTree as ET
from html.parser import HTMLParser
from pathlib import Path

import jsonschema
import pytest
import xmlschema

REPO = Path(__file__).resolve().parents[2]
SITE = REPO / "site"
FIXTURES = Path(__file__).parent / "fixtures"
sys.path.insert(0, str(REPO / "scripts"))

import site_build  # noqa: E402

SM = "{http://www.sitemaps.org/schemas/sitemap/0.9}"
PAGE_PATHS = ["/", "/docs/", "/docs/cli/", "/docs/mcp/", "/docs/protocol/", "/about/", "/contact/", "/privacy/"]
TRUST_PAGES = ["about", "contact", "privacy"]


def page_file(url_path: str) -> Path:
    return SITE / url_path.strip("/") / "index.html" if url_path != "/" else SITE / "index.html"


class TextOf(HTMLParser):
    """The <title>, the visible text inside <article>, and each <h1>'s text."""

    def __init__(self) -> None:
        super().__init__()
        self.article = 0
        self.tag = ""
        self.title = ""
        self.text: list[str] = []
        self.h1: list[str] = []

    def handle_starttag(self, tag, attrs):
        self.tag = tag
        self.article += tag == "article"

    def handle_endtag(self, tag):
        self.article -= tag == "article"
        self.tag = ""

    def handle_data(self, data):
        if self.tag == "title":
            self.title += data
        if self.article > 0:
            self.text.append(data)


def parse(path: Path) -> TextOf:
    raw = path.read_text()
    p = TextOf()
    p.feed(raw)
    p.h1 = [re.sub(r"<[^>]+>", "", m) for m in re.findall(r"<h1[^>]*>(.*?)</h1>", raw, re.S)]
    return p


@pytest.fixture(scope="module")
def sitemap_root() -> ET.Element:
    return ET.parse(SITE / "sitemap.xml").getroot()


@pytest.fixture(scope="module")
def llms() -> str:
    return (SITE / "llms.txt").read_text()


@pytest.fixture(scope="module")
def jsonld() -> dict:
    raw = (SITE / "index.html").read_text()
    block = re.search(r'<script type="application/ld\+json">(.*?)</script>', raw, re.S).group(1)
    graph = json.loads(block)["@graph"]
    return {node["@type"]: node for node in graph}


@pytest.fixture(scope="module")
def mcp_snapshot() -> dict:
    return json.loads((SITE / "src" / "mcp-tools.json").read_text())


# ---------------------------------------------------------------- generated files are fresh


def test_generated_files_match_sources():
    stale = [p for p, content in site_build.outputs().items() if not p.exists() or p.read_text() != content]
    assert stale == []


# ---------------------------------------------------------------- sitemap and robots


def test_sitemap_is_valid_against_the_sitemap_schema():
    assert xmlschema.XMLSchema(FIXTURES / "sitemap.xsd").is_valid(str(SITE / "sitemap.xml"))


def test_sitemap_lists_every_page(sitemap_root):
    locs = sorted(u.find(f"{SM}loc").text for u in sitemap_root)
    assert locs == sorted(f"https://mermoz.xyz{p}" for p in PAGE_PATHS)


@pytest.mark.parametrize("url_path", PAGE_PATHS)
def test_sitemap_url_has_a_file(url_path):
    assert page_file(url_path).is_file()


def test_every_sitemap_url_has_lastmod(sitemap_root):
    assert all(re.fullmatch(r"\d{4}-\d{2}-\d{2}", u.find(f"{SM}lastmod").text) for u in sitemap_root)


def test_robots_points_to_the_sitemap():
    assert "Sitemap: https://mermoz.xyz/sitemap.xml" in (SITE / "robots.txt").read_text().splitlines()


def test_robots_allows_everything():
    assert "Disallow: /" not in (SITE / "robots.txt").read_text()


# ---------------------------------------------------------------- llms.txt (llmstxt.org)


def test_llms_starts_with_the_project_h1(llms):
    assert llms.splitlines()[0] == "# Mermoz"


def test_llms_has_a_blockquote_summary_after_the_h1(llms):
    assert llms.split("\n\n")[1].startswith("> ")


def test_llms_has_one_h1(llms):
    assert len(re.findall(r"^# ", llms, re.M)) == 1


def test_llms_has_no_h3_or_deeper(llms):
    assert not re.search(r"^###", llms, re.M)


def test_llms_sections_hold_only_link_lists(llms):
    sections = re.split(r"^## .*$", llms, flags=re.M)[1:]
    lines = [line for s in sections for line in s.strip().splitlines() if line.strip()]
    assert all(re.match(r"^- \[[^\]]+\]\([^)]+\)(: .+)?$", line) for line in lines)


def test_llms_has_when_to_use_section(llms):
    assert "## When to use Mermoz" in llms.splitlines()


@pytest.mark.parametrize("job", ["Fast local Gmail and Outlook client on macOS", "Agent-first mail through the CLI and the MCP server", "Offline search"])
def test_llms_when_to_use_names_the_job(llms, job):
    section = llms.split("## When to use Mermoz", 1)[1].split("\n## ", 1)[0]
    assert job in section


def test_llms_says_how_to_launch_the_mcp_server(llms):
    assert "claude mcp add mermoz -- mermoz mcp" in llms


def test_llms_lists_every_mcp_tool(llms, mcp_snapshot):
    assert all(f"[{t['name']}](" in llms for t in mcp_snapshot["tools"])


def test_llms_local_links_resolve(llms):
    paths = re.findall(r"\]\(https://mermoz\.xyz(/[^)#]*)", llms)
    missing = [p for p in paths if not (page_file(p) if p.endswith("/") else SITE / p.lstrip("/")).exists()]
    assert missing == []


def test_llms_full_contains_the_privacy_page():
    assert "# Mermoz privacy" in (SITE / "llms-full.txt").read_text()


# ---------------------------------------------------------------- JSON-LD


def test_jsonld_software_application_name(jsonld):
    assert jsonld["SoftwareApplication"]["name"] == "Mermoz"


def test_jsonld_software_application_os(jsonld):
    assert jsonld["SoftwareApplication"]["operatingSystem"].startswith("macOS")


def test_jsonld_software_application_category(jsonld):
    assert jsonld["SoftwareApplication"]["applicationCategory"] in ("BusinessApplication", "EmailApplication")


def test_jsonld_offer_is_free(jsonld):
    assert jsonld["SoftwareApplication"]["offers"]["price"] == "0"


def test_jsonld_download_url_is_the_dmg(jsonld):
    assert jsonld["SoftwareApplication"]["downloadUrl"] == site_build.DOWNLOAD_URL


def test_jsonld_same_as_github(jsonld):
    assert "https://github.com/ClementWalter/mermoz" in jsonld["SoftwareApplication"]["sameAs"]


def test_jsonld_organization_url(jsonld):
    assert jsonld["Organization"]["url"] == "https://mermoz.xyz/"


def test_jsonld_organization_logo(jsonld):
    assert (SITE / jsonld["Organization"]["logo"].removeprefix("https://mermoz.xyz/")).is_file()


@pytest.mark.parametrize("field", ["address", "telephone"])
def test_jsonld_organization_has_no_invented_contact(jsonld, field):
    assert field not in json.dumps(jsonld["Organization"])


def test_jsonld_version_matches_the_release_script(jsonld):
    assert jsonld["SoftwareApplication"]["softwareVersion"] == site_build.app_version()


# ---------------------------------------------------------------- pages


@pytest.mark.parametrize("url_path", PAGE_PATHS)
def test_page_title_names_mermoz(url_path):
    assert "Mermoz" in parse(page_file(url_path)).title


@pytest.mark.parametrize("url_path", PAGE_PATHS)
def test_page_h1_names_mermoz(url_path):
    assert any("Mermoz" in h for h in parse(page_file(url_path)).h1)


@pytest.mark.parametrize("url_path", PAGE_PATHS)
def test_page_has_a_markdown_twin(url_path):
    twin = SITE / "index.md" if url_path == "/" else SITE / url_path.strip("/") / "index.md"
    assert twin.is_file()


@pytest.mark.parametrize("name", TRUST_PAGES)
def test_trust_page_has_over_500_characters(name):
    text = re.sub(r"\s+", " ", "".join(parse(SITE / name / "index.html").text)).strip()
    assert len(text) > 500


def test_homepage_title():
    assert parse(SITE / "index.html").title.startswith("Mermoz: fast, agent-first email for Mac (Gmail and Outlook)")


def test_homepage_keeps_the_tagline():
    assert "must go through." in (SITE / "index.html").read_text()


def test_homepage_keeps_both_beta_forms():
    assert (SITE / "index.html").read_text().count("data-beta-form") == 2


def test_homepage_keeps_both_download_links():
    assert (SITE / "index.html").read_text().count(f'href="{site_build.DOWNLOAD_URL}" data-download') == 2


def test_homepage_keeps_the_animation_script():
    assert '<script src="main.js" defer></script>' in (SITE / "index.html").read_text()


@pytest.mark.parametrize("target", ["/llms.txt", "/sitemap.xml", "/docs/"])
def test_404_links_to(target):
    assert f'href="{target}"' in (SITE / "404.html").read_text()


def test_404_is_not_indexed():
    assert '<meta name="robots" content="noindex">' in (SITE / "404.html").read_text()


def test_404_uses_absolute_asset_paths():
    assert 'href="/style.css"' in (SITE / "404.html").read_text()


@pytest.mark.parametrize("phrase", ["gmail.modify", "drive.readonly", "drive.file", "TypeSafe", "SubQ", "ChatGPT", "Anthropic", "Google Form"])
def test_privacy_page_covers(phrase):
    assert phrase in (SITE / "privacy" / "index.md").read_text()


def test_privacy_page_flags_legal_review():
    assert "Needs legal review" in (SITE / "privacy" / "index.md").read_text()


def test_protocol_page_has_every_method_of_the_spec():
    spec = set(re.findall(r"^\| `([a-z_.]+)`", (REPO / "docs" / "protocol.md").read_text(), re.M))
    page = (SITE / "docs" / "protocol" / "index.html").read_text()
    assert [m for m in sorted(spec) if f"<code>{m}</code>" not in page] == []


# ---------------------------------------------------------------- MCP discovery


def test_server_json_is_valid_against_the_registry_schema():
    schema = json.loads((FIXTURES / "server.schema.json").read_text())
    jsonschema.validate(json.loads((SITE / "server.json").read_text()), schema)


def test_server_json_uses_the_vendored_schema_version():
    schema = json.loads((FIXTURES / "server.schema.json").read_text())
    assert json.loads((SITE / "server.json").read_text())["$schema"] == schema["$id"]


def test_server_json_has_no_remote_endpoint():
    assert "remotes" not in json.loads((SITE / "server.json").read_text())


def test_well_known_mcp_is_stdio():
    assert json.loads((SITE / ".well-known" / "mcp.json").read_text())["transport"] == "stdio"


def test_well_known_mcp_launch_command():
    install = json.loads((SITE / ".well-known" / "mcp.json").read_text())["install"]
    assert [install["command"], *install["args"]] == ["mermoz", "mcp"]


def test_mcp_snapshot_matches_the_server_source(mcp_snapshot):
    source = (REPO / "crates" / "mermoz-cli" / "src" / "mcp.rs").read_text()
    tools_fn = source.split("fn tools()", 1)[1].split("\nfn ", 1)[0]
    names = re.findall(r'\{ "name": "([a-z_]+)"', tools_fn)
    assert [t["name"] for t in mcp_snapshot["tools"]] == names


def test_mcp_snapshot_protocol_version_matches_the_server_source(mcp_snapshot):
    source = (REPO / "crates" / "mermoz-cli" / "src" / "mcp.rs").read_text()
    assert f'PROTOCOL_VERSION: &str = "{mcp_snapshot["protocolVersion"]}"' in source


def test_cli_snapshot_matches_the_cli_source():
    source = (REPO / "crates" / "mermoz-cli" / "src" / "main.rs").read_text()
    body = source.split("enum Cmd {", 1)[1].split("\n}\n", 1)[0]
    variants = re.findall(r"^    ([A-Z][A-Za-z]+)\b", body, re.M)
    kebab = [re.sub(r"(?<!^)([A-Z])", r"-\1", v).lower() for v in variants]
    snap = json.loads((SITE / "src" / "cli-help.json").read_text())
    assert [c["name"] for c in snap["commands"] if " " not in c["name"]] == kebab
