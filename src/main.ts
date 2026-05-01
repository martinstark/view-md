import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { openUrl } from "@tauri-apps/plugin-opener";

const ZOOM_MIN = 0.5;
const ZOOM_MAX = 3.0;
const ZOOM_STEP = 0.1;
const ZOOM_DEFAULT = 1.0;

const SCROLL_LINE_PX = 40;
const HALF_PAGE_FRAC = 0.5;
const FULL_PAGE_FRAC = 0.9;
const HEADING_OFFSET_PX = 24;
const HEADING_SEL = "h1, h2, h3, h4, h5, h6";
const BLOCK_SEL = ":scope > :is(p, blockquote, ul, ol, pre, table, hr)";

const clampZoom = (z: number) => Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, z));

const readZoom = (): number => {
  const raw = Number(localStorage.getItem("zoom"));
  return Number.isFinite(raw) && raw > 0 ? clampZoom(raw) : ZOOM_DEFAULT;
};

let zoom = ZOOM_DEFAULT;

const applyZoom = async (z: number, persist = false) => {
  zoom = clampZoom(z);
  await getCurrentWebview().setZoom(zoom).catch(() => {});
  if (persist) localStorage.setItem("zoom", String(zoom));
};

interface DocPayload {
  html: string;
  title: string;
  cssLight: string;
  cssDark: string;
}

const $ = <T extends Element>(sel: string) => document.querySelector<T>(sel);

const setTheme = (mode: "light" | "dark", persist = false) => {
  document.documentElement.dataset.theme = mode;
  const lightOn = mode === "light" ? "all" : "not all";
  const darkOn = mode === "dark" ? "all" : "not all";
  $<HTMLLinkElement>("#gh-light")!.media = lightOn;
  $<HTMLLinkElement>("#gh-dark")!.media = darkOn;
  $<HTMLStyleElement>("#syntect-light")!.media = lightOn;
  $<HTMLStyleElement>("#syntect-dark")!.media = darkOn;
  if (persist) localStorage.setItem("theme", mode);
};

const initTheme = () => {
  // data-theme + GH link media already set by inline <head> script;
  // sync syntect <style> media to match.
  const mode = (document.documentElement.dataset.theme as "light" | "dark") || "dark";
  setTheme(mode);
};

const toggleTheme = () => {
  const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
  setTheme(next, true);
};

const wireLinks = (root: HTMLElement) => {
  root.addEventListener("click", (e) => {
    const a = (e.target as HTMLElement).closest("a");
    if (!a) return;
    const href = a.getAttribute("href") ?? "";
    if (/^https?:\/\//.test(href)) {
      e.preventDefault();
      openUrl(href).catch(console.error);
    }
  });
};

const scrollBy = (dy: number) => window.scrollBy({ top: dy, behavior: "instant" });
const scrollLines = (n: number) => scrollBy(n * SCROLL_LINE_PX);
const scrollHalfPage = (dir: 1 | -1) => scrollBy(dir * window.innerHeight * HALF_PAGE_FRAC);
const scrollFullPage = (dir: 1 | -1) => scrollBy(dir * window.innerHeight * FULL_PAGE_FRAC);
const scrollToTop = () => window.scrollTo({ top: 0, behavior: "instant" });
const scrollToBottom = () =>
  window.scrollTo({ top: document.documentElement.scrollHeight, behavior: "instant" });

const jumpTo = (root: HTMLElement, selector: string, dir: 1 | -1) => {
  const els = Array.from(root.querySelectorAll<HTMLElement>(selector));
  if (els.length === 0) return;
  const cur = window.scrollY;
  const tops = els.map((el) => el.getBoundingClientRect().top + cur);
  const target =
    dir === 1
      ? tops.find((t) => t > cur + 5)
      : [...tops].reverse().find((t) => t < cur - 5);
  if (target !== undefined) {
    window.scrollTo({ top: Math.max(0, target - HEADING_OFFSET_PX), behavior: "instant" });
  }
};

const toggleHelp = () => {
  const help = $<HTMLElement>("#help")!;
  help.hidden = !help.hidden;
};

const wireCopyButtons = (root: HTMLElement) => {
  root.querySelectorAll<HTMLPreElement>("pre.code").forEach((pre) => {
    const btn = document.createElement("button");
    btn.className = "copy-btn";
    btn.textContent = "copy";
    btn.addEventListener("click", async () => {
      const code = pre.querySelector("code")?.textContent ?? "";
      await navigator.clipboard.writeText(code);
      btn.textContent = "copied";
      setTimeout(() => (btn.textContent = "copy"), 1500);
    });
    pre.appendChild(btn);
  });
};

const init = async () => {
  const doc = await invoke<DocPayload>("load_document");
  document.title = doc.title;

  $<HTMLStyleElement>("#syntect-light")!.textContent = doc.cssLight;
  $<HTMLStyleElement>("#syntect-dark")!.textContent = doc.cssDark;

  initTheme();
  await applyZoom(readZoom());

  const article = $<HTMLElement>("#content")!;
  article.innerHTML = doc.html;

  wireLinks(article);
  wireCopyButtons(article);

  document.addEventListener("keydown", (e) => {
    const help = $<HTMLElement>("#help")!;
    if (!help.hidden) {
      if (e.key === "Escape" || e.key === "?") {
        e.preventDefault();
        help.hidden = true;
      }
      return;
    }

    if (e.ctrlKey || e.altKey || e.metaKey) return;

    switch (e.key) {
      case "q":
        e.preventDefault();
        getCurrentWindow().close().catch(() => {});
        break;
      case "t":
        e.preventDefault();
        toggleTheme();
        break;
      case "+":
      case "=":
        e.preventDefault();
        applyZoom(zoom + ZOOM_STEP, true);
        break;
      case "-":
        e.preventDefault();
        applyZoom(zoom - ZOOM_STEP, true);
        break;
      case "0":
        e.preventDefault();
        applyZoom(ZOOM_DEFAULT, true);
        break;
      case "j":
        e.preventDefault();
        scrollLines(1);
        break;
      case "k":
        e.preventDefault();
        scrollLines(-1);
        break;
      case "d":
        e.preventDefault();
        scrollHalfPage(1);
        break;
      case "u":
        e.preventDefault();
        scrollHalfPage(-1);
        break;
      case "f":
        e.preventDefault();
        scrollFullPage(1);
        break;
      case "b":
        e.preventDefault();
        scrollFullPage(-1);
        break;
      case "g":
        e.preventDefault();
        scrollToTop();
        break;
      case "G":
        e.preventDefault();
        scrollToBottom();
        break;
      case "]":
        e.preventDefault();
        jumpTo(article, HEADING_SEL, 1);
        break;
      case "[":
        e.preventDefault();
        jumpTo(article, HEADING_SEL, -1);
        break;
      case "}":
        e.preventDefault();
        jumpTo(article, BLOCK_SEL, 1);
        break;
      case "{":
        e.preventDefault();
        jumpTo(article, BLOCK_SEL, -1);
        break;
      case "?":
        e.preventDefault();
        toggleHelp();
        break;
    }
  });
};

init().catch((err) => {
  document.body.innerHTML = `<pre style="color:#c00;padding:2rem;">${String(err)}</pre>`;
});
