(() => {
  "use strict";

  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const finePointer = window.matchMedia("(hover: hover) and (pointer: fine)").matches;
  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

  /** Wraps each character (or word) of a [data-split] element in animatable spans while keeping nested markup. */
  function splitText(el) {
    const mode = el.dataset.split === "words" ? "words" : "chars";
    let i = 0;
    const walk = (node) => {
      if (node.nodeType === Node.TEXT_NODE) {
        const text = node.nodeValue;
        if (!text.trim()) return;
        const frag = document.createDocumentFragment();
        if (mode === "words") {
          text.split(/(\s+)/).forEach((part) => {
            if (!part) return;
            if (/^\s+$/.test(part)) return;
            const w = document.createElement("span");
            w.className = "wd";
            w.style.setProperty("--i", i++);
            w.textContent = part;
            frag.appendChild(w);
          });
        } else {
          for (const ch of Array.from(text)) {
            const s = document.createElement("span");
            s.className = ch === " " ? "ch space" : "ch";
            s.style.setProperty("--i", i++);
            s.textContent = ch === " " ? "\u00a0" : ch;
            frag.appendChild(s);
          }
        }
        node.parentNode.replaceChild(frag, node);
      } else if (node.nodeType === Node.ELEMENT_NODE && !node.classList.contains("ch") && !node.classList.contains("wd")) {
        Array.from(node.childNodes).forEach(walk);
      }
    };
    Array.from(el.childNodes).forEach(walk);
    if (mode === "words") {
      const words = $$(".wd", el);
      words.forEach((w, idx) => {
        const next = words[idx + 1];
        if (next && w.parentNode !== next.parentNode) w.style.marginRight = "0";
      });
    }
    el.style.setProperty("--stagger", mode === "words" ? "55ms" : el.style.getPropertyValue("--stagger") || "26ms");
  }

  /** Reveals elements on scroll with IntersectionObserver; adds `is-in` once. */
  function setupReveals() {
    const targets = $$(".reveal, [data-split], .mock, .footer-giant");
    targets.forEach((el) => {
      if (el.dataset.delay) el.style.setProperty("--d", `${el.dataset.delay}ms`);
    });
    if (reduced || !("IntersectionObserver" in window)) {
      targets.forEach((el) => el.classList.add("is-in"));
      return;
    }
    const io = new IntersectionObserver(
      (entries) => {
        entries.forEach((e) => {
          if (!e.isIntersecting) return;
          e.target.classList.add("is-in");
          io.unobserve(e.target);
        });
      },
      { rootMargin: "0px 0px -10% 0px", threshold: 0.12 },
    );
    targets.forEach((el) => io.observe(el));
  }

  /** Hides the intro curtain after its CSS timeline and unlocks scrolling. */
  function setupIntro() {
    const intro = $(".intro");
    if (!intro) return;
    if (reduced || sessionStorage.getItem("rustic-intro-seen")) {
      intro.classList.add("done");
      return;
    }
    document.body.classList.add("locked");
    const finish = () => {
      intro.classList.add("done");
      document.body.classList.remove("locked");
      sessionStorage.setItem("rustic-intro-seen", "1");
    };
    intro.addEventListener("animationend", (e) => {
      if (e.target === intro) finish();
    });
    setTimeout(finish, 2800);
  }

  /** Sticky nav: adds a blurred background after scrolling and hides on fast downward scroll. */
  function setupNav() {
    const nav = $("#nav");
    if (!nav) return;
    let last = window.scrollY;
    const onScroll = () => {
      const y = window.scrollY;
      nav.classList.toggle("scrolled", y > 24);
      if (y > 320 && y - last > 6) nav.classList.add("hidden");
      else if (last - y > 4 || y < 320) nav.classList.remove("hidden");
      last = y;
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    onScroll();
  }

  /** Scroll progress bar driven by document scroll ratio. */
  function setupProgress() {
    const bar = $(".progress span");
    if (!bar) return;
    let raf = 0;
    const update = () => {
      raf = 0;
      const max = document.documentElement.scrollHeight - window.innerHeight;
      const p = max > 0 ? window.scrollY / max : 0;
      bar.style.transform = `scaleX(${Math.min(1, Math.max(0, p))})`;
    };
    window.addEventListener("scroll", () => { if (!raf) raf = requestAnimationFrame(update); }, { passive: true });
    update();
  }

  /** Custom cursor: dot follows instantly, ring eases; grows over interactive elements. */
  function setupCursor() {
    if (!finePointer || reduced) return;
    const cursor = $(".cursor");
    if (!cursor) return;
    document.body.classList.add("has-cursor");
    const dot = $(".cursor-dot", cursor);
    const ring = $(".cursor-ring", cursor);
    let tx = window.innerWidth / 2, ty = window.innerHeight / 2, rx = tx, ry = ty;
    let visible = false;
    window.addEventListener("mousemove", (e) => {
      tx = e.clientX; ty = e.clientY;
      if (!visible) { visible = true; cursor.style.opacity = "1"; rx = tx; ry = ty; }
    }, { passive: true });
    document.addEventListener("mouseleave", () => { cursor.style.opacity = "0"; visible = false; });
    window.addEventListener("mousedown", () => cursor.classList.add("is-down"));
    window.addEventListener("mouseup", () => cursor.classList.remove("is-down"));
    const hoverSel = "a, button, [data-magnetic], .card, .dl, .step, .logo-pill, .chips li";
    document.addEventListener("mouseover", (e) => {
      cursor.classList.toggle("is-hover", Boolean(e.target.closest(hoverSel)));
    });
    const loop = () => {
      rx += (tx - rx) * 0.16;
      ry += (ty - ry) * 0.16;
      dot.style.transform = `translate(${tx}px, ${ty}px) translate(-50%, -50%)`;
      ring.style.transform = `translate(${rx}px, ${ry}px) translate(-50%, -50%)`;
      requestAnimationFrame(loop);
    };
    cursor.style.opacity = "0";
    cursor.style.transition = "opacity 0.3s";
    loop();
  }

  /** Magnetic pull for [data-magnetic] elements: eases toward the pointer while hovered. */
  function setupMagnetic() {
    if (!finePointer || reduced) return;
    $$("[data-magnetic]").forEach((el) => {
      const strength = el.classList.contains("btn-lg") ? 0.35 : 0.25;
      let raf = 0, cx = 0, cy = 0, x = 0, y = 0, active = false;
      const tick = () => {
        x += (cx - x) * 0.18;
        y += (cy - y) * 0.18;
        el.style.transform = `translate(${x.toFixed(2)}px, ${y.toFixed(2)}px)`;
        if (active || Math.abs(x) > 0.1 || Math.abs(y) > 0.1) raf = requestAnimationFrame(tick);
        else { raf = 0; el.style.transform = ""; }
      };
      el.addEventListener("mouseenter", () => { active = true; if (!raf) raf = requestAnimationFrame(tick); });
      el.addEventListener("mousemove", (e) => {
        const r = el.getBoundingClientRect();
        cx = (e.clientX - (r.left + r.width / 2)) * strength;
        cy = (e.clientY - (r.top + r.height / 2)) * strength;
      });
      el.addEventListener("mouseleave", () => { active = false; cx = 0; cy = 0; if (!raf) raf = requestAnimationFrame(tick); });
    });
  }

  /** Tracks pointer position into --mx/--my for spotlight cards, and applies a light 3D tilt to [data-tilt]. */
  function setupSpotAndTilt() {
    if (!finePointer) return;
    $$("[data-spot], [data-tilt]").forEach((el) => {
      const tilt = el.hasAttribute("data-tilt");
      const light = el.dataset.tilt === "light";
      const max = light ? 2.5 : 6;
      let raf = 0, px = 0.5, py = 0.5, inside = false;
      const apply = () => {
        raf = 0;
        if (tilt && !reduced) {
          const rx = inside ? (0.5 - py) * max : 0;
          const ry = inside ? (px - 0.5) * max : 0;
          el.style.transform = `perspective(1000px) rotateX(${rx.toFixed(2)}deg) rotateY(${ry.toFixed(2)}deg)`;
        }
      };
      el.addEventListener("mousemove", (e) => {
        const r = el.getBoundingClientRect();
        px = (e.clientX - r.left) / r.width;
        py = (e.clientY - r.top) / r.height;
        inside = true;
        el.style.setProperty("--mx", `${(px * 100).toFixed(1)}%`);
        el.style.setProperty("--my", `${(py * 100).toFixed(1)}%`);
        if (!raf) raf = requestAnimationFrame(apply);
      });
      el.addEventListener("mouseleave", () => { inside = false; if (!raf) raf = requestAnimationFrame(apply); });
      if (tilt) el.style.transition = (el.style.transition ? el.style.transition + ", " : "") + "transform 0.6s cubic-bezier(0.16,1,0.3,1)";
    });
  }

  /** Cycles the highlighted verb in the hero lede with a vertical slide. */
  function setupWordSwap() {
    const el = $(".swap");
    if (!el) return;
    const words = (el.dataset.words || "").split(",").map((w) => w.trim()).filter(Boolean);
    if (words.length < 2) return;
    el.textContent = "";
    const w = document.createElement("span");
    w.className = "w";
    w.textContent = words[0];
    el.appendChild(w);
    if (reduced) return;
    let idx = 0;
    const measure = () => {
      const probe = document.createElement("span");
      probe.style.cssText = "position:absolute;visibility:hidden;white-space:nowrap;font:inherit";
      el.appendChild(probe);
      const widths = words.map((t) => { probe.textContent = t; return probe.offsetWidth; });
      el.removeChild(probe);
      return widths;
    };
    const widths = measure();
    el.style.transition = "width 0.5s cubic-bezier(0.16,1,0.3,1)";
    el.style.width = `${widths[0] + 4}px`;
    setInterval(() => {
      idx = (idx + 1) % words.length;
      const old = $(".w", el);
      old.classList.add("out");
      const next = document.createElement("span");
      next.className = "w in";
      next.textContent = words[idx];
      el.style.width = `${widths[idx] + 4}px`;
      setTimeout(() => { old.remove(); el.appendChild(next); }, 380);
    }, 2400);
  }

  /** Types the agent's final message once the mockup is in view. */
  function setupTyper() {
    const t = $(".typer");
    if (!t) return;
    const text = t.dataset.type || "";
    if (reduced) { t.textContent = text; t.classList.add("done"); return; }
    const mock = t.closest(".mock");
    const start = () => {
      let i = 0;
      const step = () => {
        t.textContent = text.slice(0, i++);
        if (i <= text.length) setTimeout(step, 14 + Math.random() * 26);
        else t.classList.add("done");
      };
      setTimeout(step, 3300);
    };
    const obs = new MutationObserver(() => {
      if (mock.classList.contains("is-in")) { obs.disconnect(); start(); }
    });
    if (mock.classList.contains("is-in")) start();
    else obs.observe(mock, { attributes: true, attributeFilter: ["class"] });
  }

  /** Counts stat numbers up when they enter the viewport. */
  function setupCounters() {
    const els = $$("[data-count]");
    if (!els.length) return;
    const run = (el) => {
      const target = Number(el.dataset.count);
      if (reduced || target === 0) { el.textContent = String(target); return; }
      const dur = 1200; const t0 = performance.now();
      const tick = (now) => {
        const p = Math.min(1, (now - t0) / dur);
        const eased = 1 - Math.pow(1 - p, 3);
        el.textContent = String(Math.round(target * eased));
        if (p < 1) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    };
    const io = new IntersectionObserver((entries) => {
      entries.forEach((e) => { if (e.isIntersecting) { run(e.target); io.unobserve(e.target); } });
    }, { threshold: 0.4 });
    els.forEach((el) => io.observe(el));
  }

  /** Detects the visitor's OS to label the hero CTA and highlight the matching download card. */
  function setupOsDetect() {
    const ua = navigator.userAgent || "";
    const plat = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
    let os = "windows", label = "Windows";
    if (/Mac|iPhone|iPad/i.test(plat) || /Mac OS X/i.test(ua)) { os = "mac"; label = "macOS"; }
    else if (/Linux|X11/i.test(plat) && !/Android/i.test(ua)) { os = "linux"; label = "Linux"; }
    else if (/Android/i.test(ua)) { os = ""; label = "desktop"; }
    const heroLabel = $("#hero-dl-label");
    if (heroLabel) heroLabel.textContent = os ? `Download for ${label}` : "Download for desktop";
    if (os) {
      const card = $(`.dl[data-os="${os}"]`);
      if (card) card.classList.add("recommended");
    }
  }

  /** Fills footer year and pulls the latest release tag from GitHub (best-effort). */
  function setupFooterMeta() {
    const y = $("#year");
    if (y) y.textContent = String(new Date().getFullYear());
    const v = $("#ver");
    if (!v) return;
    fetch("https://api.github.com/repos/avijitbhuin21/Rustic/releases/latest", { headers: { Accept: "application/vnd.github+json" } })
      .then((r) => (r.ok ? r.json() : null))
      .then((j) => { if (j && j.tag_name) v.textContent = String(j.tag_name).replace(/^v/, ""); else v.parentElement.style.display = "none"; })
      .catch(() => { v.parentElement.style.display = "none"; });
  }

  /** Smooth in-page anchor scrolling that accounts for the fixed nav. */
  function setupAnchors() {
    document.addEventListener("click", (e) => {
      const a = e.target.closest('a[href^="#"]');
      if (!a) return;
      const id = a.getAttribute("href").slice(1);
      const target = id ? document.getElementById(id) : null;
      if (!target) return;
      e.preventDefault();
      const top = target.getBoundingClientRect().top + window.scrollY - (id === "top" ? 0 : 56);
      window.scrollTo({ top, behavior: reduced ? "auto" : "smooth" });
      history.replaceState(null, "", `#${id}`);
    });
  }

  document.addEventListener("DOMContentLoaded", () => {
    $$("[data-split]").forEach(splitText);
    setupIntro();
    setupReveals();
    setupNav();
    setupProgress();
    setupCursor();
    setupMagnetic();
    setupSpotAndTilt();
    setupWordSwap();
    setupTyper();
    setupCounters();
    setupOsDetect();
    setupFooterMeta();
    setupAnchors();
  });
})();
