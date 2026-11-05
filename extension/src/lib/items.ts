// The per-tab media list model (pure). Persisted per tab in
// chrome.storage.session by the background worker.

import type { MediaKind } from "./media";
import { isKnownVideoPage } from "./sites";

export type ItemKind = MediaKind | "page";

export interface MediaItem {
  /** Dedupe key (normalised URL, or "page:<url>" for a page-level stream). */
  key: string;
  /** What to download (the page URL for kind "page"). */
  url: string;
  kind: ItemKind;
  /** "network" = sniffed response; "dom" = <video>/<audio> src; "page" = known site / blob player. */
  source: "network" | "dom" | "page";
  fileName: string;
  mediaType: string | null;
  size: number | null;
  quality: string | null;
  /** Page the media was found on (sent as Referer). */
  pageUrl: string;
  /** The page's player uses Encrypted Media Extensions (DRM). */
  drm: boolean;
  seenAt: number;
  /** HLS master playlist (or a DASH MPD): lists every quality. */
  master?: boolean;
  /** Seen by the player probe: the page's player really loaded it. */
  fromPlayer?: boolean;
}

export const MAX_ITEMS_PER_TAB = 100;

/** Insert or refresh `item` (same key = same media; keep the best info). */
export function mergeItem(list: MediaItem[], item: MediaItem): MediaItem[] {
  const i = list.findIndex((x) => x.key === item.key);
  if (i < 0) return [...list, item].slice(-MAX_ITEMS_PER_TAB);
  const old = list[i];
  const merged: MediaItem = {
    ...old,
    // The newest URL carries a fresh signature, if any.
    url: item.url,
    size: Math.max(old.size ?? 0, item.size ?? 0) || null,
    mediaType: old.mediaType ?? item.mediaType,
    quality: old.quality ?? item.quality,
    drm: old.drm || item.drm,
    seenAt: item.seenAt,
    master: old.master || item.master || undefined,
    fromPlayer: old.fromPlayer || item.fromPlayer || undefined,
    // The probe knows the exact frame that loaded it (the right Referer).
    pageUrl: item.fromPlayer ? item.pageUrl : old.pageUrl,
  };
  const copy = list.slice();
  copy[i] = merged;
  return copy;
}

/**
 * The playlist behind a blob:/MediaSource player: a master playlist (all
 * qualities; the app picks the best) before a single-quality one, one the
 * player was seen loading before one only sniffed, then the newest.
 */
export function bestStream(list: MediaItem[]): MediaItem | null {
  const streams = list.filter((x) => !x.drm && (x.kind === "hls" || x.kind === "dash"));
  if (streams.length === 0) return null;
  const n = (b: boolean | undefined) => (b ? 1 : 0);
  return streams
    .slice()
    .sort((a, b) => n(b.master) - n(a.master) || n(b.fromPlayer) - n(a.fromPlayer) || b.seenAt - a.seenAt)[0];
}

const KIND_RANK: Record<ItemKind, number> = { page: 4, video: 3, hls: 2, dash: 2, audio: 1 };

function qualityNumber(q: string | null): number {
  const m = q?.match(/(\d+)/);
  return m ? Number(m[1]) : 0;
}

/**
 * "Download page video (best)": the hovered video's own file if we know it;
 * else a known site's page stream (yt-dlp picks the best quality itself);
 * else the biggest / highest-quality file. A page stream on an unknown site
 * can't be resolved by the app, so it never wins.
 */
export function bestItem(
  list: MediaItem[],
  preferredKey?: string | null,
  blobPlayer = false
): MediaItem | null {
  const usable = list.filter((x) => !x.drm && (x.kind !== "page" || isKnownVideoPage(x.url)));
  const preferred = preferredKey ? usable.find((x) => x.key === preferredKey) : undefined;
  if (preferred) return preferred;
  // A blob:/MediaSource player plays a stream: its playlist is the video.
  if (blobPlayer) {
    const stream = bestStream(usable);
    if (stream) return stream;
  }
  if (usable.length === 0) return null;
  return usable.slice().sort(
    (a, b) =>
      KIND_RANK[b.kind] - KIND_RANK[a.kind] ||
      qualityNumber(b.quality) - qualityNumber(a.quality) ||
      (b.size ?? 0) - (a.size ?? 0)
  )[0];
}
