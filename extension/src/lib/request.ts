// Building the POST /v1/download payload (pure apart from the injected cookie
// lookup, so it is tested with a mocked chrome.cookies).

import type { MediaItem } from "./items";
import { isHttpUrl } from "./media";
import { MSG } from "./messages";
import { isKnownVideoPage, knownSiteFor } from "./sites";

export type BridgeKind = "auto" | "file" | "stream";

/** Exactly the bridge contract (PROGRESS.md); undefined fields are omitted by JSON. */
export interface DownloadPayload {
  url?: string;
  pageUrl?: string;
  referer?: string;
  userAgent?: string;
  cookies?: string;
  fileName?: string;
  kind?: BridgeKind;
  mediaType?: string;
  size?: number;
  /** "best" (default) | "audio" | a height like "1080" — streams only. */
  quality?: string;
  /** Why it was sent: take-overs start at once, the rest follow "ask before starting". */
  source?: Source;
}

export type Source = "takeover" | "user-action" | "page-scan";

/** What the user asked to download, before cookies/UA are attached. */
export interface SendTarget {
  /** Media/link URL (may be blob:/data: — then the page is used, if possible). */
  url: string | null;
  /** The page it was found on. */
  pageUrl: string | null;
  kind: BridgeKind;
  fileName?: string | null;
  mediaType?: string | null;
  size?: number | null;
  drm?: boolean;
  /** An HLS/DASH manifest (needs the app's stream path). */
  manifest?: boolean;
  /** The page's player streams through blob:/MSE (no file URL known). */
  pagePlayer?: boolean;
  /** Defaults to "user-action". */
  source?: Source;
  quality?: string | null;
}

export interface CookieLike {
  name: string;
  value: string;
}

export interface RequestDeps {
  /** Cookies Chrome would send to exactly this URL (chrome.cookies.getAll({url})). */
  getCookies(url: string): Promise<CookieLike[]>;
  userAgent: string;
}

export type BuildResult = { ok: true; payload: DownloadPayload } | { ok: false, error: string };

/** The bridge accepts at most 32 KB of cookies; drop whole cookies past that. */
export const MAX_COOKIE_BYTES = 32 * 1024;

export function cookieHeader(cookies: CookieLike[], max = MAX_COOKIE_BYTES): string | undefined {
  const parts: string[] = [];
  let len = 0;
  for (const c of cookies) {
    // A cookie with CR/LF would be refused by the bridge; skip it.
    if (/[\r\n\u0000]/.test(c.name + c.value) || !c.name) continue;
    const part = `${c.name}=${c.value}`;
    const add = part.length + (parts.length ? 2 : 0);
    if (len + add > max) continue;
    parts.push(part);
    len += add;
  }
  return parts.length ? parts.join("; ") : undefined;
}

/** Map a list item to a send target (manifests and page streams handled). */
export function targetForItem(item: MediaItem): SendTarget {
  if (item.kind === "page") return { url: null, pageUrl: item.url, kind: "stream", drm: item.drm, pagePlayer: true };
  return {
    url: item.url,
    pageUrl: item.pageUrl,
    kind: "auto",
    fileName: item.fileName,
    mediaType: item.mediaType,
    size: item.size,
    drm: item.drm,
    manifest: item.kind === "hls" || item.kind === "dash",
  };
}

/** Decide what actually goes to the app, or a user-readable reason why not. */
export function resolveTarget(t: SendTarget): { url?: string; pageUrl?: string; kind: BridgeKind } | { error: string } {
  if (t.drm) return { error: MSG.drm };
  const page = t.pageUrl && isHttpUrl(t.pageUrl) ? t.pageUrl : undefined;
  const pageIsStreamSite = page !== undefined && isKnownVideoPage(page);
  if (t.kind === "stream" || t.manifest || (t.url !== null && !isHttpUrl(t.url))) {
    if (t.url !== null && !isHttpUrl(t.url) && !/^blob:/i.test(t.url) && t.kind !== "stream") {
      return { error: MSG.blockedUrl };
    }
    if (pageIsStreamSite) return { pageUrl: page, kind: "stream" };
    if (t.pagePlayer && !(page && knownSiteFor(page))) return { error: MSG.blobUnsupported };
    if (t.url !== null && /^blob:/i.test(t.url)) return { error: MSG.blobUnsupported };
    // A direct HLS/DASH manifest: the app downloads it with yt-dlp.
    if (t.manifest && t.url !== null && isHttpUrl(t.url)) return { url: t.url, pageUrl: page, kind: "stream" };
    if (page !== undefined && knownSiteFor(page)) return { error: MSG.nothingFound };
    return { error: page ? MSG.siteUnsupported : MSG.blockedUrl };
  }
  if (t.url === null) return { error: MSG.nothingFound };
  return { url: t.url, pageUrl: page, kind: t.kind };
}

export async function buildPayload(t: SendTarget, deps: RequestDeps): Promise<BuildResult> {
  const r = resolveTarget(t);
  if ("error" in r) return { ok: false, error: r.error };
  // Cookies only for the URL that will be fetched (the page for a stream).
  const fetched = r.url ?? r.pageUrl;
  if (!fetched) return { ok: false, error: MSG.nothingFound };
  let cookies: string | undefined;
  try {
    cookies = cookieHeader(await deps.getCookies(fetched));
  } catch {
    cookies = undefined;
  }
  const payload: DownloadPayload = {
    url: r.url,
    pageUrl: r.pageUrl,
    referer: r.pageUrl && r.pageUrl !== r.url ? r.pageUrl : undefined,
    userAgent: deps.userAgent || undefined,
    cookies,
    kind: r.kind,
    source: t.source ?? "user-action",
  };
  if (r.kind === "stream" && t.quality) payload.quality = t.quality;
  if (r.kind !== "stream") {
    if (t.fileName) payload.fileName = t.fileName;
    if (t.mediaType) payload.mediaType = t.mediaType.slice(0, 128);
    if (typeof t.size === "number" && Number.isFinite(t.size) && t.size > 0) payload.size = Math.floor(t.size);
  }
  return { ok: true, payload };
}
