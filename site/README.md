# mermoz.xyz

The landing page and its agent-facing files: plain HTML, CSS and a little JavaScript.

- `index.html`, `style.css`, `main.js` (the paper-plane sky): hand-written. The
  JSON-LD block in `index.html` is generated (between the `jsonld` markers).
- `assets/`: icon, the launch film (from `marketing/video`), poster and share image.
- `src/`: sources of every other page. `src/pages/*.md` is one Markdown page per URL
  (front matter: path, title, description, updated); `src/cli-help.json` and
  `src/mcp-tools.json` are snapshots of the real CLI and MCP surface.
- Generated and committed (do not edit): `docs/`, `about/`, `contact/`, `privacy/`,
  `404.html`, every `index.md` / `404.md` twin, `sitemap.xml`, `robots.txt`,
  `llms.txt`, `llms-full.txt`, `server.json`, `.well-known/mcp.json`.
  - Rebuild: `uv run scripts/site_build.py build` (also after editing `docs/protocol.md`).
  - After CLI or MCP changes: `cargo build --release -p mermoz-cli`, then
    `uv run scripts/site_build.py snapshot --mermoz target/release/mermoz`, then build.
- `edge/`: a Cloudflare Worker for Markdown content negotiation, not deployed (see
  `docs/release-plan.md`).
- Tests: `uv run --with pytest --with jsonschema --with xmlschema --with markdown --with click pytest site/tests`
  and `node --test site/edge/worker.test.mjs`.
- Preview: `python3 -m http.server 8000 -d site`, then open http://localhost:8000.
- Deploy: pushing to `main` runs `.github/workflows/site.yml`: freshness check, tests,
  then GitHub Pages publishes `site/` without `src/`, `tests/`, `edge/` and this file.
  `CNAME` sets the domain to mermoz.xyz.
