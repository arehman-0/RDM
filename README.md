# IDM Replica — Dynamic Segmentation Download Manager

A cross-platform download manager (an "IDM killer") built with **Rust + Tokio**
(engine) and **Tauri + React/TypeScript** (UI). It implements true IDM-style
**Dynamic File Segmentation**, plus a YouTube dual-stream + FFmpeg muxing
pipeline.

## Install (Windows 10/11, 64-bit)

Download one installer (and `SHA256SUMS.txt`) from the CI artifacts / release page:

| Installer | Size | Use it when |
|---|---|---|
| `IDM-Replica-<version>-x64-lite-setup.exe` | ~1.7 MB | Normal case. On first run the app downloads yt-dlp, QuickJS and FFmpeg (~97 MB) and checks their SHA-256 before using them. |
| `IDM-Replica-<version>-x64-full-setup.exe` | ~66 MB | Offline PCs: the same tools are inside the installer. They move to the app's components folder on first run, so yt-dlp still updates itself. |

- Double-click; no admin rights needed (installs for your user in
  `%LOCALAPPDATA%\IDM Replica`). Start menu shortcut; desktop shortcut optional.
- Needs Microsoft Edge WebView2 (built into Windows 11 and updated Windows 10). If it is
  missing, the installer downloads Microsoft's small bootstrapper and installs it.
- Then add the browser extension: the installer's last page and the app's first-run
  screen offer the store pages (once published) or **Manual install (developer mode)**.
  Browsers only install extensions you approve; nothing is installed behind your back.
- Uninstall from Settings → Apps. You can choose to also delete the download history,
  settings and downloaded video tools. Your downloaded files are never deleted.

Check a download: `Get-FileHash .\IDM-Replica-*-setup.exe` and compare with `SHA256SUMS.txt`.

### FAQ

**Windows says "Windows protected your PC" / my antivirus warns.** The installers are not
code-signed yet (see `docs/SIGNING.md`), so SmartScreen doesn't know them. Click **More
info → Run anyway** after checking the SHA-256 against `SHA256SUMS.txt`. Some antivirus
products flag new unsigned download managers, and yt-dlp, heuristically. Report false
positives to the vendor; don't turn your antivirus off.

**The extension says IDM Replica isn't running / doesn't detect the app.** The app must be
running (it can be in the tray: closing the window keeps it running). Then: click the
extension's icon → **Connect to IDM Replica** → **Allow** in the app. If it still fails:
the app's *Browser integration* section shows the port it listens on (38917 by default)
and the extension's Options must use the same port; another program may hold that port.
A store-installed extension has a different ID than the developer build — if it isn't
in the app's allowed list yet, paste its ID under Browser integration → Advanced.

**YouTube stopped working.** Sites change; yt-dlp follows. The app checks for a new yt-dlp
once a day and after a failed extraction. To force it: *Video components* → **Check
now**. If that says it's up to date, try again in a day.

**Where are my settings and history?** `%APPDATA%\com.idmreplica.app` (history, settings,
bridge token) and `%LOCALAPPDATA%\com.idmreplica.app\components` (video tools).

## Building the installers

```powershell
npm ci; npm --prefix extension ci
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -Variant lite
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -Variant full   # fetches + verifies the pinned tools
powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1 -Installer release\IDM-Replica-0.1.0-x64-lite-setup.exe
```

