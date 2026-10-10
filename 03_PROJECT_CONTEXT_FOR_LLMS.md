# IDM Replica — Complete Project Context (hand this file to any LLM)

> **Purpose.** This single file gives an AI assistant everything needed to
> understand the project, know exactly where it stands, and continue the work
> safely. It is self-contained: no other file is required to get oriented.
> **Snapshot date:** 2026-10-06. **Basis:** every source file was read in full; the
> code was **not compiled or run** during this audit, so "tests pass / CI green"
> claims below come from the repo's own docs, not from me. Where I infer something
> from reading code, it says so.
> **Owner:** Rehman (final-year Software Engineering student, NUST). Windows machine;
> repo at `F:\IDM_replica`.

---

## 1. One-paragraph summary

**IDM Replica** is a cross-platform desktop download manager ("IDM killer"). A
**Rust + Tokio** engine downloads a file over many parallel HTTP range-request
connections using IDM-style **dynamic segmentation** (when a connection finishes,
the largest remaining byte range is halved and the back half is handed to a new
worker), writing straight into a pre-allocated file via positioned writes through
one dedicated writer thread. A **Tauri v1 + React 18/TypeScript** UI shows live
speed/ETA/per-thread segment bars. It also has a YouTube path (yt-dlp resolves
separate video/audio streams → both downloaded in parallel with the same engine →
ffmpeg `-c copy` mux), plus resume across restarts, speed limiting, a download
queue with scheduling support, checksum verification, custom headers, and mirrors.

**Maturity:** a working, well-structured engine with a good deterministic test suite
(9 tests); a functional but minimal UI; and a roadmap that is mostly shipped on the
backend side. The biggest remaining gaps are UI-level (history, scheduling UI,
persistence), a few correctness/safety hardening items (Section 8), and packaging.

---

## 2. Tech stack (exact)

| Layer | Tech |
|---|---|
| Engine | Rust edition 2021, `rust-version = 1.70`, Tokio 1.37 (`full`) |
| HTTP | reqwest 0.12 (`default-features=false`; features `stream`, `rustls-tls`, `gzip`, `brotli`) |
| Desktop shell | Tauri **1.6** (features `dialog-all`, `shell-open`, `path-all`); `tauri-build` 1.5 |
| Serialization | serde + serde_json |
| Misc crates | futures-util, bytes, anyhow, uuid (v4), num_cpus, async-trait, sha2, md-5, tracing, tracing-subscriber (env-filter) |
| Dev-only | axum 0.7 (test fixture server) |
| UI | React 18.3, TypeScript 5.5 (strict, no unused locals/params), Vite 5.3 (port **1420**, strictPort), `@tauri-apps/api` ^1.6 |
| External binaries | `yt-dlp`, `ffmpeg` on PATH (YouTube path only; **not bundled**) |
| CI | GitHub Actions: lint-test (Ubuntu) then build matrix (Win/macOS/Ubuntu) |
| Release profile | opt-level 3, LTO, codegen-units 1, panic=abort, strip |

---

## 3. Commands (run from repo root)

| Goal | Command |
|---|---|
| Install JS deps | `npm install` |
| One-time icon generation (required before first Rust build) | `npx @tauri-apps/cli icon app-icon.png` (icons already exist in `src-tauri/icons/`) |
| Typecheck frontend | `npx tsc --noEmit` |
| Check backend | `cargo check --manifest-path src-tauri/Cargo.toml` |
| Format | `cargo fmt --manifest-path src-tauri/Cargo.toml` |
| Lint | `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings` |
| Tests | `cargo test --manifest-path src-tauri/Cargo.toml` |
| Run app (dev) | `npm run tauri dev` |
| Release bundle | `npm run tauri build` |
| Verbose logs | set `RUST_LOG=debug` (default `info`) |

**Definition of Done for any task:** `cargo fmt` clean, `cargo clippy -D warnings`
clean, `npx tsc --noEmit` passes, app builds in dev, new logic has tests where
feasible. First `cargo` build is slow. On Windows prefer the MSVC toolchain.

---

## 4. Repository map

