# CLAUDE.md — IDM Replica

Standing context for every session. Load this first. Based strictly on what
exists in this repo.

## Tech Stack

- **Backend:** Rust (edition 2021, `rust-version = 1.70`), Tokio (`full`),
  reqwest (`stream` + `rustls-tls`, default-features off), Tauri v1.6, serde,
  serde_json, bytes, anyhow, uuid (v4), num_cpus. Hashing via `sha2` + `md-5`.
  Logging via `tracing` + `tracing-subscriber`.
- **Frontend:** React 18 + TypeScript (strict), Vite (dev port **1420**),
  `@tauri-apps/api` v1.
- **Shells out to:** `yt-dlp` and `ffmpeg` — must be on `PATH` (YouTube path only).

## Architecture Invariants (DO NOT BREAK)

- **The streaming hot path is LOCK-FREE.** `Chunk.current` / `Chunk.end` are
  `AtomicU64`; a dynamic split is a single atomic `store` on `end`. Never
  introduce a lock (Mutex/RwLock) on the per-frame receive→write path.
- **All disk writes go through positioned writes** (`seek_write` on Windows,
  `write_at` on Unix) via the **single dedicated flusher thread** over a
  **BOUNDED** mpsc channel (depth 512). Never write to the file directly from a
  worker; never make the write channel unbounded.
- **Positioned writes must stay idempotent.** The small post-split overlap
  (old worker streams a few bytes past the new midpoint before reloading `end`)
  re-writes identical bytes on purpose — keep it safe, don't "optimize" it away.
- **The UI is a passive mirror.** State flows Rust→React **only** via
  `emit_all("download-progress" | "youtube-status")` events. No polling, no
  React→Rust state mutation beyond the explicit control commands.
- **Pause/resume/cancel** work via atomic flags + `Notify`; workers re-issue
  ranged requests from `current`. **Resume must never restart a chunk from byte
  0.** The same rule applies to cross-restart resume (the `.idm` sidecar).
- **Throttling gates at the network-read step, not the flusher.** A rate limiter
  must be awaited before consuming a network frame — never around a disk write.

## Conventions

- **Rust:** run `cargo fmt` and `cargo clippy -- -D warnings` before a change is
  "done". Prefer `anyhow::Result` for fallible fns. No `unwrap()`/`expect()` on
  I/O or network paths (engine init that runs once at startup may `expect`).
- **TS/React:** strict mode; no `any`; functional components only.
- **Cross-platform:** any OS-specific code needs both `#[cfg(windows)]` and
  `#[cfg(unix)]` paths (see `pwrite_all`).
- **Engine ⟶ UI decoupling:** the engine reports progress through a
  `ProgressSink` trait, not `AppHandle` directly, so it stays unit-testable
  without a GUI. The Tauri sink lives in `main.rs`; tests use an in-memory sink.

## Key files

| File | Responsibility |
|---|---|
| `src-tauri/src/downloader.rs` | The engine: segmentation, workers, flusher, resume, throttle, integrity. |
| `src-tauri/src/main.rs` | Tauri commands, IPC bridge, YouTube orchestration, preflight. |
| `src-tauri/src/youtube.rs` | Stream-site extraction (yt-dlp), behind the `Extractor` provider trait. |
| `src-tauri/src/muxer.rs` | FFmpeg `-c copy` mux + temp cleanup. |
| `src/App.tsx` | Dashboard: commands + event listeners + per-thread bars. |

## Build / Test / Run commands

Run from the **repo root** unless noted.

| Goal | Command |
|---|---|
| Typecheck frontend | `npx tsc --noEmit` |
| Build / check backend | `cargo check --manifest-path src-tauri/Cargo.toml` |
| Format Rust | `cargo fmt --manifest-path src-tauri/Cargo.toml` |
| Lint Rust | `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings` |
| Run engine tests | `cargo test --manifest-path src-tauri/Cargo.toml` |
| Run app (dev) | `npm run tauri dev` |
| Release bundle | `npm run tauri build` |

> First `cargo` build is slow (compiles Tauri + reqwest). Icons must exist under
> `src-tauri/icons/` for `generate_context!` to compile — generate with
> `npm run tauri icon app-icon.png`.

## Definition of Done (every task)

`cargo fmt` clean, `cargo clippy -- -D warnings` clean, `npx tsc --noEmit`
passes, the app builds in dev, and new logic has tests where feasible.
