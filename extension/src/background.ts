// MV3 service worker. It can be killed at any time, so nothing important lives
// only in memory: per-tab media lists are in chrome.storage.session, settings
// in chrome.storage.local. All listeners are registered synchronously at the
// top level (required for the worker to be woken by events).

import { checkConnection, describeConn, pair, sendBatch, sendDownload, type SendResult } from "./lib/bridge";
import { guessFileName, guessQuality } from "./lib/filename";
import { bestItem, mergeItem, type MediaItem } from "./lib/items";
import { normalizeUrl, toRecordable } from "./lib/media";
import { MSG } from "./lib/messages";
import type { AllReply, ConnReply, DomMedia, MediaList, PairReply, Request, SettingsReply } from "./lib/protocol";
import { buildPayload, targetForItem, type DownloadPayload, type SendTarget } from "./lib/request";
import {
  decideTakeover,
  DEFAULT_PORT,
  normalizeSettings,
  OnceSet,
  RecentPosts,
  type DownloadInfo,
  type KeepReason,
  type Settings,
} from "./lib/settings";
import { isKnownSiteCdn, isKnownVideoPage, knownSiteFor } from "./lib/sites";

const MENU_ID = "idm-replica-download";

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

async function getSettings(): Promise<Settings> {
  const r = await chrome.storage.local.get("settings");
  return normalizeSettings(r.settings);
}

// ---------------------------------------------------------------------------
// Per-tab media store (chrome.storage.session) — updates are serialised so
// concurrent webRequest events can't overwrite each other.
// ---------------------------------------------------------------------------

let chain: Promise<unknown> = Promise.resolve();
function serialized<T>(fn: () => Promise<T>): Promise<T> {
  const p = chain.then(fn, fn);
  chain = p.catch(() => undefined);
  return p;
}

const tabKey = (tabId: number) => `tab:${tabId}`;

async function readTab(tabId: number): Promise<MediaItem[]> {
  const k = tabKey(tabId);
  const r = await chrome.storage.session.get(k);
  const v: unknown = r[k];
  return Array.isArray(v) ? (v as MediaItem[]) : [];
}

async function setBadge(tabId: number, count: number): Promise<void> {
  try {
    await chrome.action.setBadgeBackgroundColor({ tabId, color: "#2f6fed" });
    await chrome.action.setBadgeText({ tabId, text: count > 0 ? String(Math.min(count, 99)) : "" });
  } catch {
    // The tab is gone.
  }
}

function updateTab(tabId: number, f: (list: MediaItem[]) => MediaItem[]): Promise<void> {
  return serialized(async () => {
    const list = await readTab(tabId);
    const next = f(list);
    if (next === list) return;
    await chrome.storage.session.set({ [tabKey(tabId)]: next });
    await setBadge(tabId, next.length);
  });
}

function clearTab(tabId: number): Promise<void> {
  return serialized(async () => {
    await chrome.storage.session.remove(tabKey(tabId));
    await setBadge(tabId, 0);
  });
}

async function tabUrl(tabId: number): Promise<string> {
  try {
    return (await chrome.tabs.get(tabId)).url ?? "";
  } catch {
    return "";
  }
}

// ---------------------------------------------------------------------------
// 1. Network sniffing (observe only)
// ---------------------------------------------------------------------------

function header(headers: chrome.webRequest.HttpHeader[] | undefined, name: string): string | null {
  const h = headers?.find((x) => x.name.toLowerCase() === name);
  return h?.value ?? null;
}

async function onHeaders(d: chrome.webRequest.WebResponseHeadersDetails): Promise<void> {
  if (d.tabId < 0) return;
  // A new document in the tab: start a fresh list.
  if (d.type === "main_frame") await clearTab(d.tabId);
  // Known stream sites are resolved by the app from the page URL.
  if (d.initiator && knownSiteFor(d.initiator)) return;
  if (isKnownSiteCdn(d.url)) return;
  const rec = toRecordable({
    url: d.url,
    statusCode: d.statusCode,
    contentType: header(d.responseHeaders, "content-type"),
    contentLength: header(d.responseHeaders, "content-length"),
    contentRange: header(d.responseHeaders, "content-range"),
  });
  if (!rec) return;
  const pageUrl = d.type === "main_frame" ? d.url : await tabUrl(d.tabId);
  if (pageUrl && knownSiteFor(pageUrl)) return;
  const item: MediaItem = {
    key: rec.key,
    url: rec.url,
    kind: rec.kind,
    source: "network",
    fileName: guessFileName(rec.url, rec.kind, header(d.responseHeaders, "content-disposition")),
    mediaType: rec.mediaType,
    size: rec.size,
    quality: guessQuality(rec.url),
    pageUrl,
    drm: false,
    seenAt: Date.now(),
  };
  await updateTab(d.tabId, (l) => mergeItem(l, item));
}

