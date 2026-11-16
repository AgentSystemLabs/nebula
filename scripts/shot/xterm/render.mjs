#!/usr/bin/env node
// Renders `make shot` captures (design-screenshots/<scene>.ansi) the way a real terminal draws them:
// the bytes go through xterm.js, the emulator inside VS Code, in headless Chrome at 2x device pixels,
// with JetBrains Mono (its box-drawing glyphs join at line height 1). render.py draws each glyph
// itself with Pillow at 1x and stays the dependency-light fallback the harness uses on its own.
// (xterm's WebGL renderer draws only part of the screen under headless software GL, so the DOM
// renderer is used.)
//
//   node render.mjs in.ansi out.png [in2.ansi out2.png …] [--cols 190] [--rows 50] [--size 14] [--scale 2]
//
// Needs Google Chrome (CHROME=/path/to/chrome to point elsewhere) and `npm install` in this folder.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";

const HERE = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const opt = { cols: 190, rows: 50, size: 14, scale: 2, pad: 22, wait: 300, line: 1.15, font: process.env.SHOT_FONT || "'JetBrains Mono', monospace" };
const pairs = [];
for (let i = 0; i < args.length; i++) {
  if (args[i].startsWith("--")) opt[args[i].slice(2)] = Number(args[++i]);
  else pairs.push(args[i]);
}
if (!pairs.length || pairs.length % 2) {
  console.error("usage: node render.mjs in.ansi out.png [in2.ansi out2.png …] [--cols 190] [--rows 50] [--size 14] [--scale 2]");
  process.exit(2);
}

// The 16 colours Orbit's palette indices land on, matched to the website's status dots, so a red
// dot in a screenshot is the same red as on the page around it. 16–255 stay xterm's standard cube.
const THEME = {
  background: "#000000",
  foreground: "#d4d4d8",
  cursor: "#000000",
  black: "#15161e", red: "#f87171", green: "#4ade80", yellow: "#facc15",
  blue: "#60a5fa", magenta: "#c084fc", cyan: "#22d3ee", white: "#d4d4d8",
  brightBlack: "#6b7280", brightRed: "#fca5a5", brightGreen: "#86efac", brightYellow: "#fde047",
  brightBlue: "#93c5fd", brightMagenta: "#d8b4fe", brightCyan: "#67e8f9", brightWhite: "#fafafa",
};

const lib = (p) => readFileSync(join(HERE, "node_modules", p), "utf8");
const page_html = `<!doctype html><html><head><meta charset="utf-8">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;700&display=block">
<style>${lib("@xterm/xterm/css/xterm.css")}
html,body{margin:0;background:${THEME.background}}
#frame{display:inline-block;padding:${opt.pad}px;background:${THEME.background}}
.xterm-viewport{overflow:hidden!important}
</style></head><body><div id="frame"><div id="term"></div></div>
<script>${lib("@xterm/xterm/lib/xterm.js")}</script>
</body></html>`;

const chrome = process.env.CHROME || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const browser = await puppeteer.launch({
  executablePath: chrome,
  headless: true,
  args: ["--hide-scrollbars"],
});

try {
  const page = await browser.newPage();
  await page.setViewport({ width: 3000, height: 1600, deviceScaleFactor: opt.scale });
  await page.setContent(page_html, { waitUntil: "networkidle0" });
  await page.evaluate(() => Promise.all([document.fonts.load("14px 'JetBrains Mono'"), document.fonts.load("bold 14px 'JetBrains Mono'")]));

  for (let i = 0; i < pairs.length; i += 2) {
    const [input, output] = [pairs[i], pairs[i + 1]];
    // tmux writes one screen row per line; a terminal wants CR LF between rows and nothing after
    // the last, or the screen scrolls by one.
    const rows = readFileSync(input, "utf8").replace(/\r/g, "").split("\n").slice(0, opt.rows);
    const data = "\x1b[?25l" + rows.join("\x1b[0m\r\n");
    await page.evaluate(
      async ({ data, opt, theme }) => {
        document.getElementById("term").innerHTML = "";
        window.__term?.dispose();
        const term = new Terminal({
          cols: opt.cols, rows: opt.rows, fontFamily: opt.font, fontSize: opt.size,
          lineHeight: opt.line, letterSpacing: 0, theme, allowTransparency: false, cursorBlink: false,
          customGlyphs: true, drawBoldTextInBrightColors: false, minimumContrastRatio: 1, scrollback: 0,
        });
        window.__term = term;
        term.open(document.getElementById("term"));
        await new Promise((r) => term.write(data, r));
        term.refresh(0, opt.rows - 1);
        await new Promise((r) => setTimeout(r, opt.wait));
        await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
        // The DOM renderer draws a box-drawing glyph at its natural height, which leaves a hairline
        // between rows once the line height is above 1. Stretch each one (U+2500–U+259F) to the row
        // so borders and blocks join, as a native terminal does.
        const rowsEl = document.querySelector(".xterm-rows");
        const rowH = rowsEl.firstElementChild.getBoundingClientRect().height;
        // The ink of "│" (ascender to descender), measured from the font itself.
        const ctx = document.createElement("canvas").getContext("2d");
        ctx.font = `${opt.size}px ${opt.font}`;
        const mt = ctx.measureText("│");
        const ink = mt.actualBoundingBoxAscent + mt.actualBoundingBoxDescent;
        const k = Math.max(1, (rowH + 0.6) / ink).toFixed(4);
        const box = /[─-▟]+/g;
        const walker = document.createTreeWalker(rowsEl, NodeFilter.SHOW_TEXT);
        const nodes = [];
        while (walker.nextNode()) if (/[─-▟]/.test(walker.currentNode.data)) nodes.push(walker.currentNode);
        for (const node of nodes) {
          const frag = document.createDocumentFragment();
          let last = 0;
          for (const m of node.data.matchAll(box)) {
            if (m.index > last) frag.append(node.data.slice(last, m.index));
            const s = document.createElement("span");
            s.textContent = m[0];
            s.style.cssText = `display:inline-block;height:${rowH}px;line-height:${rowH}px;vertical-align:top;transform:scaleY(${k});transform-origin:50% 50%`;
            frag.append(s);
            last = m.index + m[0].length;
          }
          if (last < node.data.length) frag.append(node.data.slice(last));
          node.replaceWith(frag);
        }
        return "ok";
      },
      { data, opt, theme: THEME },
    );
    const frame = await page.$("#frame");
    writeFileSync(output, await frame.screenshot({ type: "png" }));
    const box = await frame.boundingBox();
    console.log(`${output}  ${Math.round(box.width * opt.scale)}x${Math.round(box.height * opt.scale)}`);
  }
} finally {
  await browser.close();
}