Outputs go to `release\` with `SHA256SUMS.txt`. Tauri v1 NSIS (per-user), with the
template in `src-tauri/installer/installer.nsi` (Tauri's own, plus the extension page and
a cleaner uninstall). CI (`.github/workflows/ci.yml`) runs every test, builds both and
uploads them as artifacts; it never publishes. Store kit for the extension:
`extension/store/`. Signing: `docs/SIGNING.md`. Manual test plan:
`docs/CLEAN_INSTALL_TEST.md`.

## Features

- **Dynamic File Segmentation** — `HEAD` probe → pre-allocate → spawn `N` ranged
  workers → when a worker finishes early, the largest remaining chunk is split
  in half and a new worker takes the tail. Connections stay saturated to the
  last byte.
- **Concurrent positioned writes** — every worker writes straight to its byte
  offset via OS positioned I/O (`seek_write` / `write_at`) through one dedicated
  flusher thread. No shared file cursor, no `Mutex<File>` throughput cliff.
- **Resilient** — per-read timeouts, exponential-backoff retry on dropped
  connections, graceful pause/resume/cancel.
- **YouTube** — one `yt-dlp -J` call resolves the best video-only + audio-only
  streams; both download in parallel (each fully segmented); FFmpeg stream-copies
  them into one `.mp4` (`-c copy`, no re-encode) and the temp files are cleaned up.
  The speed limit is a **total** cap shared by both streams; cancel also stops
  `yt-dlp` mid-extraction (whole process tree).
- **Custom headers** (Advanced) — sent on every request, including for video
  sites: they are passed to `yt-dlp` (`--add-header`) and to both stream
  downloads. Mirrors and checksum verification apply to plain files only.
- **Live UI** — overall + per-thread segment bars, MB/s, ETA, connection slider
  (1–16), all streamed over Tauri IPC events.

## Project layout

```
F:\IDM_replica
├── index.html
├── package.json            # React / Tauri / Vite deps
├── vite.config.ts
├── tsconfig.json
├── src/                    # Frontend
│   ├── main.tsx
│   ├── App.tsx             # Composition + card command dispatch
│   ├── types.ts            # IPC payload types (mirror the Rust structs)
│   ├── components/         # AddDownloadForm, SettingsPanel, DownloadList,
│   │                       # DownloadCard, SegmentBars, EmptyState, LogPanel, Header
│   ├── hooks/              # useDownloadEvents, useTauriEvent, useSettings, useLogs, useToast
│   ├── lib/                # Pure logic + Vitest tests: cards (reducer), status, format, input
│   └── styles.css
└── src-tauri/              # Backend
    ├── Cargo.toml          # Tokio / reqwest / tauri / serde …
    ├── build.rs
    ├── tauri.conf.json
    └── src/
        ├── main.rs         # Tauri commands + IPC bridge + YouTube pipeline
        ├── bridge.rs       # Browser-extension HTTP API on 127.0.0.1 (BridgeHost trait)
        ├── downloader.rs   # The segmentation engine (core)
        ├── queue.rs        # Queue: concurrency cap, scheduling (QueueHost trait)
        ├── history.rs      # Persistence: downloads.json, restore rules (StateStore trait)
        ├── youtube.rs      # yt-dlp extraction layer
        ├── muxer.rs        # FFmpeg copy-mux layer
        ├── opener.rs       # Open file / show in folder (validated, id-only)
        ├── logs.rs         # tracing layer feeding the in-app log panel
        └── process.rs      # Spawning external tools (no console, tree kill)
```

## Prerequisites

- **Rust** (stable) and **Node 18+**.
- **Tauri system deps** — see https://tauri.app/v1/guides/getting-started/prerequisites
  (on Windows: WebView2, already present on Win 11; the MSVC build tools).
- **Tauri CLI:** `npm i -g @tauri-apps/cli` (or use the dev-dependency via `npx`).
- **Video tools** (yt-dlp, QuickJS, FFmpeg): nothing to install by hand — the app's
  component manager downloads the pinned, hash-checked versions (see *Components*).

## Setup & run

```bash
npm install

# One-time: generate the app icons Tauri embeds at build time.
# (Tauri's generate_context! requires the icon files referenced in
#  src-tauri/tauri.conf.json to exist.)
npx @tauri-apps/cli icon path/to/any-1024x1024.png

# Dev (hot-reload UI + Rust):
npm run tauri dev