chrome.webRequest.onHeadersReceived.addListener(
  (d) => {
    void onHeaders(d);
  },
  { urls: ["<all_urls>"], types: ["main_frame", "sub_frame", "media", "xmlhttprequest", "other"] },
  ["responseHeaders"]
);

// ---------------------------------------------------------------------------
// 2. Per-tab lifecycle
// ---------------------------------------------------------------------------

chrome.tabs.onUpdated.addListener((tabId, change) => {
  // Same-document (SPA) navigation; full loads are cleared by main_frame above.
  if (change.url && change.status !== "loading") void clearTab(tabId);
});
chrome.tabs.onRemoved.addListener((tabId) => {
  void serialized(() => chrome.storage.session.remove(tabKey(tabId)));
});

// Media elements reported by the content script.
function domItems(list: DomMedia[]): MediaItem[] {
  const now = Date.now();
  const out: MediaItem[] = [];
  for (const m of list) {
    if (m.src && !isKnownVideoPage(m.pageUrl) && !isKnownSiteCdn(m.src)) {
      let key: string;
      try {
        key = normalizeUrl(m.src).key;
      } catch {
        continue;
      }
      out.push({
        key,
        url: m.src,
        kind: m.kind,
        source: "dom",
        fileName: guessFileName(m.src, m.kind),
        mediaType: null,
        size: null,
        quality: m.height > 0 ? `${m.height}p` : guessQuality(m.src),
        pageUrl: m.pageUrl,
        drm: m.drm,
        seenAt: now,
      });
    } else {
      // blob:/MSE player or a known stream site: the page itself.
      out.push({
        key: `page:${m.pageUrl}`,
        url: m.pageUrl,
        kind: "page",
        source: "page",
        fileName: m.title || knownSiteFor(m.pageUrl)?.name || "Page video",
        mediaType: null,
        size: null,
        quality: "best",
        pageUrl: m.pageUrl,
        drm: m.drm,
        seenAt: now,
      });
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// 3. Sending to the app
// ---------------------------------------------------------------------------

async function payloadFor(t: SendTarget): Promise<{ payload: DownloadPayload } | { error: string }> {
  const r = await buildPayload(t, {
    // Exactly the cookies Chrome would send to that URL; never stored or logged.
    getCookies: (url) => chrome.cookies.getAll({ url }),
    userAgent: navigator.userAgent,
  });
  return r.ok ? { payload: r.payload } : { error: r.error };
}

async function send(t: SendTarget): Promise<SendResult> {
  const built = await payloadFor(t);
  if ("error" in built) return { ok: false, error: built.error };
  const s = await getSettings();
  return sendDownload(s.port, s.token, built.payload, fetch, chrome.runtime.id);
}

function bestTarget(list: MediaItem[], pageUrl: string, src?: string | null): SendTarget | { error: string } {
  if (isKnownVideoPage(pageUrl)) {
    const drm = list.some((x) => x.drm && x.pageUrl === pageUrl);
    return { url: null, pageUrl, kind: "stream", drm };
  }
  let preferred: string | null = null;
  try {
    preferred = src ? normalizeUrl(src).key : null;
  } catch {
    preferred = null;
  }
  const best = bestItem(list, preferred);
  if (best) return targetForItem(best);
  if (list.some((x) => x.drm)) return { error: MSG.drm };
  return { error: list.some((x) => x.kind === "page") ? MSG.blobUnsupported : MSG.nothingFound };
}

async function sendBest(tabId: number, pageUrl: string, src?: string | null): Promise<SendResult> {
  const t = bestTarget(await readTab(tabId), pageUrl, src);
  return "error" in t ? { ok: false, error: t.error } : send(t);
}

async function sendAll(tabId: number): Promise<SendResult[]> {
  const list = await readTab(tabId);
  const built = await Promise.all(list.map((i) => payloadFor({ ...targetForItem(i), source: "page-scan" })));
  const payloads = built.flatMap((b) => ("payload" in b ? [b.payload] : []));
  const s = await getSettings();
  const sent = payloads.length ? await sendBatch(s.port, s.token, payloads, fetch, chrome.runtime.id) : [];
  let i = 0;
  return built.map((b) => ("payload" in b ? sent[i++] : { ok: false, error: b.error }));
}

function resultText(r: SendResult): string {
  if (!r.ok) return r.error;
  return r.queued ? MSG.sentQueued : MSG.sentPending;
}

async function toast(tabId: number | undefined, text: string): Promise<void> {
  if (tabId === undefined || tabId < 0) return;
  try {
    await chrome.tabs.sendMessage(tabId, { type: "toast", text } satisfies Request, { frameId: 0 });
  } catch {
    // No content script there (e.g. chrome:// pages).
  }
}

// ---------------------------------------------------------------------------
// Messages from content script / popup / options
// ---------------------------------------------------------------------------

async function handle(msg: Request, sender: chrome.runtime.MessageSender): Promise<unknown> {
  const senderTab = sender.tab?.id;
  switch (msg.type) {
    case "dom-media": {
      if (senderTab === undefined) return null;
      const items = domItems(msg.items);
      // The top frame's report is authoritative for page-level entries: after
      // an SPA navigation (YouTube) the previous video's page entry goes away.
      const top = sender.frameId === 0;
      const current = new Set(items.map((i) => i.key));
      await updateTab(senderTab, (l) =>
        items.reduce(mergeItem, top ? l.filter((x) => x.source !== "page" || current.has(x.key)) : l)
      );
      return null;
    }
    case "get-media": {
      const tabId = msg.tabId ?? senderTab;
      if (tabId === undefined) return { items: [], host: "" } satisfies MediaList;
      const url = await tabUrl(tabId);
      let host = "";
      try {
        host = new URL(url).hostname;
      } catch {
        host = "";
      }
      return { items: await readTab(tabId), host } satisfies MediaList;
    }
    case "download-item": {
      const tabId = msg.tabId ?? senderTab;
      const item = tabId === undefined ? undefined : (await readTab(tabId)).find((x) => x.key === msg.key);
      if (!item) return { ok: false, error: MSG.nothingFound } satisfies SendResult;
      return send(targetForItem(item));
    }
    case "download-best": {
      const tabId = msg.tabId ?? senderTab;
      if (tabId === undefined) return { ok: false, error: MSG.nothingFound } satisfies SendResult;
      return sendBest(tabId, msg.pageUrl, msg.src);
    }
    case "download-all":
      return { results: await sendAll(msg.tabId) } satisfies AllReply;
    case "download-url": {
      const url = msg.url.trim();
      const target: SendTarget = isKnownVideoPage(url)
        ? { url: null, pageUrl: url, kind: "stream" }
        : { url, pageUrl: msg.pageUrl ?? null, kind: "auto" };
      return send(target);
    }
    case "check-connection": {
      const s = await getSettings();
      const conn = await checkConnection(s.port, s.token, fetch, DEFAULT_PORT);
      return {
        conn,
        text: describeConn(conn, chrome.runtime.id, s.port),
        extensionId: chrome.runtime.id,
        port: s.port,
      } satisfies ConnReply;
    }
    case "pair": {
      // One-click pairing: the app asks the user, then hands us the token.
      const s = await getSettings();
      const r = await pair(s.port, fetch, chrome.runtime.id);
      if (!r.ok) return { ok: false, error: r.error, code: r.code } satisfies PairReply;
      await chrome.storage.local.set({ settings: { ...s, token: r.token } });
      return { ok: true } satisfies PairReply;
    }
    case "get-settings":
      return { settings: await getSettings() } satisfies SettingsReply;
    case "disable-site": {
      const s = await getSettings();
      const host = msg.host.trim().toLowerCase();
      if (host && !s.disabledSites.includes(host)) {
        await chrome.storage.local.set({ settings: { ...s, disabledSites: [...s.disabledSites, host] } });
      }
      return null;
    }
    case "toast":
      return null;
  }
}

chrome.runtime.onMessage.addListener((msg: Request, sender, sendResponse) => {
  // Only our own pages and content scripts can reach this listener.
  if (sender.id !== chrome.runtime.id) return false;
  handle(msg, sender).then(sendResponse, (e: unknown) =>
    sendResponse({ ok: false, error: e instanceof Error ? e.message : String(e) })
  );
  return true;
});

// ---------------------------------------------------------------------------
// 4. Context menu
// ---------------------------------------------------------------------------

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: MENU_ID,
      title: "Download with IDM Replica",
      contexts: ["link", "video", "audio", "page"],
    });
  });
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId !== MENU_ID) return;
  void (async () => {
    const pageUrl = info.frameUrl ?? info.pageUrl ?? tab?.url ?? null;
    let result: SendResult;
    if ((info.mediaType === "video" || info.mediaType === "audio") && info.srcUrl) {
      result = await send({ url: info.srcUrl, pageUrl, kind: "auto" });
    } else if (info.linkUrl) {
      result = await send(
        isKnownVideoPage(info.linkUrl)
          ? { url: null, pageUrl: info.linkUrl, kind: "stream" }
          : { url: info.linkUrl, pageUrl, kind: "auto" }
      );
    } else if (tab?.id !== undefined && pageUrl) {
      result = await sendBest(tab.id, pageUrl);
    } else {
      result = { ok: false, error: MSG.nothingFound };
    }
    await toast(tab?.id, resultText(result));
  })();
});

