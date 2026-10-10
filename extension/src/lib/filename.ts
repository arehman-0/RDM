// File-name and quality guesses for display (pure). The app re-sanitises any
// name it receives, so this only needs to be readable, not authoritative.

import type { MediaKind } from "./media";

const DEFAULT_EXT: Record<MediaKind, string> = { video: "mp4", audio: "mp3", hls: "m3u8", dash: "mpd" };

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** Strip path parts and characters no file system accepts. */
export function cleanName(raw: string): string | null {
  const base = raw.split(/[\\/]/).pop() ?? "";
  const name = base
    .replace(/[<>:"|?*\u0000-\u001f]/g, "_")
    .trim()
    .replace(/[. ]+$/, "");
  return name && !/^\.+$/.test(name) ? name.slice(0, 200) : null;
}

/** `filename*=UTF-8''…` (preferred) or `filename="…"` from Content-Disposition. */
export function fileNameFromDisposition(cd: string | null | undefined): string | null {
  if (!cd) return null;
  const ext = cd.match(/filename\*\s*=\s*([^']*)'[^']*'([^;]+)/i);
  if (ext) return cleanName(decode(ext[2].trim().replace(/^"|"$/g, "")));
  const plain = cd.match(/filename\s*=\s*("([^"]*)"|[^;]+)/i);
  if (plain) return cleanName((plain[2] ?? plain[1]).trim());
  return null;
}

/** Last path segment of the URL, decoded. */
export function fileNameFromUrl(url: string): string | null {
  try {
    const seg = new URL(url).pathname.split("/").filter(Boolean).pop();
    return seg ? cleanName(decode(seg)) : null;
  } catch {
    return null;
  }
}

/** Best display name: Content-Disposition, then URL, then "<kind>.<ext>". */
export function guessFileName(url: string, kind: MediaKind, cd?: string | null): string {
  const name = fileNameFromDisposition(cd) ?? fileNameFromUrl(url);
  if (name && name.includes(".")) return name;
  const stem = name ?? kind;
  return `${stem}.${DEFAULT_EXT[kind]}`;
}

/** "1080p", "720p", "4K" … from URL hints, if any. */
export function guessQuality(url: string): string | null {
  const s = decode(url).toLowerCase();
  if (/(^|[^a-z0-9])(2160p?|4k|uhd)([^a-z0-9]|$)/.test(s)) return "2160p";
  const p = s.match(/(?:^|[^0-9])(\d{3,4})p(?:[^a-z0-9]|$)/);
  if (p) return `${p[1]}p`;
  const wh = s.match(/(?:^|[^0-9])\d{3,4}x(\d{3,4})(?:[^0-9]|$)/);
  if (wh) return `${wh[1]}p`;
  const hq = s.match(/[_\-/](hd|sd|hq|lq)[_\-./]/);
  return hq ? hq[1].toUpperCase() : null;
}
