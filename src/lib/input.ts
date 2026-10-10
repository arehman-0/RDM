// Pure parsing / validation of the add-download form's inputs.

/** "Key: Value" lines → header object (null if none). */
export function parseHeaders(text: string): Record<string, string> | null {
  const out: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const idx = line.indexOf(":");
    if (idx <= 0) continue;
    const key = line.slice(0, idx).trim();
    const value = line.slice(idx + 1).trim();
    if (key) out[key] = value;
  }
  return Object.keys(out).length > 0 ? out : null;
}

/** One URL per line → array (null if none). */
export function parseMirrors(text: string): string[] | null {
  const list = text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
  return list.length > 0 ? list : null;
}

/** `null` if `raw` is a usable download URL, else a message for the user. */
export function validateUrl(raw: string): string | null {
  const text = raw.trim();
  if (!text) return "Paste a link to download.";
  if (/\s/.test(text)) return "A link can't contain spaces — paste one URL at a time.";
  let u: URL;
  try {
    u = new URL(text);
  } catch {
    return /^[\w.-]+\.[a-z]{2,}(\/|$)/i.test(text)
      ? "Add http:// or https:// at the start of the link."
      : "That doesn't look like a link.";
  }
  if (u.protocol !== "http:" && u.protocol !== "https:") {
    return `Only http:// and https:// links are supported (got ${u.protocol}).`;
  }
  if (!u.hostname) return "The link is missing a host name.";
  return null;
}

/**
 * `<input type="datetime-local">` value → epoch seconds.
 * "" → null (start now); unparseable → NaN.
 */
export function scheduleToEpoch(local: string): number | null {
  if (!local) return null;
  return Math.floor(new Date(local).getTime() / 1000);
}

/** Filename guess for the Save-As dialog's default. */
export function guessName(u: string): string {
  const clean = u.split(/[?#]/)[0];
  const name = clean.split("/").pop()?.trim();
  return name && name.length > 0 ? name : "download.bin";
}

/** A `start_download` error caused by missing yt-dlp / ffmpeg. */
export function isPrerequisiteError(message: string): boolean {
  return /missing required tool/i.test(message) || (/yt-dlp|ffmpeg/i.test(message) && /install/i.test(message));
}
