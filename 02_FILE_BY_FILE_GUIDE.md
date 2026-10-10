# IDM Replica — Every File Explained

> **For:** someone new to the project (and to Rust/React/Tauri).
> **Method:** I read every file listed below in full, except where marked
> *(generated)* or *(binary)*. Line numbers refer to the files as they exist now
> (October 2026) and will drift as you edit — treat them as a map, not gospel.

---

## 0. The Map

```
IDM_replica/
├── CLAUDE.md                 ← standing rules for AI assistants working on the repo
├── README.md                 ← short setup/usage guide
├── PROJECT_OVERVIEW.md       ← long engineering document (design + roadmap)
├── package.json / package-lock.json   ← frontend dependencies & scripts
├── tsconfig.json / tsconfig.node.json ← TypeScript settings
├── vite.config.ts            ← dev server / bundler settings
├── index.html                ← the one HTML page the UI lives in
├── app-icon.png              ← source image for generating app icons
├── .gitignore                ← what Git should ignore
├── .github/workflows/ci.yml  ← automated checks on GitHub
├── docs/architecture/        ← 4 diagrams + a README describing them
├── src/                      ← FRONTEND (what you see)
│   ├── main.tsx
│   ├── App.tsx
│   └── styles.css
├── src-tauri/                ← BACKEND (the engine)
│   ├── Cargo.toml / Cargo.lock
│   ├── build.rs
│   ├── tauri.conf.json
│   ├── icons/                ← app icons
│   └── src/
│       ├── main.rs
│       ├── downloader.rs
│       ├── youtube.rs
│       └── muxer.rs
├── node_modules/             ← downloaded JS libraries (≈1,900 entries; never edit)
└── (not present yet, created by builds) dist/, src-tauri/target/
```

**How the pieces connect, in one sentence:** `index.html` loads `main.tsx`, which
draws `App.tsx` (the window); `App.tsx` calls Rust commands in `main.rs`;
`main.rs` hands real work to `downloader.rs` (and, for YouTube, `youtube.rs` +
`muxer.rs`); `downloader.rs` sends progress back to `App.tsx` through events.

---

## 1. Root-level files

### `CLAUDE.md` (≈4 KB)
Standing instructions for AI coding assistants (like Claude Code). It lists the tech
stack, the **architecture invariants** ("do not break" rules), code conventions, a
table of key files, the exact build/test commands, and the **Definition of Done**.
*Touch it when:* you change a rule, a convention, or a command. *Don't ignore it:*
the invariants in it are the reason the engine is fast and correct.

### `README.md` (≈5 KB)
The "getting started" page: features, project layout, prerequisites (Rust, Node 18+,
Tauri system deps, optional yt-dlp/ffmpeg), setup steps (`npm install`, generate
icons, `npm run tauri dev`), a short explanation of the algorithm, and the IPC
surface. **Note:** its IPC section is slightly out of date — it lists only
`start_download`, `pause/resume/cancel`, `default_save_dir`, and
`suggested_connections`, and only two events. The real code also has
`set_speed_limit`, `set_max_concurrent`, `pick_save_path`, `check_prerequisites`,
more `start_download` parameters, and a third event, `queue-status`.

### `PROJECT_OVERVIEW.md` (≈30 KB)
The long-form engineering document: what the project does, the mechanics, the
architecture diagram, how it was built step by step, key code walkthroughs,
comparison with other tools, use cases, a roadmap with ✅/⬜ status markers, known
limitations, and a glossary. It is the best narrative source of "why", but a few
details lag the code (for example it says the test suite has 6 tests; the code now
has 9, and the first ✅ roadmap entries are described as "shipped" — which matches
the code).

### `package.json` / `package-lock.json`
- `package.json` declares the frontend project: name `idm-replica`, version 0.1.0,
  ES-module project (`"type": "module"`).
  - **Scripts:** `dev` (start Vite), `build` (type-check then bundle), `preview`
    (preview the build), `tauri` (run the Tauri CLI → `npm run tauri dev`/`build`).
  - **Dependencies:** `react` and `react-dom` ^18.3.1, `@tauri-apps/api` ^1.6.0
    (the JavaScript side of Tauri: `invoke`, `listen`).
  - **Dev dependencies:** `@tauri-apps/cli` ^1.6.0, `typescript` ^5.5.3, `vite`
    ^5.3.4, `@vitejs/plugin-react`, and React type definitions.
