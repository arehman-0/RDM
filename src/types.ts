// Mirrors of the Rust IPC payloads (keep in sync with the Rust structs and
// README "IPC surface"), plus the UI's own card model.

export interface ThreadProgress {
  id: number;
  start: number;
  current: number;
  end: number;
  progress: number;
  done: boolean;
  failed: boolean;
}

/** `download-progress` — one engine job (a YouTube card has two). */
export interface ProgressPayload {
  id: string;
  fileName: string;
  url: string;
  total: number;
  downloaded: number;
  speed: number; // bytes/s
  eta: number; // seconds, -1 = unknown
  progress: number; // 0..100
  status: string;
  connections: number;
  activeThreads: number;
  speedLimit: number; // bytes/s, 0 = unlimited
  threads: ThreadProgress[];
  error?: string | null;
  sha256?: string | null;
  verified?: boolean | null;
  warnings?: string[]; // non-fatal, e.g. a rejected mirror; omitted when empty
}

/** `youtube-status` — extraction / mux lifecycle of a YouTube card. */
export interface YoutubeStatus {
  id: string;
  fileName: string;
  status: string; // muxing | complete | error | canceled
  error?: string | null;
}

export type Priority = "high" | "normal" | "low";

/** `queue-status` — a not-yet-running entry (or one that just left the queue). */
export interface QueueStatus {
  id: string;
  fileName: string;
  status: string; // queued | scheduled | held | starting | canceled
  position: number; // 1-based among waiting; 0 if not waiting
  startAt?: number | null; // epoch seconds, for "scheduled"
  priority: Priority;
  held: boolean;
}

/** A persisted download (`list_history`). Header values are never stored. */
export interface JobRecord {
  id: string;
  kind: "file" | "youtube" | "manifest";
  url: string;
  mirrors: string[];
  headerNames: string[];
  dir: string;
  dest: string | null;
  fileName: string;
  status: string; // queued|scheduled|downloading|paused|muxing|interrupted|complete|error|canceled
  total: number;
  downloaded: number;
  connections: number;
  speedLimit: number;
  verify: boolean;
  expectedChecksum: string | null;
  startAt: number | null;
  priority: Priority;
  held: boolean;
  createdAt: number;
  updatedAt: number;
  error: string | null;
}

/** `history-removed` */
export interface HistoryRemoved {
  ids: string[];
}

/** `log-line` / `recent_logs` */
export interface LogLine {
  ts: number; // epoch ms
  level: string; // ERROR | WARN | INFO
  target: string;
  message: string;
}

/** `start_download` result. */
export interface StartResult {
  id: string;
  kind: "file" | "youtube";
  fileName: string;
}

/** One card in the dashboard: a file download, or a YouTube video+audio pair. */
export interface DownloadCard {
  id: string; // group id
  fileName: string;
  status: string;
  // Aggregated across video+audio jobs (for YouTube), or the single job.
  total: number;
  downloaded: number;
  speed: number;
  progress: number;
  eta: number;
  connections: number;
  isYoutube: boolean;
  speedLimit: number; // bytes/s, 0 = unlimited
  queuePosition?: number; // >0 while waiting in the queue
  sha256?: string | null;
  verified?: boolean | null;
  // Per-stream sub-jobs keyed by full job id.
  jobs: Record<string, ProgressPayload>;
  youtubeStatus?: string;
  error?: string | null;
  startAt?: number | null; // epoch seconds while scheduled
  headerNames?: string[]; // restored downloads: headers that were not saved
  priority?: Priority; // while waiting
}

// --- Browser bridge (bridge.rs) -------------------------------------------

/** `BridgeSettings` — editable via `set_bridge_settings`. */
export interface BridgeSettings {
  enabled: boolean;
  /** Allowed `chrome-extension://<id>` origins (32 letters a–p each). */
  allowedExtensionIds: string[];
  /** Browser requests wait as "pending-confirm" until accepted (not take-over). */
  askBeforeStart: boolean;
  /** Also confirm downloads the extension took over from Chrome (default off). */
  confirmTakeover: boolean;
}

/** `get_bridge_info` / `set_bridge_settings` / `regenerate_token` result and `bridge-status` event. */
export interface BridgeInfo {
  settings: BridgeSettings;
  listening: boolean;
  port: number | null;
  address: string | null; // "127.0.0.1:PORT" while listening
  error: string | null;
  token: string;
  defaultPort: number;
  /** Extension IDs allowed out of the box (no setup needed). */
  shippedExtensionIds: string[];
  protocol: number;
}

/** `bridge-request` event / `list_bridge_requests` — a browser download awaiting Accept/Dismiss. */
export interface PendingRequest {
  id: string;
  status: "pending-confirm";
  url: string;
  pageUrl: string | null;
  fileName: string;
  kind: "file" | "stream";
  mediaType: string | null;
  size: number | null;
  headerNames: string[]; // names only; values never leave the backend
  hasCookies: boolean;
  receivedAt: number; // epoch seconds
  source: "takeover" | "user-action" | "page-scan" | "command-line";
  quality: string | null;
}

/** `bridge-request-removed` */
export interface BridgeRequestRemoved {
  id: string;
}

// --- Components (components.rs / shell.rs) --------------------------------

/** One external tool (yt-dlp, QuickJS, FFmpeg). */
export interface ComponentStatus {
  id: string;
  name: string;
  pinnedVersion: string;
  installedVersion: string | null;
  /** Where it was found; null = missing everywhere. */
  source: "components" | "bundled" | "path" | null;
  size: number;
  license: string;
  purpose: string;
}

/** `get_components_status` result and `components-status` event. */
export interface ComponentsInfo {
  components: ComponentStatus[];
  missing: string[];
  missingBytes: number;
  settingUp: boolean;
  autoUpdate: boolean;
  error: string | null;
}

/** `component-progress` event. */
export interface ComponentProgress {
  id: string;
  name: string;
  phase: "downloading" | "verifying" | "installing" | "done" | "error";
  downloaded: number;
  total: number;
  error: string | null;
}

export type UpdateOutcome =
  | { result: "upToDate"; version: string }
  | { result: "updated"; from: string; to: string }
  | { result: "rolledBack"; reason: string }
  | { result: "skipped"; reason: string };

/** `ytdlp_version` / `check_ytdlp_update` result. */
export interface YtdlpInfo {
  version: string | null;
  outcome: UpdateOutcome | null;
}

// --- App settings + pairing ------------------------------------------------

/** `get_app_settings` / `set_app_settings`. */
export interface AppSettings {
  startWithWindows: boolean;
  startMinimized: boolean;
  trayNoticeShown: boolean;
}

/** `pairing-request` event / `get_pending_pairing`. */
export interface PairingPrompt {
  id: string;
  extensionId: string;
  browser: string;
}