# Production bundle:
npm run tauri build
```

> If you skip the icon step, `npm run tauri icon` (with the default Tauri logo)
> or dropping any PNG/ICO into `src-tauri/icons/` with the names listed in
> `tauri.conf.json` will satisfy the build.

### Release profile and benchmark

`[profile.release]` is `opt-level = "s"`, `lto = true`, `codegen-units = 1`,
`panic = "unwind"`, `strip = true` (exe: 5.15 MB). With `unwind`, a panic ends only the
task or thread it happened in (the download fails with an error; the app, the other
downloads and the bridge keep running). A panic on the main/UI thread still ends the app.

`scripts/bench-download.mjs` measures throughput through the real exe: it serves a file
from a local range server, hands the URL to the app over the bridge as a take-over, times
the download and checks its SHA-256.

```bash
npm run build
node scripts/bench-download.mjs --opt 3,s,z --size-mb 3000 --runs 3
```

Measured 2026-10-12 (3000 MB, 6 runs each, loopback): opt 3 ≈ 1045 MB/s, `"s"` ≈ 1055 MB/s,
`"z"` ≈ 905 MB/s (median).

## How the engine works (the IDM algorithm)

1. **Init** — `HEAD` (with a `Range: bytes=0-0` fallback) gets `Content-Length`
   and confirms `Accept-Ranges: bytes`. No range support → single-stream
   fallback.
2. **Pre-allocate** — `File::set_len(total)` reserves the full extent so the OS
   doesn't fragment the file as workers write scattered offsets.
3. **Spawn** — divide into `N` equal chunks; spawn `N` async workers, each
   streaming a `Range: bytes=start-end` request directly to disk.
4. **Dynamic split** — a worker that finishes signals the coordinator, which
   finds the chunk with the most *remaining* bytes and atomically shrinks its
   `end` to the midpoint while spawning a fresh worker on `[mid, old_end)`.
   `current`/`end` are `AtomicU64`, so the victim worker simply stops at the new
   boundary — no locks on the hot path.

### Thread-safety model

- Hot path (streaming) is lock-free: each worker only touches its own chunk's
  atomics.
- The chunk *list* is behind a `tokio::Mutex` touched only on split/snapshot.
- Disk writes are funneled to **one** flusher thread doing positioned writes, so
  two writers can never corrupt each other's region, and writes are idempotent
  (a re-fetched overlap after a split just rewrites identical bytes).

## IPC surface

Commands (`invoke`):
- `start_download({ url, connections, saveDir?, savePath?, speedLimit?, verify?,
  expectedChecksum?, headers?, mirrors?, startAt? })` → `{ id, kind, fileName }`
  (`startAt` = epoch seconds for a scheduled start)
- `pause_download({ id })`, `resume_download({ id })`, `cancel_download({ id })` — on a
  *waiting* entry, pause/resume hold/release it in its queue position
- `set_speed_limit({ id, bytesPerSec })`, `set_max_concurrent({ max })`
- `list_history()` → `JobRecord[]` — every persisted download (called once on mount)
- `remove_history_entry({ id })` — forget a finished/failed/interrupted download
  (files on disk are left alone)
- `clear_completed()` → removed ids — forget every completed/canceled download
- `retry_download({ id })` — restart an interrupted/failed/canceled download; a
  plain file resumes from its `.idm` sidecar
- `move_in_queue({ id, delta })` — reorder a waiting entry (within its priority band)
- `set_queue_priority({ id, priority: "high"|"normal"|"low" })` — higher starts first
- `open_download({ id, target: "file"|"folder" })` — open the finished file / show it in
  its folder. Takes only an id: the path comes from the history record and must be
  inside that download's folder; programs/scripts are only revealed, never opened
- `recent_logs()` → `LogLine[]` — backlog for the log panel
- `pick_save_path({ defaultName, defaultDir? })`, `check_prerequisites()`,
  `default_save_dir()`, `suggested_connections()`

Events (`listen`):
- `download-progress` → `ProgressPayload` (per job: bytes, speed, ETA, %, status,
  per-thread segments, optional `warnings`)
- `youtube-status` → mux/extract lifecycle for grouped YouTube downloads
- `queue-status` → `{ id, fileName, status: queued|scheduled|held|starting|canceled,
  position, startAt?, priority, held }`
- `history-removed` → `{ ids }`
- `log-line` → `{ ts, level, target, message }` (INFO+ from the app, WARN+ from libraries)

Browser bridge (`bridge.rs`) — commands:
- `get_bridge_info()` / `set_bridge_settings({ settings: { enabled, allowedExtensionIds,
  askBeforeStart } })` / `regenerate_token()` → `BridgeInfo { settings, listening, port,
  address, error, token, defaultPort, protocol }`
- `list_bridge_requests()` → `PendingRequest[]`; `accept_bridge_request({ id })`,
  `dismiss_bridge_request({ id })`

Browser bridge — events: `bridge-status` → `BridgeInfo`; `bridge-request` →
`PendingRequest { id, status: "pending-confirm", url, pageUrl, fileName, kind, mediaType,
size, headerNames, hasCookies, receivedAt }`; `bridge-request-removed` → `{ id }`.

### Browser bridge HTTP API (for the extension)

A localhost-only HTTP server inside the app (`127.0.0.1`, port **38917**, or the
next free one up to 38927 — shown in *Browser integration*). Every request except
`GET /v1/ping` needs `X-IDM-Token: <token>` (Copy token in the panel; stored in
`bridge.json` in the app data dir). A request with an `Origin` header is refused
unless it is `chrome-extension://<allowed id>`; bodies are capped at 64 KB; only
http(s) URLs are accepted. Cookies/headers are used for that one job and never
logged or saved.

```bash
curl -s http://127.0.0.1:38917/v1/ping
```

```bash
curl -s -H "X-IDM-Token: $TOKEN" -H "Content-Type: application/json" -d '{"url":"https://example.com/file.zip","referer":"https://example.com/","fileName":"file.zip"}' http://127.0.0.1:38917/v1/download
```

