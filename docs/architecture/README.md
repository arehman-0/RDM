# Architecture Diagrams

Mermaid diagrams derived from the actual source. Render them in any
Mermaid-aware viewer (GitHub renders `.mmd` in fenced blocks; VS Code with the
Mermaid extension renders `.mmd` directly).

## 1. `component.mmd` — Component / IPC graph
The big picture: how `App.tsx` talks to the Rust backend over Tauri IPC, how the
backend modules relate, and which external processes (`yt-dlp`, `ffmpeg`) and OS
resources (disk, network) they touch. Edges are labeled with the concrete
`invoke()` command names and `emit_all()` event names. Note the engine reports
progress through the `ProgressSink` trait rather than `AppHandle` directly — that
indirection is what keeps the engine unit-testable without a GUI.

## 2. `download-lifecycle.mmd` — Download sequence
A single download end-to-end: probe → (resume restore) → preallocate → spawn N
workers → flusher writes → coordinator split loop → drain/sync → (optional
integrity hash) → sidecar cleanup → terminal event.

## 3. `segmentation-state.mmd` — Chunk state machine
The lifecycle of one `Chunk`: Running ↔ Splitting (the atomic `end` store),
Running ↔ Throttled (token bucket), Running ↔ Retry (backoff), Running ↔ Paused,
and the terminal Done / Failed states. The hot path stays lock-free.

## 4. `youtube-pipeline.mmd` — Adaptive-stream pipeline
Detect (provider trait) → preflight (binaries present?) → extract two direct
URLs → download both in parallel (each fully segmented) → FFmpeg copy-mux →
cleanup → single playable MP4.

## 5. `persistence-restore.mmd` — Surviving a restart
Job statuses as stored in `downloads.json` (`history.rs`), what
`history::restore()` turns them into on relaunch (interrupted / scheduled /
queued), and how `retry_download` resumes an interrupted file from its `.idm`
sidecar. The sidecar only records bytes the flusher has already written, so a
kill can't leave holes.


## 6. `browser-bridge.mmd` — Browser extension → app
One `POST /v1/download` through `bridge.rs`: Host / Origin / token checks in the
`guard` middleware, body and field validation, then either a pending-confirm
request (Accept/Dismiss in the UI) or an immediate start through the same
`enqueue_download` path the UI uses.

---

> These diagrams are **code-derived**. If you change module boundaries, IPC
> commands, or the chunk state machine, update the corresponding `.mmd` so the
> docs don't drift.
