// Network-response classification for the media sniffer (pure; unit-tested).

import { isKnownSiteCdn } from "./sites";

export type MediaKind = "video" | "audio" | "hls" | "dash";

/** Non-manifest media smaller than this is ignored (previews, beacons, UI sounds). */
export const MIN_MEDIA_BYTES = 100 * 1024;

const VIDEO_EXT = new Set(["mp4", "m4v", "webm", "mkv", "mov", "avi", "flv", "ogv", "3gp", "wmv"]);
const AUDIO_EXT = new Set(["mp3", "m4a", "aac", "oga", "ogg", "opus", "wav", "flac", "weba"]);
/** Segments of HLS/DASH streams — the manifest is listed instead. */
const FRAGMENT_EXT = new Set(["ts", "m4s", "m4f", "cmfv", "cmfa", "aac-frag", "fmp4"]);

const HLS_TYPES = ["application/vnd.apple.mpegurl", "application/x-mpegurl", "audio/mpegurl", "audio/x-mpegurl"];
const DASH_TYPES = ["application/dash+xml"];
const FRAGMENT_TYPES = ["video/mp2t", "video/iso.segment", "audio/iso.segment"];

/** Ad / tracking hosts whose media is never listed. */
const AD_HOSTS = [
  "doubleclick.net",
  "googlesyndication.com",
  "googleadservices.com",
  "imasdk.googleapis.com",
  "adservice.google.com",
  "moatads.com",
  "scorecardresearch.com",
  "adnxs.com",
  "advertising.com",
  "taboola.com",
  "outbrain.com",
  "spotxchange.com",
  "springserve.com",
  "teads.tv",
  "innovid.com",
  "serving-sys.com",
  "flashtalking.com",
];
const AD_PATH = /\/(ads?|adserver|vast|vpaid|preroll|advert)[/_.-]/i;

/** Query parameters that change between requests for the same media. */
const VOLATILE_PARAMS = new Set([
  "range", "bytestart", "byteend", "rn", "rbuf", "_", "t", "ts", "time", "timestamp",
  "cb", "cachebuster", "nocache", "rand", "random", "r", "expires", "signature", "sig",
  "token", "hdnts", "hdnea", "policy", "key-pair-id", "st", "e", "hash", "oh", "oe",
]);
/** Parameters that select a byte range: stripped from the URL we download too. */
const RANGE_PARAMS = new Set(["range", "bytestart", "byteend"]);

export function extensionOf(url: string): string | null {
  try {
    const path = new URL(url).pathname;
    const last = path.split("/").pop() ?? "";
    const dot = last.lastIndexOf(".");
    return dot > 0 ? last.slice(dot + 1).toLowerCase() : null;
  } catch {
    return null;
  }
}

export function isHttpUrl(url: string): boolean {
  try {
    const p = new URL(url).protocol;
    return p === "http:" || p === "https:";
  } catch {
    return false;
  }
}

export function isAdOrTracker(url: string): boolean {
  try {
    const u = new URL(url);
    const host = u.hostname.toLowerCase();
    return AD_HOSTS.some((d) => host === d || host.endsWith(`.${d}`)) || AD_PATH.test(u.pathname);
  } catch {
    return false;
  }
}

export type Classification = { fragment: true } | { fragment: false; kind: MediaKind } | null;

/** What a response is, from its Content-Type (preferred) and URL extension. */
export function classify(url: string, contentType?: string | null): Classification {
  const ct = (contentType ?? "").split(";")[0].trim().toLowerCase();
  const ext = extensionOf(url);
  if (FRAGMENT_TYPES.includes(ct) || (ext !== null && FRAGMENT_EXT.has(ext))) return { fragment: true };
  if (HLS_TYPES.includes(ct) || ext === "m3u8") return { fragment: false, kind: "hls" };
  if (DASH_TYPES.includes(ct) || ext === "mpd") return { fragment: false, kind: "dash" };
  if (ct.startsWith("video/")) return { fragment: false, kind: "video" };
  if (ct.startsWith("audio/")) return { fragment: false, kind: "audio" };
  // Generic types: trust the extension.
  if (ct === "" || ct === "application/octet-stream" || ct === "binary/octet-stream") {
    if (ext !== null && VIDEO_EXT.has(ext)) return { fragment: false, kind: "video" };
    if (ext !== null && AUDIO_EXT.has(ext)) return { fragment: false, kind: "audio" };
  }
  return null;
}

/**
 * Full size of the resource: the total from `Content-Range` (a 206 carries only
 * a part in Content-Length), else `Content-Length`; `null` if unknown.
 */
export function totalSize(contentLength?: string | null, contentRange?: string | null): number | null {
  const total = contentRange?.match(/\/(\d+)\s*$/);
  if (total) return Number(total[1]);
  if (contentLength && /^\d+$/.test(contentLength.trim())) return Number(contentLength.trim());
  return null;
}

/**
 * Dedupe key (origin + path + stable params, sorted) and the URL to download
 * (byte-range params removed, everything else kept — signatures are needed).
 */
export function normalizeUrl(url: string): { key: string; cleanUrl: string } {
  const u = new URL(url);
  u.hash = "";
  for (const p of [...u.searchParams.keys()]) {
    if (RANGE_PARAMS.has(p.toLowerCase())) u.searchParams.delete(p);
  }
  const cleanUrl = u.toString();
  const stable = [...u.searchParams.entries()]
    .filter(([k]) => !VOLATILE_PARAMS.has(k.toLowerCase()))
    .sort(([a], [b]) => a.localeCompare(b));
  const query = new URLSearchParams(stable).toString();
  return { key: `${u.origin}${u.pathname}${query ? `?${query}` : ""}`, cleanUrl };
}

export interface ResponseInfo {
  url: string;
  statusCode: number;
  contentType?: string | null;
  contentLength?: string | null;
  contentRange?: string | null;
}

export interface Recordable {
  kind: MediaKind;
  key: string;
  url: string;
  size: number | null;
  mediaType: string | null;
}

/** Apply every sniffer filter; `null` = don't list this response. */
export function toRecordable(r: ResponseInfo): Recordable | null {
  if (!isHttpUrl(r.url) || r.statusCode < 200 || r.statusCode >= 300) return null;
  if (isAdOrTracker(r.url) || isKnownSiteCdn(r.url)) return null;
  const c = classify(r.url, r.contentType);
  if (!c || c.fragment) return null;
  const size = totalSize(r.contentLength, r.contentRange);
  const manifest = c.kind === "hls" || c.kind === "dash";
  if (!manifest && size !== null && size < MIN_MEDIA_BYTES) return null;
  const { key, cleanUrl } = normalizeUrl(r.url);
  const mediaType = (r.contentType ?? "").split(";")[0].trim() || null;
  return { kind: c.kind, key, url: cleanUrl, size, mediaType };
}