Endpoints:
- `GET /v1/ping` → `{app, version, bridge:1}`. No token needed.
- `GET /v1/auth` → `204` if the token is right. It has no side effects.
- `POST /v1/pair` → one-click pairing, no token needed. It only works from an allowed
  extension Origin; the ID is taken from the `Origin` header, never from the body. The
  app shows *"Allow <browser> extension to send downloads to IDM Replica?"*.
  **Allow** → `{ok:true, token}`. **Deny**, or no answer within 2 minutes →
  `pairing_denied`. Only one prompt at a time (`pairing_pending`), and at most 3
  attempts per minute (`pairing_rate_limited`).
- `POST /v1/download` takes `{url?, pageUrl?, referer?, userAgent?, cookies?, headers?, fileName?, kind?: "auto"|"file"|"stream", mediaType?, size?, quality?, source?}`.
  It returns `{ok:true, id, fileName, queued, status: "queued"|"pending-confirm", duplicate}`.
- `POST /v1/batch {items:[…]}` → `{ok, results:[…]}`. Validation is all-or-nothing, and each item gets its own result.

Field rules:
- **`kind:"stream"`**: either a known video page (YouTube, Vimeo, X, Facebook
  watch/reel/videos/fb.watch, Instagram posts/reels, Reddit posts/v.redd.it, Twitch
  VODs and clips, Dailymotion, TikTok), or a direct HLS/DASH manifest (`.m3u8` /
  `.mpd`, or an HLS/DASH `mediaType`).
  - yt-dlp downloads the manifest itself as one job. It sends Referer, User-Agent and
    Cookie with `--add-header`. Cancel kills yt-dlp and its children.
  - With `kind:"auto"`, a manifest URL also takes this path, so it is never saved as
    playlist text.
- **`quality`**: `"best"` (default), `"audio"`, or a height (`"1080"`, `"720p"`,
  144–4320). It applies to stream jobs.
- **`source`** (default `"user-action"`):
  - `"takeover"`: Chrome's own download, handed over by the extension. It starts at
    once, without a confirm card. This only applies when the request carries the
    allowed extension's `Origin` (otherwise it counts as a user action), and only for
    up to 20 starts per 10 s. Beyond that, take-overs wait as pending-confirm. The
    setting *Confirm browser take-over downloads* (default off) makes every take-over
    wait.
  - `"user-action"` (floating button, context menu, popup) and `"page-scan"` ("download
    all") follow *Ask before starting*. At most 50 of them can wait at once (`app_busy`
    after that).
  - `"command-line"`: a URL passed to a second app launch, which always asks.
- The same URL arriving again within 5 s gets the first result back with `duplicate:true`.
- Every filename hint goes through `safe_file_name`. The engine refuses to write
  outside the download folder, and a yt-dlp output outside it is rejected.

Error codes (every error is `{ok:false, code, message}`, plus `error` with the same text):

| code | HTTP | when |
|---|---|---|
| `bad_token` | 401 | `X-IDM-Token` missing or wrong |
| `origin_not_allowed` | 403 | `Origin` isn't an allowed `chrome-extension://<id>` |
| `bad_host` | 403 | `Host` isn't 127.0.0.1/localhost (DNS rebinding) |
| `bad_request` | 400 | malformed JSON, unknown field, bad header/quality/source |
| `bad_url` | 400 | URL missing, not http(s), or malformed |
| `unsupported_site` | 400 | `kind:"stream"` for a page yt-dlp isn't set up for |
| `unsupported_media_type` | 415 | body isn't `application/json` |
| `payload_too_large` | 413 | body over 64 KB |
| `app_busy` | 429 | too many requests waiting for confirmation |
| `ytdlp_missing` / `ffmpeg_missing` | 503 | the stream path needs a tool that isn't installed |
| `js_runtime_missing` | 503 | a video page needs a JavaScript runtime for yt-dlp and none was found (QuickJS from the component manager, or `deno` / `node` on `PATH`). Not checked for `.m3u8` / `.mpd` manifests |
| `drm_protected` | — | reported on the job (its error text), not in the HTTP response: DRM is found only while yt-dlp runs |
| `pairing_denied` | 403 | the user clicked Deny on the pairing prompt, or didn't answer |
| `pairing_pending` | 409 | a pairing prompt is already on screen |
| `pairing_rate_limited` | 429 | more than 3 pairing attempts in a minute |
| `not_found` | 404 | unknown endpoint |
| `internal` | 500 | anything else |

The extension IDs in `bridge::shipped_extension_ids()` are always allowed: the development
build's fixed ID (also the unpacked copy the installer ships), plus the Chrome Web Store
and Edge Add-ons IDs from `src-tauri/extension-store.json` (both `null` until the extension
is published; the stores assign their own IDs — see `extension/store/README.md`). The
settings field only adds IDs for custom builds.