// ---------------------------------------------------------------------------
// 5. Take over Chrome's downloads (ON by default)
//
// Order matters, so a download is never lost:
//   1. decide (pure, settings + URL scheme + POST + filters);
//   2. send to the app and WAIT for ok:true;
//   3. only then cancel Chrome's copy and erase it from the list.
// Anything else — app not running, wrong token, refused, not re-fetchable —
// leaves Chrome's download untouched, with a badge hint.
// ---------------------------------------------------------------------------

/** Each download id is handled once (onCreated and onDeterminingFilename both fire). */
const handled = new OnceSet();
/** URLs requested with POST recently: their downloads can't be replayed by the app. */
const recentPosts = new RecentPosts();

chrome.webRequest.onBeforeRequest.addListener(
  (d) => {
    if (d.method === "POST") recentPosts.add(d.url);
  },
  { urls: ["<all_urls>"], types: ["main_frame", "sub_frame", "xmlhttprequest", "other"] }
);

function downloadInfo(item: chrome.downloads.DownloadItem): DownloadInfo {
  return {
    url: item.url,
    finalUrl: item.finalUrl,
    filename: item.filename,
    fileSize: item.fileSize > 0 ? item.fileSize : item.totalBytes,
    incognito: item.incognito,
    referrer: item.referrer,
    byOurExtension: item.byExtensionId === chrome.runtime.id,
    wasPost: recentPosts.has(item.url) || recentPosts.has(item.finalUrl),
  };
}