- `package-lock.json` *(generated)* pins the exact version of every library so
  installs are reproducible. Don't edit by hand.

### `tsconfig.json` / `tsconfig.node.json`
TypeScript compiler settings. The main one targets ES2020, uses the modern React
JSX transform, is `strict`, flags unused variables/parameters, and only covers the
`src/` folder. `tsconfig.node.json` exists solely so `vite.config.ts` (which runs in
Node, not the browser) type-checks.

### `vite.config.ts`
Tells Vite to use the React plugin, run the dev server on **port 1420 with
`strictPort`** (fail rather than pick another port — Tauri expects 1420), not clear
the terminal screen, and output the production build to `dist/` (which matches
`tauri.conf.json → build.distDir`).

### `index.html`
Eight lines of real content: a blank `<div id="root">` and a script tag loading
`/src/main.tsx`. React fills that div. Title: "IDM Replica".

### `app-icon.png`
The source artwork. `npx tauri icon app-icon.png` generates every icon size into
`src-tauri/icons/`. *(binary)*

### `.gitignore`
Ignores `node_modules/`, `dist/`, `src-tauri/target/`, **`src-tauri/Cargo.lock`**,
editor folders, `.DS_Store`, and `*.log`. **Heads-up:** this is an *application*, and
Rust's guidance is to *commit* `Cargo.lock` for applications so everyone builds the
same dependency versions. Ignoring it works but makes builds less reproducible.

---

## 2. `.github/workflows/ci.yml` — automated checks

Runs on pushes to `main`/`master` and on every pull request; a newer run on the same
branch cancels the older one.

- **Job 1 `lint-test`** (Ubuntu): install Tauri's Linux system packages → set up
  Rust (with rustfmt + clippy) → set up Node 20 → cache Cargo → `npm ci || npm
  install` → `npx tsc --noEmit` → `cargo fmt --check` → `cargo clippy --all-targets
  -D warnings` → `cargo test`.
- **Job 2 `build`** (only if job 1 passes): builds the real app with `npm run tauri
  build` on Windows, macOS and Ubuntu (not fail-fast, so one OS failing doesn't
  hide the others).

*Watch-out:* it installs `libwebkit2gtk-4.0-dev`, which Tauri v1 needs, on
`ubuntu-latest`. Newer Ubuntu images ship 4.1 instead of 4.0, so this step may fail
there; pinning `ubuntu-22.04` is the usual fix. (I could not run CI to confirm —
this is from reading it.)

---

## 3. `docs/architecture/` — the diagrams

All are **Mermaid** text diagrams (render in GitHub or VS Code with a Mermaid
extension), derived from the code.

| File | What it shows |
|---|---|
| `README.md` | Describes the four diagrams and warns to keep them in sync with code. |
| `component.mmd` | Frontend ⇄ Tauri IPC ⇄ Rust modules ⇄ external tools (yt-dlp, ffmpeg, disk, network), with the command/event names on the arrows. *Out of date:* it doesn't show `set_max_concurrent`, `check_prerequisites`, or the `queue-status` event. |
| `download-lifecycle.mmd` | Sequence diagram of one download: probe → (resume restore) → preallocate → spawn → write loop → split → drain → optional hash → cleanup → completion event. |
| `segmentation-state.mmd` | State machine for one chunk: Running ↔ Splitting / Throttled / Retry / Paused → Done or Failed. |
| `youtube-pipeline.mmd` | Flowchart: detect → preflight → extract → two parallel downloads → mux → cleanup. |

---

## 4. Frontend — `src/`

### `src/main.tsx` (≈10 lines)
The entry point. Finds `#root` in `index.html` and renders `<App />` inside React's
`StrictMode` (which double-runs some things in development to catch mistakes),
and imports `styles.css`.

### `src/App.tsx` (≈740 lines) — the whole UI
One big file. Layout by line range:

