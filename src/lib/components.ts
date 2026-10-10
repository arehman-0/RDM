// Pure helpers for the component setup screen and settings (unit-tested).

import type { ComponentProgress, ComponentsInfo, UpdateOutcome } from "../types";

/** Latest progress event per component id. */
export type ProgressMap = Record<string, ComponentProgress>;

export function applyProgress(map: ProgressMap, p: ComponentProgress): ProgressMap {
  return { ...map, [p.id]: p };
}

/** Should the full-screen setup be shown? (Something is missing and the user hasn't skipped.) */
export function needsSetup(info: ComponentsInfo | null, skipped: boolean): boolean {
  return info !== null && !skipped && (info.missing.length > 0 || info.settingUp);
}

/** 0..100 for one component's bar. */
export function percent(p: ComponentProgress | undefined): number {
  if (!p) return 0;
  if (p.phase === "done" || p.phase === "installing" || p.phase === "verifying") return 100;
  return p.total > 0 ? Math.min(100, (p.downloaded / p.total) * 100) : 0;
}

export function phaseText(p: ComponentProgress | undefined, installed: boolean): string {
  if (installed && (!p || p.phase === "done")) return "Ready";
  switch (p?.phase) {
    case "downloading":
      return "Downloading…";
    case "verifying":
      return "Checking the file…";
    case "installing":
      return "Installing…";
    case "done":
      return "Ready";
    case "error":
      return "Failed";
    default:
      return "Waiting…";
  }
}

/** One sentence for the result of "Check now". */
export function describeOutcome(o: UpdateOutcome | null): string {
  if (!o) return "";
  switch (o.result) {
    case "upToDate":
      return `yt-dlp ${o.version} is the latest version.`;
    case "updated":
      return `Updated yt-dlp from ${o.from} to ${o.to}.`;
    case "rolledBack":
      return `The new yt-dlp didn't work (${o.reason}); the previous version was kept.`;
    case "skipped":
      return `Not updated: ${o.reason}.`;
  }
}

const SOURCE_TEXT = {
  components: "installed by IDM Replica",
  bundled: "bundled with the app",
  path: "found on this PC (PATH)",
} as const;

export function sourceText(source: "components" | "bundled" | "path" | null): string {
  return source ? SOURCE_TEXT[source] : "not installed";
}
