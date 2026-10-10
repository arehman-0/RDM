// Extension settings (chrome.storage.local) and the pure take-over decision.

import { extensionOf } from "./media";

export const DEFAULT_PORT = 38917;

export interface Settings {
  port: number;
  /** Bridge token pasted from the app (never logged, never sent anywhere but 127.0.0.1). */
  token: string;
  /** Hand every Chrome download to the app (ON by default). */
  intercept: boolean;
  /** Only take over downloads at least this big (MB); 0 = no size filter. */
  interceptMinMB: number;
  /** If non-empty, take over only these file extensions. */
  allowExts: string[];
  /** "Let Chrome handle these types": never take these over. */
  denyExts: string[];
  /** Hostnames (page or file) whose downloads Chrome always keeps. */
  neverSites: string[];
  /** Show the floating "Download" button over videos. */
  floatingButton: boolean;
  /** Hostnames where the floating button is hidden. */
  disabledSites: string[];
}

export const DEFAULTS: Settings = {
  port: DEFAULT_PORT,
  token: "",
  intercept: true,
  interceptMinMB: 0,
  allowExts: [],
  denyExts: [],
  neverSites: [],
  floatingButton: true,
  disabledSites: [],
};

function strList(v: unknown, fallback: string[]): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : fallback;
}

function hosts(list: string[]): string[] {
  return [...new Set(list.map((s) => s.trim().toLowerCase().replace(/^\*\./, "")).filter(Boolean))];
}

/** Coerce whatever is in storage into valid settings (bad values → defaults). */
export function normalizeSettings(raw: unknown): Settings {
  const r = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  const port =
    typeof r.port === "number" && Number.isInteger(r.port) && r.port > 0 && r.port < 65536 ? r.port : DEFAULTS.port;
  const min = typeof r.interceptMinMB === "number" && r.interceptMinMB >= 0 ? r.interceptMinMB : DEFAULTS.interceptMinMB;
  return {
    port,
    token: typeof r.token === "string" ? r.token.trim() : "",
    intercept: typeof r.intercept === "boolean" ? r.intercept : DEFAULTS.intercept,
    interceptMinMB: min,
    allowExts: normalizeExts(strList(r.allowExts, DEFAULTS.allowExts)),
    denyExts: normalizeExts(strList(r.denyExts, DEFAULTS.denyExts)),
    neverSites: hosts(strList(r.neverSites, [])),
    floatingButton: typeof r.floatingButton === "boolean" ? r.floatingButton : DEFAULTS.floatingButton,
    disabledSites: hosts(strList(r.disabledSites, [])),
  };
}

/** "zip, .EXE  iso" → ["zip", "exe", "iso"] */
export function parseExtList(text: string): string[] {
  return normalizeExts(text.split(/[\s,;]+/));
}

function normalizeExts(list: string[]): string[] {
  return [...new Set(list.map((e) => e.trim().toLowerCase().replace(/^\./, "")).filter(Boolean))];
}

function hostListed(host: string, list: string[]): boolean {
  const h = host.toLowerCase();
  return list.some((d) => h === d || h.endsWith(`.${d}`));
}

export function siteDisabled(host: string, s: Settings): boolean {
  return hostListed(host, s.disabledSites);
}

function hostOf(url: string | undefined): string {
  if (!url) return "";
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}

/** What Chrome told us about a download (subset of chrome.downloads.DownloadItem). */
export interface DownloadInfo {
  url: string;
  /** After redirects. */
  finalUrl?: string;
  /** Chrome's suggested file name / path. */
  filename: string;
  /** Bytes, or <= 0 when unknown. */
  fileSize: number;
  incognito: boolean;
  /** The page the download came from. */
  referrer?: string;
  /** Started by this extension (never re-intercept our own). */
  byOurExtension?: boolean;
  /** The request that produced it was a POST (the app can't replay it). */
  wasPost?: boolean;
}

/** Why Chrome keeps a download. `app-*` reasons come from the send step. */
export type KeepReason =
  | "disabled"
  | "incognito"
  | "own-download"
  | "not-refetchable" // blob:, data:, filesystem:, file:, …
  | "post-request"
  | "site-excluded"
  | "type-excluded"
  | "not-allowed-type"
  | "too-small";

export type TakeoverDecision = { take: true } | { take: false; reason: KeepReason };

/**
 * Should this Chrome download be handed to the app? Decided BEFORE anything
 * is cancelled; the app's answer is still awaited before Chrome's copy goes.
 */
export function decideTakeover(d: DownloadInfo, s: Settings): TakeoverDecision {
  const keep = (reason: KeepReason): TakeoverDecision => ({ take: false, reason });
  if (!s.intercept) return keep("disabled");
  if (d.byOurExtension) return keep("own-download");
  if (d.incognito) return keep("incognito");
  const url = d.finalUrl || d.url;
  let scheme = "";
  try {
    scheme = new URL(url).protocol;
  } catch {
    return keep("not-refetchable");
  }
  // Only something the app can fetch again by URL.
  if (scheme !== "http:" && scheme !== "https:") return keep("not-refetchable");
  if (d.wasPost) return keep("post-request");
  if (hostListed(hostOf(url), s.neverSites) || hostListed(hostOf(d.referrer), s.neverSites)) {
    return keep("site-excluded");
  }
  const name = d.filename.split(/[\\/]/).pop() ?? "";
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : extensionOf(url);
  if (ext !== null && s.denyExts.includes(ext)) return keep("type-excluded");
  const allowed = ext !== null && s.allowExts.includes(ext);
  if (s.allowExts.length > 0 && !allowed) return keep("not-allowed-type");
  if (s.interceptMinMB > 0) {
    // Unknown size: only an explicitly allow-listed type passes a size floor.
    if (d.fileSize <= 0) return allowed ? { take: true } : keep("too-small");
    if (d.fileSize < s.interceptMinMB * 1024 * 1024) return keep("too-small");
  }
  return { take: true };
}

/** Back-compat helper (true = take over). */
export function shouldIntercept(d: DownloadInfo, s: Settings): boolean {
  return decideTakeover(d, s).take;
}

/**
 * Debounce: the same download id must be handled once, even though both
 * onCreated and onDeterminingFilename report it.
 */
export class OnceSet {
  private seen = new Map<number, number>();
  constructor(private readonly ttlMs = 10 * 60 * 1000) {}
  /** true the first time `id` is claimed (within the TTL). */
  claim(id: number, now = Date.now()): boolean {
    for (const [k, t] of this.seen) if (now - t > this.ttlMs) this.seen.delete(k);
    if (this.seen.has(id)) return false;
    this.seen.set(id, now);
    return true;
  }
}

/** Recently seen POST request URLs (the response may become a download). */
export class RecentPosts {
  private urls = new Map<string, number>();
  constructor(private readonly ttlMs = 60 * 1000) {}
  add(url: string, now = Date.now()): void {
    this.prune(now);
    this.urls.set(url, now);
  }
  has(url: string | undefined, now = Date.now()): boolean {
    if (!url) return false;
    this.prune(now);
    return this.urls.has(url);
  }
  private prune(now: number): void {
    for (const [k, t] of this.urls) if (now - t > this.ttlMs) this.urls.delete(k);
  }
}
