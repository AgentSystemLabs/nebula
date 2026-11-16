// Builds the Orbit website into ./dist.
//
//   node build.mjs
//
// The landing page is src/index.html. Every docs page is rendered from the
// repository's own Markdown (docs/*.md, ARCHITECTURE.md, CONTRIBUTING.md), so
// the site and GitHub never disagree. Links between those files are rewritten
// to site URLs; links to anything else in the repo point at GitHub.

import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, normalize, posix, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { Marked } from "marked";
import { gfmHeadingId, getHeadingList } from "marked-gfm-heading-id";
import { markedHighlight } from "marked-highlight";
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import json from "highlight.js/lib/languages/json";
import rust from "highlight.js/lib/languages/rust";
import ini from "highlight.js/lib/languages/ini";

hljs.registerLanguage("bash", bash);
hljs.registerLanguage("sh", bash);
hljs.registerLanguage("shell", bash);
hljs.registerLanguage("json", json);
hljs.registerLanguage("jsonc", json);
hljs.registerLanguage("rust", rust);
hljs.registerLanguage("toml", ini);

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const SRC = join(HERE, "src");
const OUT = join(HERE, "dist");
const REPO = "https://github.com/AmenAdem/Orbit";
const SITE = "https://orbit-tui.pages.dev";

