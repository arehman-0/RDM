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
  };
  const copy = list.slice();
  copy[i] = merged;
  return copy;
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
export function bestItem(list: MediaItem[], preferredKey?: string | null): MediaItem | null {
  const usable = list.filter((x) => !x.drm && (x.kind !== "page" || isKnownVideoPage(x.url)));
  const preferred = preferredKey ? usable.find((x) => x.key === preferredKey) : undefined;
  if (preferred) return preferred;
  if (usable.length === 0) return null;
  return usable.slice().sort(
    (a, b) =>
      KIND_RANK[b.kind] - KIND_RANK[a.kind] ||
      qualityNumber(b.quality) - qualityNumber(a.quality) ||
      (b.size ?? 0) - (a.size ?? 0)
  )[0];
}
