# IDM Replica — Project Overview & Engineering Document

> A cross-platform, high-performance download manager ("IDM killer") built with a
> **Rust + Tokio** engine and a **Tauri + React/TypeScript** desktop UI. It
> implements true Internet-Download-Manager-style **Dynamic File Segmentation**,
> and adds a YouTube dual-stream + FFmpeg muxing pipeline.

---

## Table of Contents

1. [What This Project Does](#1-what-this-project-does)
2. [How It Works — The Mechanics](#2-how-it-works--the-mechanics)
   - [2.1 The Dynamic Segmentation Algorithm](#21-the-dynamic-segmentation-algorithm)
   - [2.2 Concurrent Disk Writes Without Corruption](#22-concurrent-disk-writes-without-corruption)
   - [2.3 Resilience: Timeouts, Retries, Pause/Resume](#23-resilience-timeouts-retries-pauseresume)
   - [2.4 The YouTube Pipeline](#24-the-youtube-pipeline)
   - [2.5 The IPC Bridge (Rust ↔ React)](#25-the-ipc-bridge-rust--react)
3. [Architecture at a Glance](#3-architecture-at-a-glance)
4. [How It Was Built — Step by Step](#4-how-it-was-built--step-by-step)
5. [Data Structures & Key Code Walkthrough](#5-data-structures--key-code-walkthrough)
6. [What It Brings That Existing Tools Don't](#6-what-it-brings-that-existing-tools-dont)
7. [Who It Helps & Real-World Use Cases](#7-who-it-helps--real-world-use-cases)
8. [Future Improvements — The Roadmap](#8-future-improvements--the-roadmap)
9. [Known Limitations](#9-known-limitations)
10. [Glossary](#10-glossary)

---

## 1. What This Project Does

At its core, this app downloads files **faster than your browser** by opening
many parallel connections to the same file, each grabbing a different byte range,
and writing them all to disk simultaneously.

A browser typically downloads a file over a **single TCP connection**. On
high-latency or per-connection-throttled links (very common on CDNs and file
hosts), that one connection never saturates your bandwidth. This tool splits the
file into many ranges and downloads them at once — often **3–10× faster**.

What it offers to the user:

| Capability | Description |
|---|---|
| **Multi-connection downloads** | 1–16 parallel connections per file (user-controlled slider). |
| **Dynamic segmentation** | Connections that finish early *steal* work from slower ones, so every connection stays busy until the final byte. |
| **Live dashboard** | Real-time speed (MB/s), ETA, overall %, and a per-thread segment visualization. |
| **Pause / Resume / Cancel** | Full control over in-flight downloads. |
| **YouTube downloads** | Paste a watch URL → it fetches the highest-quality video and audio streams in parallel and muxes them into a single playable MP4. |
| **Cross-platform** | One codebase runs on Windows, macOS, and Linux via Tauri. |
| **Tiny footprint** | Tauri uses the OS's native webview instead of bundling Chromium, so the app is a few MB, not hundreds. |

---

## 2. How It Works — The Mechanics

### 2.1 The Dynamic Segmentation Algorithm

This is the heart of the project and the thing that makes it an "IDM clone"
rather than a naive parallel downloader. It runs in four phases.

**Phase 1 — Initialization (the probe).**
Before downloading anything, the engine sends a `HEAD` request to learn two
things:
- `Content-Length` → the total file size.
- `Accept-Ranges: bytes` → whether the server supports partial requests.

Some servers lie about or refuse `HEAD`, so there's a fallback: a tiny
`GET` with `Range: bytes=0-0`. If the server answers `206 Partial Content`, we
read the real size out of the `Content-Range` header. If the server supports no
ranges at all, the engine gracefully degrades to a **single-stream** download.

**Phase 2 — Pre-allocation.**
We create the destination file and immediately call `File::set_len(total_size)`.
This reserves the file's full size on disk *up front*. Why? Because we're about
to write to scattered byte offsets out of order. Without pre-allocation, the OS
would grow the file piecemeal and likely **fragment** it across the disk,
hurting both write and later read performance. Pre-allocation gives us one
contiguous extent.

**Phase 3 — Static spawning.**
The file is divided into `N` equal byte ranges (where `N` = the user's connection
count). For each range we spawn an async Tokio task ("worker") that issues a
`GET` with a `Range: bytes=START-END` header and streams the response body
straight to its byte offset on disk.

```
File: [============================================================] 100 MB
       |  chunk 0  |  chunk 1  |  chunk 2  |  chunk 3  |   (N = 4)
       0          25MB        50MB        75MB        100MB
```

**Phase 4 — Dynamic splitting (the clever part).**
Real-world connections finish at wildly different speeds. A naive downloader
that just waits for the slowest chunk wastes the connections that already
finished. IDM's trick — and ours — is to **redistribute work**:

> When a worker finishes its range, a central *coordinator* finds the chunk with
> the **most bytes still remaining**, splits it in half, shrinks that worker's
> target to the midpoint, and spawns a brand-new worker to download the second
> half.

```
Before split — chunk 1 is the laggard, chunk 0/2/3 are done:
       |  done  |##########  chunk 1 still has 18 MB left  ##########|

After split — coordinator halves chunk 1 and spawns a new worker:
       |  done  |### chunk 1 (now ends at midpoint) ###|### new chunk ###|
                                                       ^ new worker starts here
```

The mechanism that makes this safe and lock-free on the hot path:
- Each chunk's `current` (next byte needed) and `end` (target) are
  **`AtomicU64`**.
- A split is just `victim.end.store(midpoint)` — one atomic write.
- The victim worker re-reads `end` on every received network frame, so it
  naturally stops at the new, smaller boundary. No locks, no signaling, no
  interruption of its in-flight request.

This loop repeats every time *any* worker frees up, keeping all `N` connections
saturated until the remaining pieces are too small to be worth splitting
(`MIN_SPLIT_BYTES`, 1 MiB).

### 2.2 Concurrent Disk Writes Without Corruption

The hardest correctness problem in a multi-connection downloader: **many threads
writing to one file at once**. The obvious approach — wrap the file in a
`Mutex<File>` and `seek()` + `write()` under the lock — is *correct* but
*slow*: every write serializes on the lock, and a shared file cursor means you
must seek before every write.

This project uses a better approach: **OS positioned writes**.

- On Windows: `seek_write(buf, offset)` (from `std::os::windows::fs::FileExt`).
- On Unix: `write_at(buf, offset)` (from `std::os::unix::fs::FileExt`).

These write to an explicit offset **without touching the file's shared cursor**,
so two threads writing to two different regions can never clobber each other.
This is the same primitive (`pwrite`) that databases use for concurrent I/O.

To keep network tasks from blocking on disk, all writes are funneled through a
**single dedicated flusher thread**:
- Workers send `(offset, bytes)` over a **bounded** channel (depth 512).
- One `spawn_blocking` thread drains the channel and performs the positioned
  writes.
- The bounded channel provides natural **back-pressure**: if the disk can't keep
  up, workers slow down instead of ballooning RAM usage.

Because positioned writes are **idempotent** (writing the same bytes to the same
offset twice yields the same result), the tiny overlap that can occur right after
a split — where the old worker streamed a few bytes past the new midpoint before
noticing — is harmless. The new worker simply re-writes those identical bytes.

This is how we satisfy "strict thread safety" *and* "high-performance, low-memory
overhead" simultaneously.

### 2.3 Resilience: Timeouts, Retries, Pause/Resume

Networks drop. Servers hiccup. The engine handles it:

- **Read timeout, not total timeout.** The HTTP client uses a *read* timeout
  (30s) rather than a total-request timeout. A total timeout would kill a
  perfectly healthy chunk that simply takes several minutes; a read timeout only
  fires when the socket genuinely stalls.
- **Exponential-backoff retry.** A dropped connection or read timeout surfaces as
  a stream error. The worker retries from its *current* byte offset (not from
  scratch) with exponential backoff, up to 6 times, before marking the chunk
  failed.
- **Pause / Resume.** Setting a `paused` flag makes workers drop their
  connections and park on a `Notify`. Resuming wakes them, and they re-issue a
  ranged request from wherever they left off — so pause/resume is essentially
  "free" thanks to HTTP range support.
- **Cancel.** Sets a `cancelled` flag, wakes parked workers, drains the writer,
  and removes the partial file.

### 2.4 The YouTube Pipeline

YouTube doesn't serve a single file. High-resolution video and audio are
delivered as **separate adaptive (DASH) streams**, and the URLs are protected by
a frequently-changing signature cipher. The pipeline:

1. **Detect** — if the input URL matches a known stream site (YouTube, Vimeo,
   etc.), route it to the streaming pipeline instead of treating it as a direct
   file.
2. **Extract** — shell out to `yt-dlp` (the maintained `youtube-dl` fork) to
   resolve two *direct* googlevideo URLs: the best **video-only** stream and the
   best **audio-only** stream. (We use `yt-dlp` rather than a hand-rolled parser
   because YouTube's cipher logic changes weekly — a native parser rots fast.)
3. **Download in parallel** — both streams are downloaded **simultaneously**,
   and *each one* gets the full dynamic-segmentation treatment. So a 4K video
   download is two parallel jobs, each itself using up to 16 connections.
4. **Mux** — once both temp files are 100% complete, FFmpeg merges them with a
   **stream copy** (`-c:v copy -c:a copy`). This is a container-level remux: no
   re-encoding, no quality loss, negligible CPU, and it finishes in seconds even
   for huge files.
5. **Clean up** — the temporary video-only and audio-only files are deleted on
   successful mux, leaving a single playable `.mp4`.

### 2.5 The IPC Bridge (Rust ↔ React)

The Rust engine and React UI communicate over Tauri's IPC in two directions:

- **React → Rust (commands).** The UI calls Rust functions via `invoke()`:
  `start_download`, `pause_download`, `resume_download`, `cancel_download`,
  `default_save_dir`, `suggested_connections`.
- **Rust → React (events).** A coordinator task samples progress every ~400ms and
  `emit_all()`s a `download-progress` event containing total/downloaded bytes,
  smoothed speed, ETA, status, and a snapshot of every thread's segment. The UI
  listens with `listen()` and re-renders the dashboard. A separate
  `youtube-status` event reports the extract/mux lifecycle.

This streaming-events model means the UI is always a passive mirror of engine
state — there's no polling, and the UI stays responsive even while the engine
hammers 16 connections.

---

## 3. Architecture at a Glance

```
┌─────────────────────────────────────────────────────────────────┐
│                         Tauri Desktop Shell                       │
│                                                                   │
│  ┌───────────────────────────┐      ┌──────────────────────────┐ │
│  │   Frontend (WebView)      │      │     Backend (Rust)        │ │
│  │   React + TypeScript      │      │     Tokio async runtime   │ │
│  │                           │      │                           │ │
│  │  App.tsx                  │      │  main.rs   (commands/IPC) │ │
│  │   • URL input             │ invoke│  downloader.rs (engine)  │ │
│  │   • connections slider    │─────▶│  youtube.rs (yt-dlp)      │ │
│  │   • dashboard / bars      │      │  muxer.rs   (ffmpeg)      │ │
│  │                           │◀─────│                           │ │
│  │   listen("download-       │events│  emit_all("download-      │ │
│  │           progress")      │      │            progress")     │ │
│  └───────────────────────────┘      └─────────────┬────────────┘ │
└────────────────────────────────────────────────────┼─────────────┘
                                                       │
                    ┌──────────────────────────────────┼───────────────┐
                    ▼                  ▼                ▼               ▼
              Worker (chunk 0)   Worker (chunk 1)  ...Worker N    Coordinator
                    │                  │                │          (splits +
                    └──────────────────┴────────────────┘           emits)
                                       │
                                       ▼  (offset, bytes) over bounded channel
                            ┌──────────────────────┐
                            │  Dedicated flusher    │  positioned writes
                            │  thread (pwrite)      │  → contiguous file
                            └──────────────────────┘
```

**File map:**

```
F:\IDM_replica
├── index.html              # WebView entry
├── package.json            # Frontend deps + npm scripts
├── vite.config.ts          # Vite (port 1420, matches Tauri)
├── tsconfig.json
├── src/                    # ── FRONTEND ──
│   ├── main.tsx            # React bootstrap
│   ├── App.tsx             # Dashboard, commands, event listeners
│   └── styles.css          # Dark IDM-style theme
└── src-tauri/              # ── BACKEND ──
    ├── Cargo.toml          # Rust deps (tokio, reqwest, tauri, serde…)
    ├── build.rs
    ├── tauri.conf.json     # App config, allowlist, window
    └── src/
        ├── main.rs         # Tauri entry, commands, YouTube orchestration
        ├── downloader.rs   # THE ENGINE: segmentation, workers, flusher
        ├── youtube.rs      # yt-dlp extraction layer
        └── muxer.rs        # FFmpeg stream-copy mux layer
```

---

## 4. How It Was Built — Step by Step

This is the actual order in which the system was designed and assembled.

**Step 1 — Project scaffolding.**
Defined the dependency surface first: `Cargo.toml` (Tokio "full", reqwest with
`stream` + `rustls-tls`, Tauri, serde, bytes, anyhow, uuid) and `package.json`
(React 18, Vite, `@tauri-apps/api`, `@tauri-apps/cli`). Chose **rustls** over
native-TLS to avoid the OpenSSL build headache on Windows. Chose **Tauri v1** for
stability and the tiny-binary native-webview model.

**Step 2 — The data model.**
Designed the state types that everything else hangs off of:
- `Chunk` — one byte range, with `AtomicU64` `current`/`end` and `AtomicBool`
  `done`/`failed`. Atomics chosen so the streaming hot path is lock-free.
- `DownloadJob` — one file: its chunk list (behind a `tokio::Mutex`, touched only
  on split/snapshot), the HTTP client, pause/cancel flags, and an active-worker
  counter.
- `DownloadManager` — a registry (`HashMap<id, Arc<DownloadJob>>`) so UI control
  commands can find a live job by id.
- `ProgressPayload` / `ThreadProgress` — the serde-serializable shapes sent to
  the UI.

**Step 3 — The probe (Phase 1).**
Wrote `probe()`: HEAD first, then a `bytes=0-0` GET fallback, deriving total
size, range support, and a sanitized filename (from `Content-Disposition` or the
URL).

**Step 4 — Pre-allocation + cross-platform positioned writes.**
Wrote `preallocate()` (`set_len`) and the `#[cfg(windows)]` / `#[cfg(unix)]`
`pwrite_all()` helpers. This is the foundation for safe concurrency.

**Step 5 — The flusher.**
Built the single `spawn_blocking` writer task draining a bounded mpsc channel,
calling `pwrite_all`, and `sync_all()`-ing on completion.

**Step 6 — The worker (Phases 3).**
Wrote `run_worker()`: the ranged-request → `bytes_stream()` → clamp-to-`end` →
send-to-flusher → advance-`current` loop, with pause parking, cancel checks, and
backoff retry built in.

**Step 7 — The coordinator + dynamic splitting (Phase 4).**
Wrote `split_largest()` (find max-remaining chunk, atomically shrink, spawn tail)
and the `download_to_file()` `tokio::select!` loop that ticks progress on a timer
*and* tops concurrency back up whenever a worker signals it finished. This is
where all phases come together.

**Step 8 — The Tauri command layer.**
Wrote `main.rs`: shared `AppState` (manager + reqwest client), the `#[tauri::
command]` functions, and the default-dir / suggested-connections helpers. Wired
`emit_all` for the progress stream.

**Step 9 — The YouTube addendum.**
Added `youtube.rs` (yt-dlp extraction) and `muxer.rs` (FFmpeg copy-mux), then
orchestrated the dual-stream-download-then-mux flow in `main.rs::start_youtube`,
reusing the *same* segmentation engine for both streams.

**Step 10 — The React UI.**
Built `App.tsx`: URL input, connections slider (1–16), save-dir field, and a
dashboard that aggregates per-job progress events into cards with overall +
per-thread segment bars. YouTube video/audio sub-jobs are grouped under one card.

**Step 11 — Hardening pass.**
Reviewed for the bugs that actually bite: a borrow-checker conflict in
`split_largest`, a move-after-use of the YouTube parent id, the
non-resumable/unknown-size EOF completion path, `Unpin` pinning of the byte
stream, and switching the HTTP client from a total timeout to a read timeout.

---

## 5. Data Structures & Key Code Walkthrough

**The atomic chunk** — why atomics matter:

```rust
struct Chunk {
    id: usize,
    start: u64,
    current: AtomicU64, // next byte needed; advances as we write
    end: AtomicU64,     // target; a SPLIT shrinks this atomically
    done: AtomicBool,
    failed: AtomicBool,
}
```

A split is a single atomic store on `end`. The worker reads `end` every frame, so
it stops at the new boundary with zero coordination overhead. That's the whole
trick that makes dynamic segmentation cheap.

**The split decision** (simplified):

```rust
fn split_largest(job, chunks) -> Option<Arc<Chunk>> {
    // pick the live chunk with the most remaining bytes
    let idx = /* argmax of (end - current) over !done && !failed */;
    if best_remaining < MIN_SPLIT_BYTES { return None; }   // not worth it
    let mid = current + (end - current) / 2;
    victim.end.store(mid, Release);                          // shrink victim
    let tail = Arc::new(Chunk::new(new_id, mid, end));       // new worker's range
    chunks.push(tail.clone());
    Some(tail)
}
```

**The coordinator loop** (simplified) — ties progress + dynamic top-up together:

```rust
loop {
    tokio::select! {
        _ = ticker.tick() => {
            // sample progress, compute smoothed speed + ETA, emit_all(...)
            // break if complete or cancelled
        }
        _ = finished_rx.recv() => {           // a worker freed a slot
            while active < max_connections {
                match split_largest(...) {
                    Some(tail) => spawn_worker(tail),
                    None => break,
                }
            }
            // if nothing left running, decide complete vs error
        }
    }
}
```

---

## 6. What It Brings That Existing Tools Don't

| Tool | Limitation | This project's edge |
|---|---|---|
| **Browser downloaders** | Single connection; no segmentation; weak resume. | True multi-connection dynamic segmentation; robust ranged resume. |
| **IDM (Internet Download Manager)** | Windows-only, closed-source, paid. | Cross-platform, open, hackable; same core algorithm. |
| **`aria2`** | Powerful but CLI/daemon only; intimidating UI story. | Same multi-connection power with a clean native GUI and live per-thread visualization. |
| **`wget` / `curl`** | Single stream; no dynamic work-stealing; no GUI. | Parallel, self-balancing, visual. |
| **Electron-based managers** | 150–300 MB binaries, heavy RAM (bundled Chromium). | Tauri = few-MB binary, native webview, low RAM. |
| **`yt-dlp` (CLI)** | Best-in-class extraction but CLI; mux is a manual extra step. | Wraps yt-dlp, then downloads each stream *segmented* and auto-muxes — extraction + acceleration + merge in one click. |

**The unique combination** is the headline: most tools do *one* of these well.
This project fuses them:

1. **IDM-grade dynamic segmentation** (work-stealing, not just static chunks),
2. a **modern, lightweight native GUI** (Tauri, not Electron, not CLI),
3. **safe high-throughput concurrent disk I/O** (positioned writes + flusher),
4. and a **YouTube path that accelerates the streams themselves** (not just a
   thin yt-dlp wrapper — each adaptive stream is segmented and parallelized, then
   losslessly muxed).

Nothing in the common toolset gives you *all four* in a single, open,
cross-platform app.

---

## 7. Who It Helps & Real-World Use Cases

- **Anyone on a throttled/high-latency link** — CDNs and file hosts frequently
  cap per-connection speed; segmentation routes around it for big wins.
- **Researchers / data engineers** pulling large datasets, model weights, ISOs,
  or archives where single-stream speed is the bottleneck.
- **Developers** who want an open, auditable, scriptable alternative to IDM they
  can extend (it's just Rust + React).
- **Content archivists / educators** saving lectures and talks: paste a YouTube
  URL and get a single high-quality MP4, downloaded fast.
- **Low-spec / battery-conscious users** who don't want a 250 MB Electron app
  idling in the background — the Tauri binary is tiny and light on RAM.
- **Unreliable-network users** — the retry/resume logic means a dropped Wi-Fi
  connection doesn't restart a 4 GB download from zero.

---

## 8. Future Improvements — The Roadmap

Concrete, prioritized ideas for the next iterations.

> **Status legend:** ✅ shipped · ⬜ planned.

### Near-term (high value, contained scope)
1. ✅ **Resume across app restarts.** The coordinator periodically writes an
   atomic `.idm` sidecar (every chunk's `start`/`current`/`end`). A fresh
   `download_to_file` for the same file validates the sidecar (URL + size + that
   the on-disk file still matches) and restarts each worker from its saved
   `current`, topping concurrency back up via the normal split path. Sidecar is
   deleted on completion/cancel, kept on error. Covered by a test that proves
   no ranged request re-fetches the already-downloaded prefix.
2. ✅ **Integrity verification.** Opt-in SHA-256 + MD5 hashing of the finished
   file (off-runtime via `spawn_blocking`), compared against a user-supplied
   checksum; result surfaced in the UI. (`DownloadOptions::verify`.)
3. ✅ **Native Save-As dialog.** `pick_save_path` opens Tauri's native dialog
   (pre-filled name + dir) and feeds an explicit destination into the download;
   the directory field remains as a fallback.
4. ✅ **Per-download speed limiting (throttling).** Shared token-bucket
   `RateLimiter` awaited at the network-read step; live-adjustable via the
   `set_speed_limit` command and a UI slider. Zero overhead when unlimited.
5. ✅ **Queue & scheduling.** A `QueueManager` layer caps concurrent downloads
   (live-adjustable via `set_max_concurrent`), waits the rest, and auto-promotes
   as slots free; entries support an optional `start_at` epoch for scheduled
   starts. Queue position streams to the UI via a `queue-status` event; cancel
   works on both queued and running downloads.

### Mid-term
6. **Browser extension + protocol handler.** Capture downloads from the browser
   (intercept clicks, "download with IDM Replica" context menu) via a companion
   extension and a custom URL scheme.
7. **Adaptive connection count.** Auto-tune `N` based on measured per-connection
   throughput and diminishing returns, instead of a fixed slider.
8. ✅ **Auth / cookies / custom headers.** `DownloadOptions.headers` apply to
   every request (probe + workers) — Bearer tokens, `Cookie`, any header — for
   private/authenticated resources, exposed in an Advanced UI panel. (mTLS client
   certs remain future work.) Covered by a header-passthrough test.
9. ✅ **Mirror / multi-source downloads.** `DownloadOptions.mirrors` adds extra
   URLs serving the same file; workers spread ranges across primary + mirrors
   (by chunk id) to combine bandwidth. Covered by a two-server test asserting
   both sources serve ranges and the file reassembles byte-exact.
10. ✅ **Pluggable extractors.** `youtube.rs` is now a provider system: an
    `Extractor` trait (`detect` + async `extract`) with a `providers()` registry;
    `YtDlpExtractor` is the first implementation. A second site slots in by
    implementing the trait and registering it — the engine and muxer are
    untouched.

### Long-term / ambitious
11. **BitTorrent + protocol plugins.** Add torrent and possibly FTP/SFTP backends
    behind the same segmentation/queue UI.
12. **Segmented streaming preview.** Prioritize the first + last segments so video
    files become previewable/seekable before they finish.
13. **P2P-assisted CDN** or LAN cache sharing between instances.
14. **Mobile** (Tauri 2 supports iOS/Android) — bring the same engine to phones.
15. **Smarter mux/transcode options.** Optional re-encode presets, subtitle
    embedding, chapter metadata, and format selection in the UI.

### Engineering hardening
16. ✅ **Deterministic test suite** — an `axum` fixture server drives 6 tests
    against an in-memory `ProgressSink`: multi-connection split reassembly,
    no-ranges single-stream fallback, HEAD-refused GET probe fallback,
    reconnect-after-truncated-response, integrity SHA match, and cancel-removes-
    partial-file. (`cargo test`, all green.)
17. ✅ **Structured logging** (`tracing` + `tracing-subscriber`, `RUST_LOG`-driven)
    with spans/events at probe, split, retry, and completion — never on the
    per-frame hot path. (Debug panel still pending.)
18. ✅ **CI** — `.github/workflows/ci.yml`: fmt-check, clippy `-D warnings`, tests,
    `tsc --noEmit`, then a Windows/macOS/Linux Tauri build matrix.
19. ⬜ **Backoff/jitter tuning** and a configurable retry policy.

### Also shipped in this iteration
- ✅ **Engine ⟶ GUI decoupling** via the `ProgressSink` trait (the engine no
  longer imports Tauri at all; `main.rs` provides `TauriSink`).
- ✅ **Preflight checks** — `yt-dlp`/`ffmpeg` presence is verified with an
  actionable, OS-aware error *before* a YouTube job starts (`check_prerequisites`
  command + `youtube::preflight`).
- ✅ **Architecture docs** — code-derived Mermaid diagrams under
  `docs/architecture/`, plus a standing `CLAUDE.md`.

---

## 9. Known Limitations

Honest current constraints worth documenting:

- **Resume** now survives restarts via the `.idm` sidecar; re-initiating the
  same URL→destination picks up where it left off. (A UI list of resumable
  downloads is still future work — today resume triggers by re-entering the URL.)
- **Servers without range support** fall back to a single connection (no speedup
  possible — this is a protocol limit, not a bug).
- **YouTube requires `yt-dlp` and `ffmpeg` on PATH.** The app detects their
  absence and reports a clear error, but they aren't bundled.
- **Build prerequisite:** Tauri embeds icon assets at compile time; the repo
  needs icons generated (`tauri icon`) before the first build.
- **Checksum verification is opt-in** and runs only after a download completes
  (it does not stream-hash during download).
- **Windows recommends the MSVC Rust toolchain;** the GNU toolchain may work for
  `dev` but is not Tauri's officially tested path.

---

## 10. Glossary

| Term | Meaning |
|---|---|
| **Segmentation / chunking** | Splitting a file into byte ranges downloaded in parallel. |
| **Dynamic segmentation** | Re-splitting the largest remaining chunk when a connection finishes early — work-stealing. |
| **Range request** | An HTTP `GET` with `Range: bytes=start-end` asking for part of a file (`206 Partial Content`). |
| **Positioned write (`pwrite`)** | Writing to an explicit file offset without using/altering the shared file cursor — safe for concurrent writers. |
| **Back-pressure** | A bounded queue making fast producers slow down when a consumer (disk) can't keep up, bounding memory use. |
| **Mux / remux** | Combining separate audio + video streams into one container; with `-c copy` it's a lossless, no-re-encode operation. |
| **DASH** | Dynamic Adaptive Streaming over HTTP — how YouTube serves separate quality-tiered audio/video streams. |
| **IPC** | Inter-Process Communication — here, the Tauri command/event bridge between the React UI and Rust engine. |
| **Tokio** | Rust's async runtime; schedules all the concurrent download workers. |
| **Tauri** | Framework for building desktop apps with a Rust backend and a web frontend using the OS's native webview. |

---

*This document describes the project as implemented in `downloader.rs`,
`main.rs`, `youtube.rs`, `muxer.rs`, and `App.tsx`. For setup and run
instructions, see [README.md](README.md).*
