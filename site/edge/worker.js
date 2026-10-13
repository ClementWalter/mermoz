/**
 * Markdown content negotiation in front of the GitHub Pages site mermoz.xyz.
 *
 * GitHub Pages cannot vary a response on the Accept header. This Worker proxies every
 * request to the origin unchanged, except page requests whose Accept header prefers
 * text/markdown: those get the page's pre-generated Markdown twin (`/about/` →
 * `/about/index.md`, built by scripts/site_build.py), or `/404.md` with status 404 when
 * the page does not exist. Every negotiated response carries `Vary: Accept` so caches
 * keep the HTML and Markdown versions apart.
 *
 * `env.ORIGIN` (optional) points the proxy at another origin, e.g. a local preview;
 * without it the request goes to the route's own origin (GitHub Pages behind Cloudflare).
 */

const MARKDOWN = "text/markdown";

/** Parses an Accept header into [{type, q}] ranges. */
export function parseAccept(header) {
  return (header || "")
    .split(",")
    .map((part) => {
      const [type, ...params] = part.trim().toLowerCase().split(";");
      let q = 1;
      for (const p of params) {
        const [k, v] = p.trim().split("=");
        if (k === "q") q = Number.parseFloat(v);
      }
      return { type: type.trim(), q: Number.isFinite(q) ? q : 0 };
    })
    .filter((r) => r.type);
}

/** The q value the Accept ranges give a media type (most specific range wins). */
function qualityOf(ranges, type) {
  const [major] = type.split("/");
  const exact = ranges.find((r) => r.type === type);
  if (exact) return exact.q;
  const partial = ranges.find((r) => r.type === `${major}/*`);
  if (partial) return partial.q;
  const any = ranges.find((r) => r.type === "*/*");
  return any ? any.q : 0;
}

/** True when the client asks for Markdown explicitly and rates it above HTML. */
export function prefersMarkdown(header) {
  const ranges = parseAccept(header);
  // A bare */* (curl's default, most bots) keeps the HTML page.
  if (!ranges.some((r) => r.type === MARKDOWN || r.type === "text/x-markdown")) return false;
  const md = Math.max(qualityOf(ranges, MARKDOWN), qualityOf(ranges, "text/x-markdown"));
  return md > 0 && md >= qualityOf(ranges, "text/html");
}

/**
 * The Markdown twin of a page path, or null for files (assets, feeds, .txt, .xml...).
 * `/` → `/index.md`, `/docs/` and `/docs` → `/docs/index.md`, `/404.html` → `/404.md`.
 */
export function markdownPath(pathname) {
  if (pathname.endsWith("/")) return `${pathname}index.md`;
  const last = pathname.slice(pathname.lastIndexOf("/") + 1);
  if (last.endsWith(".html")) return `${pathname.slice(0, -".html".length)}.md`;
  if (last.includes(".")) return null;
  return `${pathname}/index.md`;
}

/** Adds Accept to the response's Vary header without dropping what is there. */
function withVary(headers) {
  const vary = headers.get("Vary");
  if (!vary) headers.set("Vary", "Accept");
  else if (!vary.toLowerCase().split(",").map((v) => v.trim()).includes("accept")) headers.set("Vary", `${vary}, Accept`);
  return headers;
}

function originUrl(request, env, pathname) {
  const url = new URL(request.url);
  if (pathname) url.pathname = pathname;
  if (env && env.ORIGIN) {
    const origin = new URL(env.ORIGIN);
    url.protocol = origin.protocol;
    url.host = origin.host;
  }
  return url.toString();
}

async function markdownResponse(request, env, mdPath) {
  const init = { method: "GET", headers: { Accept: MARKDOWN } };
  let res = await fetch(originUrl(request, env, mdPath), init);
  let status = 200;
  if (!res.ok) {
    status = 404;
    res = await fetch(originUrl(request, env, "/404.md"), init);
    if (!res.ok) res = new Response("# Not found\n\nSee https://mermoz.xyz/llms.txt\n");
  }
  const headers = withVary(new Headers());
  headers.set("Content-Type", "text/markdown; charset=utf-8");
  headers.set("Cache-Control", "public, max-age=600");
  headers.set("X-Content-Type-Options", "nosniff");
  headers.set("Link", `<${new URL(request.url).pathname}>; rel="alternate"; type="text/html"`);
  const body = request.method === "HEAD" ? null : await res.text();
  return new Response(body, { status, headers });
}

export default {
  async fetch(request, env) {
    const { pathname } = new URL(request.url);
    const mdPath = request.method === "GET" || request.method === "HEAD" ? markdownPath(pathname) : null;

    if (mdPath && prefersMarkdown(request.headers.get("Accept"))) {
      return markdownResponse(request, env, mdPath);
    }

    const upstream = await fetch(new Request(originUrl(request, env), request));
    if (!mdPath) return upstream;
    // Pages have two representations, so their HTML is negotiated too.
    const res = new Response(upstream.body, upstream);
    withVary(res.headers);
    return res;
  },
};