```
CLAUDE.md                     standing rules (this file's invariants come from it)
README.md / PROJECT_OVERVIEW.md   human docs (PROJECT_OVERVIEW has the roadmap)
package.json, tsconfig*.json, vite.config.ts, index.html
.github/workflows/ci.yml
docs/architecture/            component.mmd, download-lifecycle.mmd,
                              segmentation-state.mmd, youtube-pipeline.mmd (+README)
src/main.tsx                  React bootstrap
src/App.tsx   (~740 lines)    entire UI: commands, event listeners, cards
src/styles.css (~435 lines)   dark theme
src-tauri/Cargo.toml, build.rs, tauri.conf.json, icons/
src-tauri/src/main.rs         (~650)  Tauri commands, queue, YouTube orchestration, entrypoint
src-tauri/src/downloader.rs   (~1700) THE ENGINE + 9 tests
src-tauri/src/youtube.rs      (~245)  Extractor trait + yt-dlp provider + preflight
src-tauri/src/muxer.rs        (~75)   ffmpeg -c copy mux
```

---

## 5. Architecture

### 5.1 Data flow

```
React (App.tsx) ──invoke(cmd)──▶ main.rs commands ──▶ QueueManager.promote()
                                                          │
                                       ┌──────────────────┴────────────┐
                                  plain file                       stream site (YouTube…)
                              download_to_file()          youtube::extract → 2× download_to_file
                                       │                    (tokio::join!) → muxer::mux
                                       ▼
   probe → (sidecar restore) → preallocate → flusher thread + N workers
        coordinator loop: 400 ms tick (emit progress, write .idm sidecar)
                          + on worker-finished: split_largest → spawn tail worker
React ◀──emit_all("download-progress" | "youtube-status" | "queue-status")──┘
```

### 5.2 The engine, precisely (`downloader.rs`)

- **`Chunk`**: `id`, `start`, `current: AtomicU64`, `end: AtomicU64` (exclusive),
  `done`/`failed: AtomicBool`. A split = `victim.end.store(mid)`.
- **`DownloadJob`**: `urls` (primary + mirrors), `headers`, `chunks: Mutex<Vec<Arc<Chunk>>>`
  (touched only on split/snapshot), `next_chunk_id`, `active` counter,
  `paused`/`cancelled` flags, `resume: Notify`, `limiter: RateLimiter`, shared `client`.
- **`DownloadManager`**: `Mutex<HashMap<id, Arc<DownloadJob>>>` so control commands
  can reach live jobs.
- **Probe**: `HEAD`; if size/ranges not confirmed, `GET Range: bytes=0-0` and parse
  `Content-Range: …/TOTAL` on 206. No ranges → single-stream fallback
  (`Chunk(0, 0, total or u64::MAX/2)`; unknown length completes on clean EOF).
- **Initial spawn**: `n = min(connections, max(1, total / 1 MiB))` equal chunks (last
  takes the remainder). `connections` is clamped to 1..=16.
- **Worker loop** (`run_worker`): check cancel → park while paused → compute
  `[current, end)` → pick source `urls[chunk.id % urls.len()]` → ranged GET →
  stream frames: re-check cancel/pause, reload `current`/`end`, clamp to `end`,
  `limiter.acquire(take)`, `write_tx.send(WriteCmd{offset: cur, data}).await`,
  `current.fetch_add(take)`, reset retries. Errors/timeouts → `should_retry`
  (backoff 250 ms·2^(n-1), exponent capped at 5, max 6 retries; real progress
  resets the counter) → reconnect from `current`.
- **Flusher**: one `spawn_blocking` thread draining a **bounded** `mpsc` (512) and
  calling `pwrite_all` (`seek_write` on Windows / `write_at` on Unix), then
  `sync_all()` when the channel closes.
- **Coordinator** (`download_to_file`): `tokio::select!` between a 400 ms ticker
  (compute smoothed speed (EMA 0.7/0.3), ETA, emit `download-progress`, write the
  `.idm` sidecar, detect completion/cancel) and `finished_rx` (a worker exited →
  while `active < max_connections`, `split_largest` and spawn; if nothing is running
  decide complete vs error).
- **Completion**: `downloaded >= total_size` (sum of `current - start`) or all
  chunks `done` with none failed. Then drain writer → unregister job → optional
  SHA-256/MD5 (spawn_blocking) → delete sidecar → emit `complete`. Cancel deletes
  the dest file + sidecar. Error keeps the sidecar (so it can resume).
