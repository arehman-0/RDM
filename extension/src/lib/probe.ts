// Recognising HLS/DASH playlists by their content (pure; unit-tested).
//
// Many players load their playlist from a URL that doesn't look like one
// (`/stream.txt`, `/api/source?id=…`, a disguised `.png`) or with a generic
// Content-Type, so the network sniffer can't tell. The MAIN-world probe
// (page-probe.ts) shows the first bytes of small text responses to
// `manifestFromText`, which only needs the beginning of the body.

export type ManifestKind = "hls" | "dash";

export interface ManifestHit {
  kind: ManifestKind;
  /** HLS master playlist (lists qualities) — the best thing to download. */
  master: boolean;
}

/** Bytes of a response body the probe looks at. */
export const PROBE_BYTES = 4096;
/** Responses larger than this are never read (segments, video files). */
export const PROBE_MAX_LENGTH = 2 * 1024 * 1024;

/** The message the MAIN-world probe posts to the extension's content script. */
export const PROBE_MESSAGE = "idm-replica:manifest";

export interface ProbeReport {
  type: typeof PROBE_MESSAGE;
  url: string;
  kind: ManifestKind;
  master: boolean;
}

export function manifestFromText(text: string): ManifestHit | null {
  const head = text.replace(/^﻿/, "").trimStart().slice(0, PROBE_BYTES);
  if (head.startsWith("#EXTM3U")) {
    // Audio-only or subtitle playlists are still media playlists; a master
    // lists variants with #EXT-X-STREAM-INF.
    return { kind: "hls", master: head.includes("#EXT-X-STREAM-INF") };
  }
  if (/^(<\?xml[^>]*>\s*)?(<!--[\s\S]*?-->\s*)*<MPD[\s>]/.test(head)) {
    return { kind: "dash", master: true };
  }
  return null;
}

/**
 * Worth reading the body? Skips anything that is clearly media or binary, and
 * anything big. `type` is the response Content-Type (may be empty).
 */
export function worthProbing(type: string, length: number | null): boolean {
  if (length !== null && length > PROBE_MAX_LENGTH) return false;
  const t = type.split(";")[0].trim().toLowerCase();
  if (t.startsWith("video/") || t.startsWith("audio/") || t.startsWith("font/")) return false;
  if (t === "video/mp2t" || t === "application/mp4" || t === "application/wasm") return false;
  return true;
}

/** Validate a report coming from the page (the page can post anything). */
export function parseProbeReport(data: unknown): ProbeReport | null {
  if (typeof data !== "object" || data === null) return null;
  const d = data as Record<string, unknown>;
  if (d.type !== PROBE_MESSAGE || typeof d.url !== "string" || d.url.length > 8192) return null;
  if (d.kind !== "hls" && d.kind !== "dash") return null;
  try {
    const p = new URL(d.url).protocol;
    if (p !== "http:" && p !== "https:") return null;
  } catch {
    return null;
  }
  return { type: PROBE_MESSAGE, url: d.url, kind: d.kind, master: d.master === true };
}
