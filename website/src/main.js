// Small progressive enhancements. The site works without this file.
(() => {
  const $ = (s, el = document) => el.querySelector(s);
  const $$ = (s, el = document) => [...el.querySelectorAll(s)];
  const store = {
    get(k) { try { return localStorage.getItem(k); } catch { return null; } },
    set(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode */ } },
  };

  // Theme: follow the system until the visitor picks one.
  const root = document.documentElement;
  const systemDark = () => window.matchMedia("(prefers-color-scheme: dark)").matches;
  $(".theme-toggle")?.addEventListener("click", () => {
    const current = root.dataset.theme || (systemDark() ? "dark" : "light");
    const next = current === "dark" ? "light" : "dark";
    root.dataset.theme = next;
    store.set("orbit-theme", next);
  });

  // Mobile menu.
  const menuBtn = $(".menu-toggle");
  const nav = $(".nav");
  menuBtn?.addEventListener("click", () => {
    const open = nav.classList.toggle("open");
    menuBtn.setAttribute("aria-expanded", String(open));
  });
  $$(".nav a").forEach((a) => a.addEventListener("click", () => { nav.classList.remove("open"); menuBtn?.setAttribute("aria-expanded", "false"); }));

  // Copy buttons: the text comes from the nearest data-copy, else the code block.
  document.addEventListener("click", async (e) => {
    const btn = e.target.closest(".copy");
    if (!btn) return;
    const holder = btn.closest("[data-copy]");
    const text = holder ? holder.dataset.copy : btn.parentElement.querySelector("code")?.innerText ?? "";
    try {
      await navigator.clipboard.writeText(text.trim());
      btn.textContent = "Copied";
      btn.classList.add("done");
    } catch {
      btn.textContent = "Press ⌘C";
    }
    setTimeout(() => { btn.textContent = "Copy"; btn.classList.remove("done"); }, 1600);
  });

  // Install tabs, preselected from the visitor's OS.
  const osTabs = $$(".os-list [role=tab]");
  if (osTabs.length) {
    const select = (tab, focus) => {
      osTabs.forEach((t) => {
        const on = t === tab;
        t.setAttribute("aria-selected", String(on));
        t.tabIndex = on ? 0 : -1;
        document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
      });
      if (focus) tab.focus();
    };
    osTabs.forEach((t, i) => {
      t.addEventListener("click", () => select(t));
      t.addEventListener("keydown", (e) => {
        const d = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[e.key] || 0;
        if (d) { e.preventDefault(); select(osTabs[(i + d + osTabs.length) % osTabs.length], true); }
      });
    });
    const ua = (navigator.userAgentData?.platform || navigator.platform || navigator.userAgent || "").toLowerCase();
    const guess = /win/.test(ua) ? "windows" : /linux|x11|cros/.test(ua) && !/android/.test(ua) ? "linux" : "macos";
    select(osTabs.find((t) => t.dataset.os === guess) || osTabs[0]);
  }

  // Hero board: sessions work, one stops to ask you something, you jump to it with ";", answer,
  // and the rest finish. The tab's counts follow the cards, as they do in Orbit.
  const board = $(".board");
  if (board) {
    const cards = $$(".card", board);
    const counts = $("[data-counts]", board);
    const hint = $(".hint-jump", board);
    const COLOR = { working: "yellow", waiting: "red", unread: "blue", seen: "green" };
    const tally = () => {
      const n = {};
      cards.forEach((c) => { n[c.dataset.state] = (n[c.dataset.state] || 0) + 1; });
      counts.innerHTML = ["waiting", "working", "unread", "seen"].filter((s) => n[s])
        .map((s) => `<b class="c-${COLOR[s]}">●${n[s]}</b>`).join("");
    };
    tally();
    const initial = cards.map((c) => c.dataset.state);
    const set = (i, state) => { cards[i].dataset.state = state; tally(); };
    const sel = (i) => cards.forEach((c, j) => c.classList.toggle("sel", i === j));
    const script = [
      () => set(1, "waiting"),
      () => hint.classList.add("lit"),
      () => { sel(1); hint.classList.remove("lit"); },
      () => set(1, "working"),
      () => set(3, "unread"),
      () => set(0, "unread"),
      () => { sel(0); set(0, "seen"); },
      () => set(1, "unread"),
      () => set(2, "working"),
      () => { initial.forEach((s, i) => { cards[i].dataset.state = s; }); sel(0); tally(); },
    ];
    const still = window.matchMedia("(prefers-reduced-motion: reduce)");
    let step = 0, timer = 0, onScreen = false;
    const tick = () => { script[step](); step = (step + 1) % script.length; };
    const run = () => {
      clearInterval(timer);
      if (onScreen && !document.hidden && !still.matches) timer = setInterval(tick, 1700);
    };
    if ("IntersectionObserver" in window) {
      new IntersectionObserver(([en]) => { onScreen = en.isIntersecting; run(); }).observe(board);
    }
    document.addEventListener("visibilitychange", run);
    still.addEventListener?.("change", run);
  }

  // Home page: the nav item for the section on screen is marked.
  const sectionLinks = $$(".nav a[data-section]");
  if (document.body.classList.contains("home") && sectionLinks.length && "IntersectionObserver" in window) {
    const navFor = { features: "features", tour: "features", install: "install", quickstart: "install", author: "author" };
    const io = new IntersectionObserver((entries) => {
      entries.forEach((en) => {
        if (!en.isIntersecting) return;
        const key = navFor[en.target.id];
        sectionLinks.forEach((a) => a.classList.toggle("is-active", a.dataset.section === key));
      });
    }, { rootMargin: "-45% 0px -50% 0px" });
    $$("main > section").forEach((el) => io.observe(el));
  }

  // Screenshot gallery.
  const gTabs = $$(".gallery-tabs [role=tab]");
  const gImg = $("#gallery-img");
  const gCap = $(".gallery-caption");
  gTabs.forEach((t) => t.addEventListener("click", () => {
    gTabs.forEach((x) => x.setAttribute("aria-selected", String(x === t)));
    gImg.style.opacity = "0.25";
    const next = new Image();
    next.src = `/assets/${t.dataset.shot}.png`;
    next.onload = () => {
      gImg.src = next.src;
      gImg.alt = t.querySelector("b").textContent + ": " + t.querySelector("span").textContent;
      gImg.style.opacity = "1";
    };
    gCap.textContent = t.querySelector("b").textContent;
  }));

  // Docs: mobile sidebar, heading anchors, and the "on this page" highlight.
  const sideBtn = $(".side-toggle");
  sideBtn?.addEventListener("click", () => {
    const open = $(".side-nav").classList.toggle("open");
    sideBtn.setAttribute("aria-expanded", String(open));
  });
  $$(".prose :is(h2, h3, h4)[id]").forEach((h) => {
    const a = document.createElement("a");
    a.className = "anchor";
    a.href = `#${h.id}`;
    a.setAttribute("aria-label", "Link to this section");
    a.textContent = "#";
    h.prepend(a);
  });
  const tocLinks = $$(".toc a");
  if (tocLinks.length && "IntersectionObserver" in window) {
    const byId = new Map(tocLinks.map((a) => [decodeURIComponent(a.hash.slice(1)), a]));
    const visible = new Set();
    const io = new IntersectionObserver((entries) => {
      entries.forEach((en) => (en.isIntersecting ? visible.add(en.target.id) : visible.delete(en.target.id)));
      const first = [...byId.keys()].find((id) => visible.has(id));
      if (first) tocLinks.forEach((a) => a.classList.toggle("active", a === byId.get(first)));
    }, { rootMargin: "-70px 0px -65% 0px" });
    byId.forEach((_, id) => { const el = document.getElementById(id); if (el) io.observe(el); });
  }
})();