| Lines | What's there |
|---|---|
| 1–66 | **Types** mirroring the Rust payloads: `ThreadProgress`, `ProgressPayload`, `YoutubeStatus`, `QueueStatus`, `StartResult`. Helpers `groupId()` / `streamLabel()` — a YouTube job's id looks like `<uuid>::video` or `<uuid>::audio`; the UI strips the suffix to group them into one card. |
| 68–117 | **Formatting helpers**: `fmtBytes`, `fmtSpeed`, `fmtEta`; `parseHeaders` (turns "Key: Value" lines into an object) and `parseMirrors` (one URL per line). |
| 119–141 | The `DownloadCard` shape: one visible card (aggregating a YouTube video+audio pair into one). |
| 143–175 | `App` component start: React **state** for the URL box, connection slider, save folder, speed limit, verify checkbox, expected checksum, Save-As path, max-concurrent, advanced panel text, and the map of cards. On first load it calls `suggested_connections` and `default_save_dir`. |
| 177–313 | **Event listeners**: `download-progress` (merges per-job progress into cards; sums sizes/speeds; picks an overall status), `youtube-status` (extract/mux lifecycle), `queue-status` (queued position/cancel). |
| 315–361 | `startDownload()` — calls the `start_download` command with all options, then immediately inserts a placeholder card so the UI feels instant. |
| 363–405 | `guessName`, `chooseSavePath` (opens the native Save-As via `pick_save_path`), `changeMaxConcurrent`, `control()` (pause/resume/cancel), `applySpeedLimit()` (pushes a new speed cap to every active download live). |
| 407–427 | Toast helper and derived totals (total speed, number active). |
| 429–598 | **The JSX layout**: header with global stats, URL input + Save As + Download buttons, sliders (connections, speed limit, max simultaneous), save-to field, verify-integrity options, a collapsible Advanced panel (headers, mirrors), the dashboard list or an empty state, and the toast. |
| 600–714 | `DownloadRow` — one card: file name with FILE/YT badge, Pause/Resume/Cancel buttons, overall bar, a meta line (bytes, status, speed, ETA, connections, speed cap), integrity line, and the **segment visualization** (each thread's slice drawn as a track positioned by its start/end with a fill for progress). |
| 716–741 | `statusText()` — turns status codes into human labels ("Muxing (FFmpeg)…", "Queued · #2", …). |

*Design note:* the UI is a **passive mirror** — it keeps no authoritative download
state, only what events tell it. *Known gaps:* it has a `startAt: null` hard-coded,
so there is no UI for scheduling yet; cards are in memory only (close the app and
the list is gone); no "open file/folder" or "remove from list" actions.

### `src/styles.css` (≈435 lines)
The dark theme. Starts with CSS variables (colors like `--bg: #0d1117`,
`--accent: #2f81f7`, `--green`, `--red`, `--amber`, radius, font "Segoe UI"), then
styles for the class names `App.tsx` uses: `.app`, `.topbar`, `.brand`, `.logo`,
`.composer`, `.controls`, `.card`, `.bar-outer/.bar-inner`, `.segments/.seg-track/
.seg-fill`, status colors, `.toast`, and so on. *(I read the top of the file and
matched the rest against the class names used in `App.tsx`.)*

---

## 5. Backend — `src-tauri/`

### `src-tauri/Cargo.toml` / `Cargo.lock`
- `Cargo.toml`: package `idm-replica` 0.1.0, edition 2021, min Rust 1.70; the
  dependency list explained in the *Technology* guide; `[dev-dependencies]` has
  `axum`; the `custom-protocol` feature (don't remove); and the aggressive
  `[profile.release]` settings (LTO, abort-on-panic, strip).
- `Cargo.lock` *(generated, ~128 KB)* pins exact crate versions. Currently
  git-ignored.

### `src-tauri/build.rs`
Three lines: `tauri_build::build()`. Runs before compiling, embedding Tauri's
config and icons into the app. This is why icons must exist before the first build.

### `src-tauri/tauri.conf.json`
App-level config: build hooks (`npm run dev` / `npm run build`), dev URL
`http://localhost:1420`, dist folder `../dist`, product name "IDM Replica",
**allowlist** (everything off except `shell.open`, `dialog.all`, `path.all`), one
window (1100×720, min 820×560, resizable), `security.csp: null` (no Content
Security Policy), bundle targets "all", identifier `com.idmreplica.app`, and the
icon list.

### `src-tauri/icons/` *(binary)*
The generated icon set: `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.ico`
(Windows), `icon.icns` (macOS), `icon.png`, and the Windows Store `Square*Logo.png`
sizes + `StoreLogo.png`.

### `src-tauri/src/main.rs` (≈650 lines) — the command layer

| Lines | What's there |
|---|---|
| 1–29 | Module declarations (`downloader`, `muxer`, `youtube`), `AppState { manager, queue, client }`, `DEFAULT_MAX_CONCURRENT = 3`. The first line hides the extra console window on Windows release builds. |
| 31–49 | `TauriSink` — implements `ProgressSink` by calling `emit_all("download-progress", …)`. `make_sink()` helper. |
| 51–160 | **The queue**: `QueueEntry` (everything needed to start a download), `QueueManager` (cap, running count, pending list), `promote()` (under one lock: check the cap, pick the next *ready* entry, reserve the slot; when it finishes, free the slot and promote again), and `run_entry()` (runs a plain file via `download_to_file`, or the YouTube pipeline). |
| 162–185 | `StartResult` and the default-download-folder helpers (`tauri::api::path::download_dir` with env-var fallbacks). |
| 187–314 | **`start_download` command**: validates the URL, decides stream-site vs file, resolves destination (explicit Save-As path wins), runs `preflight` for stream sites, builds the `QueueEntry`, and calls `enqueue()` (which also sets a sleep-timer for future-scheduled entries). `sniff_name()` guesses a filename from the URL. |
| 316–476 | **YouTube pipeline** (`run_youtube_pipeline`): extract streams, build hidden temp file names, run the video and audio `download_to_file` jobs concurrently with `tokio::join!`, then mux or clean up. Also `YoutubeStatus` + `emit_youtube_status`, `QueueStatus` + `emit_queue_status` / `emit_queue_positions`, and `sanitize_title`. |
| 478–574 | **Control commands**: `pause_download`, `resume_download`, `cancel_download` (removes from the queue if waiting, otherwise signals the live job), `set_speed_limit`, `set_max_concurrent` (clamped 1–10). A helper `id_variants()` lets one parent id control both its `::video` and `::audio` jobs. |
| 576–611 | `pick_save_path` (native Save-As dialog), `check_prerequisites`, `default_save_dir`, `suggested_connections` ((cores×2) clamped to 4–16). |
| 613–651 | `main()` — starts logging (`RUST_LOG`, default `info`), builds shared state, registers all 10 commands, runs Tauri. |

### `src-tauri/src/downloader.rs` (≈1,700 lines) — THE ENGINE

| Lines | What's there |
|---|---|
| 1–37 | Header docs and imports. |
| 39–53 | **Tunables**: `MIN_SPLIT_BYTES` = 1 MiB, `MAX_RETRIES` = 6, `WRITE_CHANNEL_DEPTH` = 512, `TICK` = 400 ms. |
| 59–124 | **IPC payloads**: `ThreadProgress`, `ProgressPayload`; the `ProgressSink` trait; `DownloadOptions` (connections, speed limit, verify, expected checksum, headers, mirrors). |
| 126–199 | **`RateLimiter`**: the token bucket (rate in an atomic; fast path when 0; burst capped to ~1 s of budget). |
| 205–343 | **Core state**: `Chunk` (atomic `current`/`end`, `done`, `failed`; `remaining()`, `snapshot()`), `WriteCmd`, `DownloadJob` (urls, headers, chunk list behind a mutex, counters, pause/cancel flags, `Notify`, limiter, client; `pause/resume/cancel/set_speed_limit`), and `DownloadManager` (registry id → job). |
| 345–466 | **Phase 1 – probe**: `probe()`; `derive_file_name()` (Content-Disposition, else last URL segment) and `sanitize()`. |
| 468–492 | **Phase 2 – `preallocate()`** (open without truncating; `set_len`). |
| 494–530 | **`pwrite_all()`** — the Windows (`seek_write`) and Unix (`write_at`) versions. |
| 532–581 | **Phase 4 – `split_largest()`**: choose the chunk with most remaining bytes (skip if < 1 MiB), compute the midpoint, atomically shrink the victim's `end`, create the tail chunk. |
| 583–741 | **`run_worker()`** (the heart): loop { honor cancel/pause → build ranged request → stream frames → clamp to current `end` → throttle → send `(offset, bytes)` to the writer → advance `current` }, plus `should_retry()` with exponential backoff. |
| 743–774 | **Integrity**: `to_hex()`, `hash_file()` (1 MiB buffer, SHA-256 + MD5). |
| 776–842 | **Resume sidecar**: `Sidecar`/`ChunkRec` types, `sidecar_path()` (`<file>.idm`), `write_sidecar()` (temp + rename), `load_sidecar()` (validates version, URL, size, existing file). |
| 844–861 | **`build_client()`** — the shared reqwest client (timeouts, pool, user-agent). |
| 863–1252 | **`download_to_file()`** — the orchestrator. Probe → check sidecar → preallocate → create the job and writer thread → spawn the initial workers (resumed layout, single-stream, or N equal chunks) → **coordinator loop** (`select!` between the 400 ms ticker — emits progress and saves the sidecar — and "a worker finished" — splits the largest chunk to keep connections full) → teardown (drain writer, unregister) → final handling for *complete* (optional hashing, delete sidecar), *canceled* (delete file + sidecar), or *error* (keep sidecar). |
| 1254–1312 | `spawn_worker()` and `emit_progress()` (computes percent and ETA, builds the payload). |
| 1314–1697 | **Tests** (9): a local `axum` fixture server with four behaviours (`Ranges`, `NoHead`, `NoRanges`, `DropFirst`) and tests for: multi-connection split reassembly; single-stream fallback; HEAD-refused probe fallback; recovery after a truncated first response; SHA-256 match; cancel removes the partial file; resume-from-sidecar without re-fetching the prefix; custom headers forwarded; mirrors spread load and reassemble. |

### `src-tauri/src/youtube.rs` (≈245 lines)
The extractor layer. `ExtractedStreams` (title + video/audio URLs and extensions);
the `Extractor` trait; `providers()` registry; `detect()`, `is_stream_site()`,
`extract()`; the `YtDlpExtractor` (host list: youtube.com, youtu.be, vimeo.com,
twitch.tv, dailymotion.com, tiktok.com, facebook.com/watch, twitter.com);
`preflight()` (checks both binaries, gives an OS-aware install hint);
and helpers that run `yt-dlp` — `resolve()` (format selectors `bv*[ext=mp4]/bv*`
for video and `ba[ext=m4a]/ba/bestaudio` for audio) and `fetch_title()`. Three
separate yt-dlp calls are made per video (video, audio, title).

### `src-tauri/src/muxer.rs` (≈75 lines)
`ffmpeg_available()` and `mux(video, audio, output)`. Runs
`ffmpeg -y -hide_banner -loglevel error -i video -i audio -c:v copy -c:a copy
-map 0:v:0 -map 1:a:0 -movflags +faststart output`, then deletes the two temp
files on success. (`+faststart` moves metadata to the front so the MP4 can start
playing before it's fully loaded.)

---

## 6. Folders you will see but never edit

- **`node_modules/`** — every JavaScript library (and its dependencies) that
  `npm install` downloaded: ~1,900 entries. Regenerate with `npm install`.
- **`dist/`** *(not yet present)* — the frontend production bundle made by
  `npm run build`.
- **`src-tauri/target/`** *(not yet present in the listing)* — Rust build output;
  gets very large. Created by `cargo`.

---

## 7. "Where do I go to change X?" quick reference

| I want to… | Edit this |
|---|---|
| Change how slices are split / how many connections | `downloader.rs` → `split_largest`, `download_to_file`, `MIN_SPLIT_BYTES` |
| Change retry behaviour | `downloader.rs` → `should_retry`, `MAX_RETRIES` |
| Change speed limiting | `downloader.rs` → `RateLimiter` |
| Add a new command the UI can call | `main.rs` (write `#[tauri::command]`, add to `generate_handler!`) then call it from `App.tsx` |
| Add a new progress field | `ProgressPayload` in `downloader.rs` **and** the interface in `App.tsx` |
| Add support for another video site | `youtube.rs` → new `Extractor` + register in `providers()` |
| Change how the dashboard looks | `App.tsx` + `styles.css` |
| Change window size / app name / icons | `tauri.conf.json`, `src-tauri/icons/` |
| Change dev port | `vite.config.ts` **and** `tauri.conf.json` (they must match) |
| Add a Rust library | `src-tauri/Cargo.toml` |
| Add a JS library | `npm install <pkg>` (updates `package.json`) |
| Change what CI checks | `.github/workflows/ci.yml` |