const version = readFileSync(join(ROOT, "Cargo.toml"), "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1] ?? "";

// Sidebar order. `file` is relative to the repo root.
const SECTIONS = [
  {
    title: "Start here",
    pages: [
      { slug: "guide", file: "docs/guide.md", label: "User guide", blurb: "The everyday loop, worktrees, reviewing, GitHub, troubleshooting." },
      { slug: "keys", file: "docs/keys.md", label: "Keys", blurb: "Every default binding, and the mouse." },
      { slug: "commands", file: "docs/commands.md", label: "Commands", blurb: "The orbit CLI and every subcommand." },
    ],
  },
  {
    title: "In depth",
    pages: [
      { slug: "sessions", file: "docs/sessions.md", label: "Sessions", blurb: "The task box, presets, follow-ups, issues and pull requests." },
      { slug: "orchestrator", file: "docs/orchestrator.md", label: "Orchestrator", blurb: "F10: one Claude chat that coordinates the rest." },
      { slug: "configuration", file: "docs/configuration.md", label: "Configuration", blurb: "config.json, the project file, hooks, environment variables." },
    ],
  },
  {
    title: "Internals",
    pages: [
      { slug: "how-it-works", file: "docs/how-it-works.md", label: "How it works", blurb: "The daemon, the hooks, persistence." },
      { slug: "architecture", file: "ARCHITECTURE.md", label: "Architecture", blurb: "Process model, crate layout, adding a modal." },
      { slug: "contributing", file: "CONTRIBUTING.md", label: "Contributing", blurb: "Build it, test it, send a pull request." },
    ],
  },
];
const PAGES = SECTIONS.flatMap((s) => s.pages);
const bySourcePath = new Map(PAGES.map((p) => [p.file, p]));

const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const stripTags = (s) => s.replace(/<[^>]+>/g, "");

// Map a link found in `fromFile` (repo-relative) to a site or GitHub URL.
function rewriteHref(href, fromFile) {
  if (!href || /^(?:[a-z]+:|#|\/)/i.test(href)) return href;
  const [path, hash] = href.split("#");
  const anchor = hash ? `#${hash}` : "";
  if (!path) return href;
  const target = posix.normalize(posix.join(posix.dirname(fromFile), path));
  if (target === "README.md") return `/${anchor}`;
  const page = bySourcePath.get(target);
  if (page) return `/docs/${page.slug}/${anchor}`;
  if (target.startsWith("assets/")) return `/${target}`;
  if (target.startsWith("..")) return href;
  const abs = join(ROOT, target);
  const kind = existsSync(abs) && statSync(abs).isDirectory() ? "tree" : "blob";
  return `${REPO}/${kind}/main/${target}${anchor}`;
}

const DOTS = { "🟡": "yellow", "🔴": "red", "🔵": "blue", "🟢": "green", "⚪": "gray", "🟣": "magenta" };
const dotify = (html) => html.replace(/🟡|🔴|🔵|🟢|⚪|🟣/gu, (m) => `<span class="dot dot-${DOTS[m]}" role="img" aria-label="${DOTS[m]} dot"></span>`);

function renderMarkdown(source, fromFile) {
  const marked = new Marked(
    markedHighlight({
      emptyLangClass: "hljs",
      langPrefix: "hljs language-",
      highlight(code, lang) {
        const language = hljs.getLanguage(lang) ? lang : null;
        return language ? hljs.highlight(code, { language }).value : esc(code);
      },
    }),
    gfmHeadingId(),
    {
      gfm: true,
      walkTokens(token) {
        if (token.type === "link" || token.type === "image") token.href = rewriteHref(token.href, fromFile);
      },
    },
  );
  let html = marked.parse(source);
  // Raw HTML in the Markdown (<img src>, <a href>) gets the same rewriting.
  html = html.replace(/\b(src|href)="([^"]+)"/g, (m, attr, url) => `${attr}="${esc(rewriteHref(url.replace(/&amp;/g, "&"), fromFile))}"`);
  // Tables get a frame; their cells wrap to the column (see .table-wrap in styles.css).
  html = html.replace(/<table>/g, '<div class="table-wrap"><table>').replace(/<\/table>/g, "</table></div>");
  html = html.replace(/<pre><code/g, '<pre class="code"><button class="copy" type="button" aria-label="Copy code">Copy</button><code');
  return { html: dotify(html), headings: getHeadingList() };
}

function prepareSource(raw) {
  let md = raw.replace(/\r\n/g, "\n");
  // The GitHub-only nav line under each title; the site has a sidebar instead.
  md = md.replace(/^<sub>.*<\/sub>\s*$/m, "");
  // GitHub comment markers such as <!-- rebrand:keep -->.
  md = md.replace(/<!--[\s\S]*?-->/g, "");
  const title = md.match(/^#\s+(.+)$/m)?.[1]?.trim() ?? "Orbit";
  return { md, title };
}

const partial = (name) => readFileSync(join(SRC, "partials", name), "utf8");
const fill = (tpl, vars) => tpl.replace(/\{\{(\w+)\}\}/g, (_, k) => vars[k] ?? "");

function sidebar(activeSlug) {
  return SECTIONS.map(
    (s) =>
      `<div class="side-group"><p class="side-title">${esc(s.title)}</p><ul>${s.pages
        .map((p) => `<li><a href="/docs/${p.slug}/"${p.slug === activeSlug ? ' aria-current="page"' : ""}>${esc(p.label)}</a></li>`)
        .join("")}</ul></div>`,
  ).join("");
}

function toc(headings) {
  const items = headings.filter((h) => h.level === 2 || h.level === 3);
  if (items.length < 2) return "";
  return `<nav class="toc" aria-label="On this page"><p class="side-title">On this page</p><ul>${items
    .map((h) => `<li class="toc-l${h.level}"><a href="#${h.id}">${dotify(stripTags(h.text))}</a></li>`)
    .join("")}</ul></nav>`;
}

function pager(i) {
  const prev = PAGES[i - 1];
  const next = PAGES[i + 1];
  const link = (p, dir) =>
    p ? `<a class="pager-${dir}" href="/docs/${p.slug}/"><span>${dir === "prev" ? "← Previous" : "Next →"}</span>${esc(p.label)}</a>` : "<span></span>";
  return `<nav class="pager" aria-label="Pages">${link(prev, "prev")}${link(next, "next")}</nav>`;
}

function build() {
  rmSync(OUT, { recursive: true, force: true });
  mkdirSync(OUT, { recursive: true });

  // Static files: everything in src except the templates.
  cpSync(SRC, OUT, { recursive: true, filter: (p) => !relative(SRC, p).startsWith("partials") });
  cpSync(join(ROOT, "assets"), join(OUT, "assets"), { recursive: true });

  const head = partial("head.html");
  const header = partial("header.html");
  const footer = partial("footer.html");
  // A content hash on the stylesheet and script URLs, so a deploy never pairs new pages with a cached old file.
  const hash = (f) => createHash("sha256").update(readFileSync(join(SRC, f))).digest("hex").slice(0, 10);
  const shared = { version, repo: REPO, site: SITE, logo: partial("logo.svg").trim(), cssV: hash("styles.css"), jsV: hash("main.js") };

  // Landing page: the shared header/footer are spliced in.
  const landing = readFileSync(join(SRC, "index.html"), "utf8");
  writeFileSync(
    join(OUT, "index.html"),
    fill(landing, {
      ...shared,
      head: fill(head, { ...shared, title: "Orbit — mission control for your Claude Code sessions", description: "Run many Claude Code sessions across every project and git worktree from one terminal. A status dot tells you which one needs you. Free, open source, for macOS and Linux.", path: "/" }),
      header: fill(header, { ...shared, docsCurrent: "", contribCurrent: "" }),
      footer: fill(footer, shared),
      docCards: PAGES.map(
        (p) => `<a class="doc-card" href="/docs/${p.slug}/"><span class="doc-card-title">${esc(p.label)}</span><span class="doc-card-blurb">${esc(p.blurb)}</span></a>`,
      ).join(""),
    }),
  );

  const docTpl = partial("doc.html");
  PAGES.forEach((page, i) => {
    const { md, title } = prepareSource(readFileSync(join(ROOT, page.file), "utf8"));
    const { html, headings } = renderMarkdown(md, page.file);
    const dir = join(OUT, "docs", page.slug);
    mkdirSync(dir, { recursive: true });
    writeFileSync(
      join(dir, "index.html"),
      fill(docTpl, {
        ...shared,
        head: fill(head, { ...shared, title: `${stripTags(title)} — Orbit docs`, description: page.blurb, path: `/docs/${page.slug}/` }),
        // Contributing lives under /docs/ but has its own entry in the top nav.
        header: fill(header, {
          ...shared,
          docsCurrent: page.slug === "contributing" ? "" : ' aria-current="page"',
          contribCurrent: page.slug === "contributing" ? ' aria-current="page"' : "",
        }),
        footer: fill(footer, shared),
        sidebar: sidebar(page.slug),
        toc: toc(headings),
        content: html,
        pager: pager(i),
        editUrl: `${REPO}/edit/main/${page.file}`,
        sourceFile: page.file,
      }),
    );
  });

  // /docs/ itself lands on the guide.
  writeFileSync(join(OUT, "_redirects"), "/docs /docs/guide/ 302\n/docs/ /docs/guide/ 302\n");

  const urls = ["/", ...PAGES.map((p) => `/docs/${p.slug}/`)];
  writeFileSync(
    join(OUT, "sitemap.xml"),
    `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls.map((u) => `  <url><loc>${SITE}${u}</loc></url>`).join("\n")}\n</urlset>\n`,
  );
  writeFileSync(join(OUT, "robots.txt"), `User-agent: *\nAllow: /\nSitemap: ${SITE}/sitemap.xml\n`);

  console.log(`built ${PAGES.length + 1} pages for v${version} → ${normalize(relative(process.cwd(), OUT)) || "."}`);
}

build();