/** Kept-by-Chrome reasons worth telling the user about (the rest are their own settings). */
const KEEP_HINT: Partial<Record<KeepReason, string>> = {
  "not-refetchable": "this download only exists inside the page (blob/data/file)",
  "post-request": "this download came from a form (POST) and can't be fetched again",
};

function fallbackWhy(r: Extract<SendResult, { ok: false }>): string {
  switch (r.code) {
    case "app_unreachable":
      return "not running";
    case "no_token":
    case "bad_token":
      return "not paired (check the token in Options)";
    case "origin_not_allowed":
      return "this extension isn't allowed in the app yet";
    default:
      return r.error;
  }
}

let hintTimer: ReturnType<typeof setTimeout> | undefined;

/** Badge "!" + tooltip for a few seconds, and a toast in the active tab. */
async function fallbackHint(text: string): Promise<void> {
  try {
    await chrome.action.setBadgeBackgroundColor({ color: "#d29922" });
    await chrome.action.setBadgeText({ text: "!" });
    await chrome.action.setTitle({ title: text });
  } catch {
    // Best effort.
  }
  clearTimeout(hintTimer);
  hintTimer = setTimeout(() => {
    void chrome.action.setBadgeText({ text: "" });
    void chrome.action.setTitle({ title: "IDM Replica" });
  }, 8000);
  try {
    const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
    await toast(tab?.id, text);
  } catch {
    // No active tab.
  }
}

/** "taken" = the app has it and Chrome's copy is gone; "kept" = Chrome continues. */
async function handOff(item: chrome.downloads.DownloadItem): Promise<"taken" | "kept"> {
  const s = await getSettings();
  const info = downloadInfo(item);
  const decision = decideTakeover(info, s);
  if (!decision.take) {
    const why = KEEP_HINT[decision.reason];
    if (why) await fallbackHint(MSG.fallback(why));
    return "kept";
  }
  const r = await send({
    url: info.finalUrl || info.url,
    pageUrl: item.referrer || null,
    kind: "file",
    fileName: item.filename.split(/[\\/]/).pop() || null,
    mediaType: item.mime || null,
    size: info.fileSize,
    source: "takeover",
  });
  if (!r.ok) {
    await fallbackHint(MSG.fallback(fallbackWhy(r)));
    return "kept";
  }
  // The app acknowledged (started, or pending/duplicate): remove Chrome's copy.
  try {
    await chrome.downloads.cancel(item.id);
  } catch {
    // Already finished/failed in Chrome: nothing to remove; the app has it too.
    return "kept";
  }
  try {
    await chrome.downloads.erase({ id: item.id });
  } catch {
    // Cancelled but still listed: harmless.
  }
  return "taken";
}

chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  if (!handled.claim(item.id)) {
    // The onCreated fallback is already handling it.
    suggest();
    return;
  }
  handOff(item).then(
    (res) => {
      if (res === "kept") suggest();
    },
    () => suggest()
  );
  // We answer asynchronously (Chrome waits for suggest()).
  return true;
});

// Fallback for a download that never reaches onDeterminingFilename.
chrome.downloads.onCreated.addListener((item) => {
  setTimeout(() => {
    if (!handled.claim(item.id)) return;
    void chrome.downloads.search({ id: item.id }).then(([current]) => {
      if (current && current.state === "in_progress") void handOff(current);
    });
  }, 1500);
});
