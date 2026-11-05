// Content script (all frames): finds <video>/<audio> elements, reports them to
// the background, and shows the IDM-style floating "Download" button.
// The UI lives in a CLOSED shadow root: site CSS can't reach it, and its CSS
// can't leak into the site. Only the button itself takes pointer events, so
// player clicks are never blocked.

import type { SendResult } from "./lib/bridge";
import type { MediaItem } from "./lib/items";
import { isHttpUrl } from "./lib/media";
import { MSG } from "./lib/messages";
import { parseProbeReport } from "./lib/probe";
import type { DomMedia, MediaList, Request, SettingsReply } from "./lib/protocol";
import { siteDisabled } from "./lib/settings";
import { isKnownVideoPage } from "./lib/sites";

const MIN_W = 200;
const MIN_H = 120;
const SCAN_THROTTLE_MS = 800;
const MIN_DURATION_S = 5;

function send<T>(msg: Request): Promise<T | null> {
  return new Promise((resolve) => {
    try {
      chrome.runtime.sendMessage(msg, (reply: T) => {
        // The worker may be restarting; treat as "no answer".
        resolve(chrome.runtime.lastError ? null : reply);
      });
    } catch {
      // Extension reloaded: this script is orphaned.
      resolve(null);
    }
  });
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

const encrypted = new WeakSet<HTMLMediaElement>();
let media: HTMLMediaElement[] = [];

function collect(root: Document | ShadowRoot, out: HTMLMediaElement[]): void {
  root.querySelectorAll<HTMLMediaElement>("video, audio").forEach((e) => out.push(e));
  // Open shadow roots only (closed ones are unreachable by design).
  root.querySelectorAll("*").forEach((el) => {
    if (el.shadowRoot) collect(el.shadowRoot, out);
  });
}

function mediaSrc(el: HTMLMediaElement): string {
  if (el.currentSrc) return el.currentSrc;
  if (el.src) return el.src;
  const source = el.querySelector("source[src]");
  return source instanceof HTMLSourceElement ? source.src : "";
}

function isDrm(el: HTMLMediaElement): boolean {
  return el.mediaKeys !== null || encrypted.has(el);
}

function describe(el: HTMLMediaElement): DomMedia | null {
  const src = mediaSrc(el);
  if (!src) return null;
  // Wait for metadata (a "loadedmetadata" rescan follows) so short clips —
  // UI sounds, tiny loops — can be filtered by duration.
  if (el.readyState < HTMLMediaElement.HAVE_METADATA) return null;
  if (Number.isFinite(el.duration) && el.duration > 0 && el.duration < MIN_DURATION_S) return null;
  const isVideo = el instanceof HTMLVideoElement;
  return {
    // blob:/MediaSource players are reported as "the page" (src null).
    src: isHttpUrl(src) ? src : null,
    kind: isVideo ? "video" : "audio",
    pageUrl: location.href,
    title: document.title.trim().slice(0, 150),
    drm: isDrm(el),
    width: isVideo ? el.videoWidth : 0,
    height: isVideo ? el.videoHeight : 0,
  };
}

let lastSignature = "";

function scanNow(): void {
  const found: HTMLMediaElement[] = [];
  collect(document, found);
  media = found;
  const items = found.map(describe).filter((x): x is DomMedia => x !== null);
  // A known stream site's video page counts even before its player exists.
  if (items.length === 0 && isKnownVideoPage(location.href)) {
    items.push({ src: null, kind: "video", pageUrl: location.href, title: document.title, drm: false, width: 0, height: 0 });
  }
  const sig = JSON.stringify(items.map((i) => [i.src, i.pageUrl, i.drm, i.height]));
  if (sig === lastSignature) return;
  lastSignature = sig;
  if (items.length) void send({ type: "dom-media", items });
}

let scanTimer: number | undefined;
let lastScan = 0;
function scheduleScan(): void {
  if (scanTimer !== undefined) return;
  const wait = Math.max(0, SCAN_THROTTLE_MS - (Date.now() - lastScan));
  scanTimer = window.setTimeout(() => {
    scanTimer = undefined;
    lastScan = Date.now();
    scanNow();
  }, wait);
}

new MutationObserver(scheduleScan).observe(document.documentElement, {
  childList: true,
  subtree: true,
  attributes: true,
  attributeFilter: ["src"],
});
// Media events don't bubble, but they do pass the capture phase.
for (const ev of ["loadedmetadata", "play", "playing", "emptied"]) {
  document.addEventListener(ev, scheduleScan, true);
}
document.addEventListener(
  "encrypted",
  (e) => {
    if (e.target instanceof HTMLMediaElement) encrypted.add(e.target);
    scheduleScan();
  },
  true
);
// Playlists the page's player loaded, found by page-probe.ts (MAIN world, same
// frame). The page could post this message too, so it is validated; at worst
// a page adds an http(s) URL to its own list, which only the user can send.
window.addEventListener("message", (e) => {
  if (e.source !== window) return;
  const r = parseProbeReport(e.data);
  if (r) void send({ type: "player-manifest", url: r.url, kind: r.kind, master: r.master, frameUrl: location.href });
});

// YouTube is a single-page app: follow its navigations.
document.addEventListener("yt-navigate-finish", () => {
  lastSignature = "";
  hide(true);
  scheduleScan();
});
window.addEventListener("popstate", scheduleScan);
scheduleScan();

// ---------------------------------------------------------------------------
// Floating button
// ---------------------------------------------------------------------------

let enabled = true;
void send<SettingsReply>({ type: "get-settings" }).then((r) => {
  if (r) enabled = r.settings.floatingButton && !siteDisabled(location.hostname, r.settings);
});
chrome.storage.onChanged.addListener((changes) => {
  if (!changes.settings) return;
  void send<SettingsReply>({ type: "get-settings" }).then((r) => {
    if (!r) return;
    enabled = r.settings.floatingButton && !siteDisabled(location.hostname, r.settings);
    if (!enabled) hide(true);
  });
});

const CSS = `
:host { all: initial; }
.wrap { position: fixed; inset: 0 auto auto 0; width: 0; height: 0; pointer-events: none;
  font: 13px/1.35 system-ui, -apple-system, "Segoe UI", sans-serif; color: #e6edf3; }
.btn { position: fixed; display: none; align-items: center; gap: 6px; pointer-events: auto; cursor: pointer;
  background: rgba(20, 24, 33, .88); border: 1px solid rgba(255,255,255,.18); border-radius: 8px;
  padding: 6px 10px; color: #fff; font: inherit; font-weight: 600; box-shadow: 0 4px 16px rgba(0,0,0,.35); }
.btn:hover { background: #2f6fed; border-color: #2f6fed; }
.btn.show { display: inline-flex; }
.arrow { font-size: 15px; line-height: 1; }
.menu { position: fixed; display: none; pointer-events: auto; min-width: 260px; max-width: 420px; max-height: 320px;
  overflow: auto; background: #161b22; border: 1px solid #30363d; border-radius: 10px; padding: 6px;
  box-shadow: 0 10px 30px rgba(0,0,0,.5); }
.menu.show { display: block; }
.item { display: block; width: 100%; text-align: left; background: none; border: 0; border-radius: 6px;
  color: inherit; font: inherit; padding: 7px 9px; cursor: pointer; }
.item:hover { background: #21262d; }
.item.primary { font-weight: 600; color: #58a6ff; }
.item.muted { color: #8b949e; font-size: 12px; }
.sub { color: #8b949e; font-size: 12px; }
.sep { height: 1px; background: #30363d; margin: 5px 0; }
.note { padding: 7px 9px; color: #8b949e; font-size: 12px; }
.toast { position: fixed; right: 16px; bottom: 16px; max-width: 380px; pointer-events: none; display: none;
  background: #161b22; border: 1px solid #30363d; border-left: 3px solid #2f6fed; border-radius: 8px;
  padding: 10px 12px; box-shadow: 0 10px 30px rgba(0,0,0,.5); }
.toast.show { display: block; }
.toast.err { border-left-color: #f85149; }
`;

interface Ui {
  host: HTMLElement;
  btn: HTMLButtonElement;
  menu: HTMLDivElement;
  toast: HTMLDivElement;
}
let ui: Ui | null = null;

function getUi(): Ui {
  if (ui) return ui;
  const host = document.createElement("idm-replica-overlay");
  host.style.cssText = "all:initial;position:fixed;top:0;left:0;width:0;height:0;z-index:2147483647;";
  const root = host.attachShadow({ mode: "closed" });
  const style = document.createElement("style");
  style.textContent = CSS;
  const wrap = document.createElement("div");
  wrap.className = "wrap";
  const btn = document.createElement("button");
  btn.className = "btn";
  btn.type = "button";
  btn.innerHTML = `<span class="arrow">⬇</span><span>Download</span>`;
  const menu = document.createElement("div");
  menu.className = "menu";
  const toast = document.createElement("div");
  toast.className = "toast";
  wrap.append(btn, menu, toast);
  root.append(style, wrap);
  // Keep the site from seeing our clicks.
  for (const ev of ["click", "mousedown", "mouseup", "pointerdown", "pointerup", "dblclick"]) {
    wrap.addEventListener(ev, (e) => e.stopPropagation());
  }
  btn.addEventListener("click", (e) => {
    e.preventDefault();
    void toggleMenu();
  });
  mountHost(host);
  ui = { host, btn, menu, toast };
  return ui;
}

/** Inside the fullscreen element when there is one (else it'd be invisible). */
function mountHost(host: HTMLElement): void {
  const fs = document.fullscreenElement;
  const parent = fs && !(fs instanceof HTMLMediaElement) ? fs : document.documentElement;
  if (host.parentNode !== parent) parent.appendChild(host);
}

document.addEventListener("fullscreenchange", () => {
  if (!ui) return;
  mountHost(ui.host);
  // A bare <video> in fullscreen can't host an overlay.
  if (document.fullscreenElement instanceof HTMLMediaElement) hide(true);
});

let target: HTMLVideoElement | null = null;
let hideTimer: number | undefined;
let menuOpen = false;

function eligible(v: HTMLMediaElement): v is HTMLVideoElement {
  if (!(v instanceof HTMLVideoElement)) return false;
  const r = v.getBoundingClientRect();
  if (r.width < MIN_W || r.height < MIN_H) return false;
  return !v.paused || mediaSrc(v) !== "" || isKnownVideoPage(location.href);
}

function place(): void {
  if (!ui || !target) return;
  const r = target.getBoundingClientRect();
  if (r.bottom < 0 || r.top > innerHeight || r.width === 0) {
    hide(true);
    return;
  }
  const bw = ui.btn.offsetWidth || 110;
  const top = Math.max(r.top, 0) + 10;
  const left = Math.min(r.right, innerWidth) - bw - 10;
  ui.btn.style.top = `${top}px`;
  ui.btn.style.left = `${left}px`;
  ui.menu.style.top = `${top + 36}px`;
  ui.menu.style.left = `${Math.max(8, Math.min(r.right, innerWidth) - 10 - Math.max(ui.menu.offsetWidth, 260))}px`;
}

function show(v: HTMLVideoElement): void {
  if (!enabled) return;
  if (document.fullscreenElement === v) return;
  const u = getUi();
  mountHost(u.host);
  window.clearTimeout(hideTimer);
  if (target !== v) closeMenu();
  target = v;
  u.btn.classList.add("show");
  u.btn.title = isDrm(v) ? MSG.drm : "Download with IDM Replica";
  place();
}

function hide(now = false): void {
  window.clearTimeout(hideTimer);
  const go = () => {
    if (menuOpen && !now) return;
    closeMenu();
    ui?.btn.classList.remove("show");
    target = null;
  };
  if (now) go();
  else hideTimer = window.setTimeout(go, 1200);
}

function videoAt(x: number, y: number): HTMLVideoElement | null {
  for (const m of media) {
    if (!eligible(m)) continue;
    const r = m.getBoundingClientRect();
    if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return m;
  }
  return null;
}

let moveQueued = false;
document.addEventListener(
  "pointermove",
  (e) => {
    if (!enabled || moveQueued) return;
    moveQueued = true;
    const { clientX, clientY } = e;
    const overUi = ui !== null && e.composedPath().includes(ui.host);
    requestAnimationFrame(() => {
      moveQueued = false;
      if (overUi) {
        window.clearTimeout(hideTimer);
        return;
      }
      const v = videoAt(clientX, clientY);
      if (v) show(v);
      else if (target) hide();
    });
  },
  { passive: true, capture: true }
);
// A large video that starts playing gets the button for a few seconds.
document.addEventListener(
  "playing",
  (e) => {
    const v = e.target;
    if (v instanceof HTMLVideoElement && eligible(v) && !menuOpen) {
      show(v);
      hideTimer = window.setTimeout(() => hide(), 3500);
    }
  },
  true
);
window.addEventListener("scroll", place, { passive: true, capture: true });
window.addEventListener("resize", place, { passive: true });

// ---------------------------------------------------------------------------
// Menu
// ---------------------------------------------------------------------------

function fmtSize(n: number | null): string {
  if (!n) return "";
  const units = ["B", "KB", "MB", "GB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
}

function closeMenu(): void {
  menuOpen = false;
  ui?.menu.classList.remove("show");
}

function row(label: string, sub: string, cls: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = `item ${cls}`;
  b.textContent = label;
  if (sub) {
    const s = document.createElement("div");
    s.className = "sub";
    s.textContent = sub;
    b.append(s);
  }
  b.addEventListener("click", (e) => {
    e.preventDefault();
    onClick();
  });
  return b;
}

function describeItem(i: MediaItem): string {
  const kind = i.kind === "page" ? "page stream" : i.kind.toUpperCase();
  return [kind, i.quality ?? "", fmtSize(i.size), i.drm ? "protected" : ""].filter(Boolean).join(" · ");
}

async function toggleMenu(): Promise<void> {
  const u = getUi();
  if (menuOpen) {
    closeMenu();
    return;
  }
  menuOpen = true;
  u.menu.replaceChildren(Object.assign(document.createElement("div"), { className: "note", textContent: "Loading…" }));
  u.menu.classList.add("show");
  place();
  const list = await send<MediaList>({ type: "get-media" });
  const items = list?.items ?? [];
  const nodes: HTMLElement[] = [];
  if (target && isDrm(target)) {
    nodes.push(Object.assign(document.createElement("div"), { className: "note", textContent: MSG.drm }));
  }
  nodes.push(row("Download page video (best)", "", "primary", () => void act({ type: "download-best", pageUrl: location.href, src: target && isHttpUrl(mediaSrc(target)) ? mediaSrc(target) : null })));
  if (items.length) nodes.push(Object.assign(document.createElement("div"), { className: "sep" }));
  for (const i of items) {
    nodes.push(row(i.fileName, describeItem(i), "", () => void act({ type: "download-item", key: i.key })));
  }
  if (!items.length) {
    nodes.push(Object.assign(document.createElement("div"), { className: "note", textContent: MSG.nothingFound }));
  }
  nodes.push(Object.assign(document.createElement("div"), { className: "sep" }));
  nodes.push(
    row(`Don't show this button on ${location.hostname}`, "Undo in the extension's Options", "muted", () => {
      void send({ type: "disable-site", host: location.hostname });
      enabled = false;
      hide(true);
    })
  );
  u.menu.replaceChildren(...nodes);
  place();
}

async function act(msg: Request): Promise<void> {
  closeMenu();
  const r = await send<SendResult>(msg);
  if (!r) showToast("The extension was updated — reload this page.", true);
  else if (r.ok) showToast(r.queued ? MSG.sentQueued : MSG.sentPending, false);
  else showToast(r.error, true);
}

function showToast(text: string, error: boolean): void {
  const u = getUi();
  u.toast.textContent = text;
  u.toast.classList.toggle("err", error);
  u.toast.classList.add("show");
  window.setTimeout(() => u.toast.classList.remove("show"), error ? 7000 : 3500);
}

document.addEventListener(
  "click",
  (e) => {
    if (menuOpen && ui && !e.composedPath().includes(ui.host)) closeMenu();
  },
  true
);

// Results of context-menu actions (sent to the top frame only).
chrome.runtime.onMessage.addListener((msg: Request) => {
  if (msg.type === "toast" && window === window.top) showToast(msg.text, !msg.text.startsWith("Sent"));
  return false;
});
