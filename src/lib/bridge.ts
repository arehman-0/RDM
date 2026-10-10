// Pure helpers for the browser-integration UI (unit-tested in bridge.test.ts).

import type { BridgeInfo, PendingRequest } from "../types";

/** Add a pending request unless it is already listed (oldest first). */
export function addPending(list: PendingRequest[], req: PendingRequest): PendingRequest[] {
  return list.some((r) => r.id === req.id) ? list : [...list, req];
}

export function removePending(list: PendingRequest[], id: string): PendingRequest[] {
  return list.filter((r) => r.id !== id);
}

/** Backlog from `list_bridge_requests`, plus live ones that arrived before it. */
export function mergePending(backlog: PendingRequest[], live: PendingRequest[]): PendingRequest[] {
  return live.reduce(addPending, backlog);
}

/** Extension IDs typed one per line (or comma/space separated). */
export function parseExtensionIds(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** The panel's one-line status. */
export function bridgeStatusLine(info: BridgeInfo | null): string {
  if (!info) return "Loading…";
  if (info.listening && info.address) {
    const moved =
      info.port !== null && info.port !== info.defaultPort
        ? ` (port ${info.defaultPort} was busy)`
        : "";
    return `Listening on ${info.address}${moved}`;
  }
  if (info.error) return `Not listening: ${info.error}`;
  return info.settings.enabled ? "Starting…" : "Off";
}
