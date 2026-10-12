// Every user-facing error in one place, so wording stays consistent.

export const MSG = {
  notRunning: "IDM Replica isn't running. Start IDM Replica and try again.",
  noToken: "Not connected yet. Click “Connect to IDM Replica” and allow it in the app.",
  badToken: "The connection to IDM Replica is no longer valid. Click “Connect to IDM Replica” to pair again.",
  pairWaiting: "Look at IDM Replica: click Allow there to connect this browser.",
  pairDenied: "The connection was not allowed in IDM Replica. Click Connect to ask again.",
  pairPending: "IDM Replica is already showing a connection prompt. Answer it there.",
  pairRateLimited: "Too many connection attempts. Wait a minute and try again.",
  paired: "Connected to IDM Replica.",
  originBlocked: (id: string) =>
    `IDM Replica doesn't allow this extension yet. Paste this ID into IDM Replica → Browser integration → Allowed extension IDs: ${id}`,
  portMismatch: (found: number, configured: number) =>
    `IDM Replica is running on port ${found}, but Options says ${configured}. Change the port in Options to ${found}.`,
  wrongApp: (port: number) => `Something else is answering on port ${port}; it isn't IDM Replica. Check the port in Options.`,
  blockedUrl: "This link type can't be downloaded (only http and https links are supported).",
  drm: "This site uses protected video; it can't be downloaded.",
  blobUnsupported:
    "This page streams its video through the player (blob:). IDM Replica can only resolve such videos on supported sites (YouTube, Vimeo, …).",
  siteUnsupported: "IDM Replica can't resolve videos from this site yet.",
  nothingFound: "No video found on this page yet. Start playing it and try again.",
  tooMany: "IDM Replica is busy: too many downloads are waiting for confirmation. Accept or dismiss some first.",
  ytdlpMissing: "IDM Replica needs yt-dlp for this video. Install it (see the notice in the app) and try again.",
  jsRuntimeMissing:
    "IDM Replica needs its JavaScript component for this video. Open IDM Replica, let it finish setting up its video components, and try again.",
  ffmpegMissing: "IDM Replica needs ffmpeg for this video. Install it (see the notice in the app) and try again.",
  badRequest: "IDM Replica rejected the request as invalid.",
  sentPending: "Sent to IDM Replica — confirm it in the app.",
  sentQueued: "Sent to IDM Replica — downloading.",
  /** Badge tooltip after Chrome kept a download because the app couldn't take it. */
  fallback: (why: string) => `IDM Replica: ${why} — Chrome handled this download.`,
} as const;

/** Bridge error codes (README "Error codes") → what the user should do. */
export function codeMessage(code: string, extensionId: string): string | null {
  switch (code) {
    case "bad_token":
      return MSG.badToken;
    case "origin_not_allowed":
      return MSG.originBlocked(extensionId);
    case "bad_url":
      return MSG.blockedUrl;
    case "unsupported_site":
      return MSG.siteUnsupported;
    case "drm_protected":
      return MSG.drm;
    case "ytdlp_missing":
      return MSG.ytdlpMissing;
    case "js_runtime_missing":
      return MSG.jsRuntimeMissing;
    case "ffmpeg_missing":
      return MSG.ffmpegMissing;
    case "app_busy":
      return MSG.tooMany;
    case "pairing_denied":
      return MSG.pairDenied;
    case "pairing_pending":
      return MSG.pairPending;
    case "pairing_rate_limited":
      return MSG.pairRateLimited;
    case "bad_request":
    case "unsupported_media_type":
    case "payload_too_large":
      return MSG.badRequest;
    default:
      return null;
  }
}