Only one copy of the app runs at a time. A second launch focuses the existing window,
hands over any `http(s)` URL it was given (as a confirm card), and exits. As a second
safeguard, an OS file lock on `app.lock` in the data dir is taken before any file is
opened. The OS releases it on a crash, so a stale lock never blocks startup.

## Components (yt-dlp, QuickJS, FFmpeg)

Video downloads need three external tools. Nothing has to be installed by hand:

- **One resolver** (`components.rs`) finds each tool in this order: the per-user folder
  `%LOCALAPPDATA%\com.idmreplica.app\components\`, then a copy bundled next to the exe,
  then `PATH`. `youtube.rs`, `muxer.rs`, `stream_dl.rs` and the prerequisite check all
  use it. Every spawn uses `CREATE_NO_WINDOW`.
- **Pinned manifest**: `src-tauri/components.json` (embedded at build time) is the only
  place with versions, URLs and SHA-256 hashes.
- **First run**: if a tool is missing everywhere, the *Setting up components* screen
  downloads it (resumable), verifies the SHA-256, and only then moves it into place.
  A wrong file is deleted. Retry continues an interrupted download.
- **yt-dlp needs a JavaScript runtime for YouTube** (per its current docs). The app
  installs QuickJS-NG (2 MB) and passes `--js-runtimes quickjs:<path>` and
  `--ffmpeg-location <dir>` on every yt-dlp call. Without QuickJS it falls back to
  `deno` on `PATH` (yt-dlp's default), then `node` on `PATH` (`--js-runtimes node`).
  The preflight for a video page checks, in this order, yt-dlp, the JavaScript runtime
  and ffmpeg, and reports the first one missing (`ytdlp_missing`, `js_runtime_missing`,
  `ffmpeg_missing`).
- **yt-dlp updates**: checked at most once a day (and after a failed extraction, at most
  every 10 minutes). The new exe must match the release's `SHA2-256SUMS` and pass
  `--version`; otherwise the previous one is restored. Toggle: *Keep yt-dlp up to date*.

Commands: `get_components_status()`, `install_components()`, `ytdlp_version()`,
`check_ytdlp_update()`, `set_ytdlp_auto_update({ enabled })`.
Events: `components-status` → `ComponentsInfo`, `component-progress` →
`{ id, name, phase, downloaded, total, error }`.

## Tray and startup

- Closing the window hides the app to the system tray, so browser take-over keeps
  working. The first time, a message owned by the window explains this, and the window
  hides when you click OK. Tray menu: Open, Pause all, Resume all, Quit.
- Test hook (off unless the running app has `IDM_REPLICA_TEST_HOOKS` set in its
  environment): a second launch with `--test-tray=pause_all|resume_all|quit|open` runs
  that tray menu item. It exists because the Windows 11 tray can't be driven by UI
  automation.
- **Quit** pauses every engine job, waits for the resume sidecars to be written, kills
  yt-dlp processes, saves the history, then exits.
- *Start with Windows* (default off) adds a per-user value under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` and removes it when turned off.
  *Start minimized to the tray* applies to that login start.

Commands: `get_app_settings()`, `set_app_settings({ settings: { startWithWindows,
startMinimized } })`, `get_pending_pairing()`, `answer_pairing({ id, allow })`.
Events: `pairing-request` → `{ id, extensionId, browser }`, `pairing-closed` → `{ id }`.

## Persistence

Downloads, the queue and schedules survive closing the app. State lives in one
JSON file, `downloads.json`, in the app data dir (Windows:
`%APPDATA%\com.idmreplica.app\`), replaced atomically (tmp + fsync + rename)
at most about once a second while things change, and on exit. On relaunch:

- downloads that were running (or waiting without a schedule) show as
  **Interrupted — resumable**; **Resume** continues a plain file from its `.idm`
  sidecar (never from byte 0). YouTube jobs re-extract, because their direct
  stream URLs expire; their stream downloads start over.
- scheduled downloads come back by themselves: future ones wait for their time,
  overdue ones start under the normal concurrency cap.
- an unreadable `downloads.json` is renamed `downloads.json.corrupt-<time>` and
  the app starts with an empty list.

**Sensitive data:** custom header *values* (auth tokens, cookies) are never
written to disk — only the header *names*. A restored download that needed
headers must be added again with them. URLs and mirror URLs *are* stored
(resume needs them; the `.idm` sidecar stores the URL too), so a pre-signed URL
carrying a token is kept until you remove the entry.
