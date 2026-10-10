// Pure display formatting helpers.

export function fmtBytes(n: number): string {
  if (!n || n < 0 || !Number.isFinite(n)) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 && i > 0 ? 2 : 1)} ${units[i]}`;
}

export function fmtSpeed(bps: number): string {
  return `${fmtBytes(bps)}/s`;
}

/** Seconds → "42s" / "3m 5s" / "2h 7m"; negative or non-finite → "—". */
export function fmtEta(sec: number): string {
  if (sec < 0 || !Number.isFinite(sec)) return "—";
  const s = Math.round(sec);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  if (m < 60) return `${m}m ${r}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

/** Epoch seconds → short local date-time, e.g. "Thu, Oct 8, 04:28 PM". */
export function fmtWhen(epochSecs: number): string {
  return new Date(epochSecs * 1000).toLocaleString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Epoch ms → "HH:MM:SS" (local) for the log panel. */
export function fmtClock(epochMs: number): string {
  const d = new Date(epochMs);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}
