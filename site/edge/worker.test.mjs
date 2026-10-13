/**
 * Unit tests of the edge Worker, run with `node --test site/edge`. The origin is a stub
 * fetch serving files from site/ (404 for anything missing), as GitHub Pages would.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import worker, { markdownPath, prefersMarkdown } from "./worker.js";

const SITE = fileURLToPath(new URL("..", import.meta.url));

globalThis.fetch = async (input) => {
  const url = new URL(typeof input === "string" ? input : input.url);
  const path = url.pathname.endsWith("/") ? `${url.pathname}index.html` : url.pathname;
  try {
    const body = await readFile(SITE + path.slice(1));
    const type = path.endsWith(".md") ? "text/markdown" : path.endsWith(".html") ? "text/html" : "application/octet-stream";
    return new Response(body, { status: 200, headers: { "Content-Type": type } });
  } catch {
    return new Response("not found", { status: 404, headers: { "Content-Type": "text/html" } });
  }
};

const get = (path, accept) => worker.fetch(new Request(`https://mermoz.xyz${path}`, { headers: accept ? { Accept: accept } : {} }), {});

test("homepage path maps to index.md", () => assert.equal(markdownPath("/"), "/index.md"));
test("slashless page maps to its index.md", () => assert.equal(markdownPath("/docs"), "/docs/index.md"));
test("404.html maps to 404.md", () => assert.equal(markdownPath("/404.html"), "/404.md"));
test("assets have no markdown twin", () => assert.equal(markdownPath("/assets/icon.png"), null));
test("llms.txt has no markdown twin", () => assert.equal(markdownPath("/llms.txt"), null));

test("bare text/markdown prefers markdown", () => assert.equal(prefersMarkdown("text/markdown"), true));
test("text/html does not prefer markdown", () => assert.equal(prefersMarkdown("text/html"), false));
test("*/* does not prefer markdown", () => assert.equal(prefersMarkdown("*/*"), false));
test("browser Accept does not prefer markdown", () =>
  assert.equal(prefersMarkdown("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"), false));
test("markdown rated above html prefers markdown", () =>
  assert.equal(prefersMarkdown("text/html;q=0.5, text/markdown"), true));
test("markdown rated below html keeps html", () => assert.equal(prefersMarkdown("text/markdown;q=0.3, text/html"), false));

test("markdown request gets text/markdown", async () => {
  const res = await get("/", "text/markdown");
  assert.equal(res.headers.get("Content-Type"), "text/markdown; charset=utf-8");
});
test("markdown request gets Vary: Accept", async () => {
  const res = await get("/", "text/markdown");
  assert.equal(res.headers.get("Vary"), "Accept");
});
test("markdown homepage body is the markdown twin", async () => {
  const res = await get("/", "text/markdown");
  assert.match(await res.text(), /^<!-- https:\/\/mermoz\.xyz\/ /);
});
test("markdown docs page answers 200", async () => {
  const res = await get("/docs/mcp/", "text/markdown");
  assert.equal(res.status, 200);
});
test("missing page in markdown answers 404", async () => {
  const res = await get("/nope", "text/markdown");
  assert.equal(res.status, 404);
});
test("missing page in markdown has a markdown body", async () => {
  const res = await get("/nope", "text/markdown");
  assert.match(await res.text(), /# Lost in the clouds/);
});
test("html request gets html", async () => {
  const res = await get("/", "text/html");
  assert.equal(res.headers.get("Content-Type"), "text/html");
});
test("html page request gets Vary: Accept", async () => {
  const res = await get("/", "text/html");
  assert.equal(res.headers.get("Vary"), "Accept");
});
test("assets pass through without Vary", async () => {
  const res = await get("/assets/favicon.png", "text/markdown");
  assert.equal(res.headers.get("Vary"), null);
});