- **Sidecar `.idm`**: JSON `{version:1, url, total_size, resumable, chunks:[{start,current,end}]}`
  written to `<dest>.idm.tmp` then renamed. Restored only if version/URL/size match
  and the existing dest file has exactly `total_size` bytes.
- **RateLimiter**: shared token bucket; `rate` atomic (0 = unlimited fast path);
  burst capped at ~1 s of budget; awaited **before** consuming a network frame.
- **HTTP client**: pool_max_idle_per_host 32, connect timeout 15 s, **read** timeout
  30 s (never a total timeout), UA `idm-replica/0.1 (+https://localhost)`.

### 5.3 Architecture invariants — DO NOT BREAK (from `CLAUDE.md`)

1. The per-frame receive→write path is **lock-free**. Never add a Mutex/RwLock there.
2. All disk writes use **positioned writes** via the **single flusher thread** over a
   **bounded** channel (depth 512). Never write from a worker; never unbound it.
3. Positioned writes must stay **idempotent** (the post-split overlap re-writes
   identical bytes by design). Don't "optimize" it away.
4. The UI is a **passive mirror**: Rust→React only via `emit_all` events; no polling;
   React→Rust only explicit control commands.
5. Pause/resume/cancel via atomic flags + `Notify`; workers re-issue ranged requests
   from `current`. **Resume must never restart a chunk from byte 0** (same for the
   `.idm` cross-restart path).
6. Throttling gates at the **network-read step**, not the flusher/disk.

### 5.4 Conventions

- Rust: `anyhow::Result` for fallible fns; **no `unwrap()`/`expect()`** on I/O or
  network paths (one-time startup `expect` is OK). OS-specific code needs both
  `#[cfg(windows)]` and `#[cfg(unix)]`.
- TS: strict; no `any`; functional components only.
- Engine ↔ UI decoupling: engine reports via `ProgressSink` trait (Tauri impl in
  `main.rs::TauriSink`; in-memory impl in tests). Do not import Tauri in
  `downloader.rs`.
- Logging: `tracing`; never inside the per-frame hot path.

---

## 6. IPC surface (the contract between UI and engine)

### 6.1 Commands (`invoke`) — registered in `main.rs::generate_handler!`

Tauri converts JS camelCase args to Rust snake_case.

| Command | Args | Returns | Notes |
|---|---|---|---|
| `start_download` | `url`, `connections`, `saveDir?`, `savePath?`, `speedLimit?` (bytes/s), `verify?`, `expectedChecksum?`, `headers?` (map), `mirrors?` (string[]), `startAt?` (epoch secs) | `{id, kind:"file"|"youtube", fileName}` | Enqueues; stream sites run `preflight` first. UI currently always sends `startAt: null`. |
| `pause_download` / `resume_download` / `cancel_download` | `id` | `()` | `id_variants` also targets `<id>::video`/`::audio`. Cancel removes a queued entry or signals the running job. |
| `set_speed_limit` | `id`, `bytesPerSec` | `()` | Live; 0 = unlimited. |
| `set_max_concurrent` | `max` | `()` | Clamped 1..=10; then promotes. Default 3. |
| `pick_save_path` | `defaultName`, `defaultDir?` | `string|null` | Native Save-As. |
| `check_prerequisites` | – | `()` / error string | yt-dlp + ffmpeg presence. |
| `default_save_dir` | – | `string` | OS Downloads dir. |
| `suggested_connections` | – | `number` | `(cores×2)` clamped 4..16. |

### 6.2 Events (`listen`)

- **`download-progress`** → `ProgressPayload` (camelCase): `id, fileName, url, total,
  downloaded, speed (B/s), eta (s, -1 unknown), progress (0..100), status
  (downloading|paused|muxing|complete|error|canceled), connections, activeThreads,
  speedLimit, threads[{id,start,current,end,progress,done,failed}], error?, sha256?,
  verified?`. YouTube jobs use ids `<uuid>::video` and `<uuid>::audio`.
- **`youtube-status`** → `{id, fileName, status (muxing|complete|error), error?}`.
- **`queue-status`** → `{id, fileName, status (queued|canceled), position (1-based; 0 = running/removed)}`.

**Rule when changing a payload:** update the Rust struct *and* the TS interface in
`App.tsx`, plus `docs/architecture/*.mmd` if the surface changes.

---

## 7. Status — what is DONE

**Core engine (all implemented in `downloader.rs`):** HEAD/range probe with fallback;
pre-allocation; N-way static spawn; dynamic splitting; lock-free atomics; single
flusher with bounded back-pressure; cross-platform positioned writes;
exponential-backoff reconnect from `current`; pause/resume/cancel; read-timeout
client; progress with smoothed speed + ETA; per-thread snapshots; single-stream
fallback for non-range servers.

**Roadmap features shipped (per `PROJECT_OVERVIEW.md` §8, confirmed present in code):**
1. Cross-restart resume (`.idm` sidecar) · 2. Integrity verification (SHA-256+MD5,
opt-in) · 3. Native Save-As dialog · 4. Speed limiting (token bucket, live-adjustable)
· 5. Queue + max-concurrent + scheduling *backend* (`start_at`) · 8. Custom headers/
cookies/auth · 9. Mirrors / multi-source · 10. Pluggable extractors (`Extractor`
trait) · 16. Deterministic test suite (axum fixture, in-memory sink) · 17. Structured
logging · 18. CI workflow · plus `ProgressSink` decoupling, YouTube preflight, Mermaid
architecture docs, and `CLAUDE.md`.

**YouTube path:** extract (yt-dlp: `bv*[ext=mp4]/bv*` video, `ba[ext=m4a]/ba/bestaudio`
audio, plus title) → two parallel segmented downloads → `ffmpeg -c copy -movflags
+faststart` → delete temps.

**UI:** URL box, Save-As, connections/speed/max-concurrent sliders, save-dir field,
verify + expected checksum, collapsible Advanced (headers, mirrors), per-download
cards with overall bar, speed, ETA, per-thread segment visualization, YT video/audio
sub-streams, integrity line, queue position, toasts.

**Tests (9, in `downloader.rs`):** split reassembly (8 conns); single-stream
fallback; HEAD-refused probe; truncated-first-response recovery; SHA-256 match;
cancel removes partial; resume from sidecar without re-fetching prefix; custom
headers forwarded; mirrors spread load & reassemble. *(Repo docs claim all green;
unverified here. `PROJECT_OVERVIEW.md` still says "6 tests" — stale.)*

**Not tested:** `main.rs` (queue/promote logic, command layer), `youtube.rs`,
`muxer.rs`, any frontend code.

---

## 8. Findings from this audit (read-only review — verify before fixing)

Ordered by importance. "Confidence" is how sure I am from reading, not from running.

### 8.1 Correctness / data-integrity risks

1. **Possible premature "complete" after a split (medium confidence).**
   Completion is `downloaded >= total_size`, where `downloaded = Σ(current − start)`.
   After `split_largest` shrinks a victim's `end`, the victim may already have
   `current > new end` (it streamed a few bytes past the midpoint; it's never rolled
   back). That overlap is counted in the sum, so `downloaded` can reach `total`
   slightly before the last bytes of the tail chunk are actually fetched; the
   coordinator then breaks, drops `write_tx`, and abandons the in-flight worker →
   a file with a short zero-filled tail reported as `complete`.
   *Fix direction:* define completion as "every chunk has `current >= end`" (or sum
   `min(current, end) − start` per chunk). Add a test that forces a split with
   injected latency. Preserve invariants 1–3.
2. **`gzip`/`brotli` enabled on a range-based client (medium).** If a server applies
   `Content-Encoding` to a ranged response, offsets/lengths refer to the encoded
   bytes while the engine counts decoded bytes. *Fix:* remove those reqwest features
   (and/or send `Accept-Encoding: identity` for download requests). Add a fixture
   behaviour that returns gzip.
3. **Mirrors aren't validated (medium).** Nothing checks that a mirror has the same
   size/ETag/Last-Modified, so a wrong mirror silently corrupts output. *Fix:* probe
   each mirror (HEAD / range probe), require equal `total_size`, drop mismatches,
   warn the user.
4. **Filename never comes from `Content-Disposition` (high confidence, UX bug).**
   `probe()` derives a smart file name, but `download_to_file` immediately overrides
   it with `dest.file_name()`, and `main.rs` builds `dest` from `sniff_name(url)`
   (last URL segment, no decoding, no sanitizing). Result: `?id=…` style URLs get
   poor names. *Fix:* probe before choosing the destination, or let the engine
   return/emit the real name and have `main.rs` rename.
5. **`sniff_name` is unsanitized (security, medium-high confidence).** The
   `sanitize()` in `downloader.rs` isn't used for `sniff_name`. On Windows a URL whose
   last segment contains `\` could produce a path with separators/`..` and escape
   the chosen folder. *Fix:* sanitize/normalize the name and reject path components
   (use `Path::file_name()`), also percent-decode first.
6. **Sidecar never records mirrors/headers (low).** Fine today (URL must match), but
   note it if resume semantics change.

### 8.2 Behavioural gaps

7. **Speed limit is per-stream for YouTube**, so a "5 MB/s" cap yields ~10 MB/s in
   total (video and audio each get their own limiter). *Fix:* share one limiter
   across both jobs, or halve it.
8. **YouTube path ignores `verify`, `expected_checksum`, `headers`, `mirrors`**
   (`run_youtube_pipeline` builds `DownloadOptions` with only connections and speed
   limit). Probably intentional; document or support it.
9. **Cancel during YouTube extraction can't stop yt-dlp** (no kill handle); three
   sequential yt-dlp spawns (video, audio, title) make extraction slow.
10. **Non-transient HTTP errors are retried like transient ones** (403/404 also
    consume up to 6 retries ≈ 16 s of backoff). Consider failing fast on 4xx
    except 408/429.
11. **Queue and schedules are in-memory only**; closing the app loses the queue
    and any `start_at` timers. The UI has no scheduling control and cards aren't
    persisted.
12. **Stale-sidecar behaviour:** a sidecar whose file is the right size but whose
    content was modified externally would be trusted. (Consider also storing
    ETag/Last-Modified.)

### 8.3 Security / config hardening

13. `tauri.conf.json` has `"csp": null` and allowlist entries `dialog.all`, `path.all`
    that the UI doesn't need (the Save-As dialog is invoked from Rust). Tighten to
    the minimum and add a CSP.
14. `User-Agent: idm-replica/0.1 (+https://localhost)` — fine for dev; set a real
    one for release.

### 8.4 Repo / CI hygiene

15. **`src-tauri/Cargo.lock` is git-ignored** though this is an application → commit it.
16. **CI uses `ubuntu-latest` with `libwebkit2gtk-4.0-dev`** (Tauri v1 needs 4.0);
    newer Ubuntu images ship 4.1 → pin `ubuntu-22.04` (likely CI failure; unverified).
17. Docs drift: `README.md` IPC section and `component.mmd` omit later commands/events;
    `PROJECT_OVERVIEW.md` says 6 tests (there are 9). Keep docs in sync.

---

## 9. What REMAINS — prioritised, with how to do each

Legend: **[E]** engine (`downloader.rs`) · **[M]** `main.rs` · **[U]** UI · **[X]** other.

### P0 — Fix before calling it release-quality
1. **Completion-race fix (8.1 #1)** [E] — change the completion predicate; add a
   regression test with a slow fixture. Keep hot path lock-free.
2. **Disable response compression for downloads (8.1 #2)** [E/Cargo] — drop
   `gzip`/`brotli`; add a test fixture.
3. **Sanitize/derive filenames (8.1 #4, #5)** [M/E] — central `safe_file_name()`;
   use Content-Disposition when no Save-As path is given.
4. **Validate mirrors (8.1 #3)** [E].
5. **CI + repo hygiene (8.4)** [X] — pin `ubuntu-22.04`, commit `Cargo.lock`,
   verify the pipeline is actually green.

### P1 — Product completeness
6. **Persistent download list / history** [M+U] — store job metadata (url, dest,
   status, timestamps) in a small JSON/SQLite file in the app data dir; reload on
   start; show "resumable" items so cross-restart resume doesn't require re-typing
   the URL (this is the known limitation in `PROJECT_OVERVIEW.md` §9). Respect the
   passive-UI rule: backend remains source of truth, emits state events.
7. **Scheduling UI** [U] — add a date/time picker that sets `startAt` (backend
   already supports it); persist timers across restarts (depends on #6).
8. **Open file / open folder / remove-from-list / retry buttons** [U+M] — use
   Tauri `shell.open` (already allowlisted) or a dedicated command.
9. **Pause/resume for queued entries, queue reordering, per-entry priority** [M+U].
10. **Shared speed limit for YouTube pairs (8.2 #7)** and **kill-able extraction
    (8.2 #9)** [M/youtube.rs].
11. **Fail-fast on non-transient HTTP errors + configurable retry policy/jitter**
    (roadmap #19) [E].

### P2 — Roadmap items not started (from `PROJECT_OVERVIEW.md`)
12. **Adaptive connection count** (auto-tune N from per-connection throughput) [E].
13. **Browser extension + custom URL protocol handler** (capture downloads from the
    browser) [X] — largest new surface; needs a native-messaging or deep-link design.
14. **Segmented streaming preview** (prioritise first+last segments) [E].
15. **BitTorrent / FTP / SFTP backends** behind the same queue/segmentation UI [E/X].
16. **Smarter mux options** (re-encode presets, subtitles, chapters, format picker UI)
    [youtube.rs/muxer.rs/U].
17. **Second `Extractor` provider** (native, no-subprocess) to prove the plug-in API [youtube.rs].
18. **mTLS client certs** (called out as future work under headers) [E].
19. **Debug panel** for tracing output (logging exists; panel pending) [U].
20. **Bundle yt-dlp/ffmpeg or auto-install** [X] — currently user-installed.
21. **Tauri 2 migration** (modern, enables mobile) and optional **mobile** targets [X].
22. **P2P/LAN cache** — long-term, speculative.

### P3 — Test & quality debt
23. Unit tests for `QueueManager::promote`/`cancel_download`/scheduling (extract the
    logic from `AppHandle` so it's testable, mirroring `ProgressSink`).
24. Tests for `youtube.rs` parsing/detection and `muxer.rs` (with a fake `ffmpeg`).
25. Frontend tests (aggregation reducer is a good first target — extract it from
    `App.tsx` into a pure function).
26. Refactor `App.tsx` (≈740 lines) into components/hooks; **keep** strict TS.
27. Update stale docs (README IPC list, `component.mmd`, test count).

---

## 10. How to work on this repo (instructions for an AI assistant)

1. **Read `CLAUDE.md` and Section 5.3 first.** Re-check each invariant against your
   diff before finishing.
2. **Make small, testable changes.** Prefer adding a deterministic test with the
   axum fixture (`Behavior` enum in `downloader.rs` tests) before/with an engine fix.
3. **Never** add a lock, unbounded channel, direct worker disk write, or
   throttle-around-disk in the engine. Never restart a chunk from byte 0 on
   pause/retry/resume.
4. **Keep the contract in sync:** Rust payload ⇄ TS interface ⇄ docs.
5. **Run the Definition of Done commands** (Section 3). Report which you actually ran.
6. **Don't claim things work unless run.** The initial audit that produced this file
   compiled and ran nothing.
7. **Windows-first dev environment;** keep both `cfg(windows)`/`cfg(unix)` paths.
8. **Ask before** changing public command signatures, the `.idm` sidecar format
   (bump `SIDECAR_VERSION` and keep old-version rejection), or Tauri major version.

### Suggested first tasks for a new assistant
(a) Run the full DoD to establish a verified baseline; (b) implement P0 #1 with a
regression test; (c) P0 #2 and #3; (d) update CI and commit `Cargo.lock`; (e) start
P1 #6 (persistent history).

---

## 11. Glossary (minimal)

*Range request*: `Range: bytes=a-b` → `206`. *Chunk/segment*: a byte range owned by
one worker. *Dynamic split*: halve the largest remaining chunk when a slot frees.
*Positioned write*: write at an explicit offset, no shared cursor (`seek_write` /
`write_at`). *Sidecar*: `<file>.idm` resume metadata. *Mux*: merge audio+video
streams. *IPC*: Tauri command/event bridge. *Sink*: `ProgressSink` trait that
decouples engine from GUI.
