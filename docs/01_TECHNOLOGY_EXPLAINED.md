# IDM Replica — The Technology, Explained From Zero

> **Who this is for:** you, with no prior knowledge of Rust, networking, or
> desktop-app frameworks. Every term is explained the first time it appears.
> **How to read it:** top to bottom. Part 1 builds the mental model, Part 2 explains
> every technology and *why it was chosen*, Part 3 explains the clever algorithms,
> Part 4 is an honest list of trade-offs.
>
> Written from reading every source file in `F:\IDM_replica` (October 2026).

---

## Part 1 — The Big Idea (no jargon yet)

### What problem does this app solve?

When you download a file in a normal browser, the browser opens **one connection**
to the server and pulls the file through it, start to finish. Think of a single
delivery truck driving one road. On many servers (and on long-distance links),
a single connection is *artificially slow* — the server or the network limits each
connection, not your total internet speed. So your 100 Mbps line might only get
10 Mbps out of one connection.

**IDM** ("Internet Download Manager", a famous Windows program) fixes this by
opening **many connections at once**, each fetching a *different slice* of the same
file, then gluing the slices together on disk. Ten trucks, ten roads, same
warehouse. This project, **IDM Replica**, is your own cross-platform version of
that idea, written from scratch.

### The one trick that makes it "IDM-grade": dynamic segmentation

If you split a 1 GB file into 8 equal slices and give one to each connection, some
connections finish early (fast road) and sit idle while a slow one is still
working. That wastes your bandwidth.

IDM's trick (and this project's core feature): **when a connection finishes,
take the slice that has the most work left, cut it in half, and give the back half
to the idle connection.** Every connection stays busy until the very last byte.
This is called *dynamic segmentation* or *work stealing*. Almost all the interesting
engineering in this project exists to make that trick **fast and safe**.

### The pieces, in one picture

```
 You paste a URL
        │
        ▼
 ┌──────────────────────┐   commands (invoke)   ┌───────────────────────────┐
 │  The window (UI)     │ ────────────────────▶ │  The engine (Rust)        │
 │  React + TypeScript  │                       │  splits file, runs N      │
 │  shows bars, speed   │ ◀──────────────────── │  connections, writes disk │
 └──────────────────────┘   events (progress)   └───────────────────────────┘
                                                      │            │
                                          yt-dlp (finds YouTube    ffmpeg (glues
                                          stream links)            audio + video)
```

Two halves that talk to each other: a **window** you look at, and an **engine**
that does the heavy lifting. They are written in different languages, and a
framework called **Tauri** lets them talk.

---

## Part 2 — Concepts You Need First (the vocabulary)

You can skip this part if you know networking and systems programming, but every
later section relies on it.

| Term | Plain-English meaning |
|---|---|
| **Client / server** | You (the app) are the *client*; the website hosting the file is the *server*. The client asks, the server answers. |
| **HTTP** | The language clients and servers use on the web. A request has a *method* (GET = "give me this", HEAD = "just tell me about it, don't send it") and *headers* (extra info lines). |
| **Header** | A "Name: value" line attached to a request or response, e.g. `Content-Length: 1048576`. |
| **Content-Length** | A response header: "the file is this many bytes". |
| **Range request** | A request that says `Range: bytes=1000-1999` → "send me only bytes 1000 to 1999". The server replies with status **206 Partial Content**. This is the single feature that makes parallel downloading possible. |
| **Accept-Ranges: bytes** | A response header meaning "yes, I support range requests". |
| **TCP connection** | The pipe between you and the server. Opening one costs a little time; each can be speed-limited. |
| **Thread** | A worker inside a program that runs code. The OS juggles many. Threads are heavy (each reserves memory). |
| **Async / await** | A lighter way to do many things "at once": a task that is *waiting* (e.g., for the network) steps aside so others can run, all on a few threads. Perfect for 16 connections that spend most time waiting. |
| **Runtime (Tokio)** | The "scheduler" that runs async tasks. |
| **Race condition** | A bug where two workers touch the same data at once and the result depends on lucky timing. |
| **Lock / Mutex** | A "one at a time" gate around shared data. Safe but workers queue up at the gate, which slows things down. |
| **Atomic** | A single number that the CPU guarantees can be read/written safely by many workers at once *without* a lock. Very fast. |
| **Lock-free hot path** | The most frequently run code (here: handling every received chunk of data) never takes a lock. |
| **Channel (mpsc)** | A pipe/queue between workers: many senders push messages in, one receiver takes them out. |
| **Bounded channel / back-pressure** | A queue with a *maximum size*. When it's full, senders must wait. This stops a fast producer from burying a slow consumer and eating all the RAM. |
| **Pre-allocation** | Telling the OS up front, "reserve a file of exactly this size" before writing into it. |
| **Positioned write (`pwrite`)** | Writing bytes to a specific offset in a file ("put these at byte 5,000,000") without moving a shared "cursor". Many writers can use it at once safely. |
| **IPC** | "Inter-Process Communication" — here, the messaging bridge between the UI and the Rust engine. |
| **WebView** | A mini web browser embedded in a desktop app window; the UI is a web page drawn inside it. |
| **Mux / remux** | Combining separate audio and video streams into one file container. |
| **Sidecar file** | A small helper file stored next to a main file. Here: `movie.mp4.idm` remembers progress. |

---

## Part 3 — Every Technology, and Why It Was Chosen

### 3.1 The engine language: **Rust**

*What it is:* A programming language famous for being **as fast as C/C++ but
memory-safe** — the compiler refuses to build code that could cause whole classes
of crashes and data races.

*Why here:* A download manager juggles many simultaneous workers writing to one
file. In most languages, a mistake there causes silent file corruption. Rust's
compiler forces you to prove sharing is safe. It also needs no heavy runtime, so
the app stays small and fast. The project's own docs call out "strict thread
safety **and** low memory overhead" as the goal — Rust is one of the few languages
that offers both.

*Where in the repo:* everything under `src-tauri/`. Rust packages are called
**crates**, and are managed by **Cargo** (its package manager/build tool, config in
`Cargo.toml`). The project uses **edition 2021** and declares a minimum compiler
version of **1.70**.

### 3.2 The async runtime: **Tokio** (`tokio` with `full` features)

*What it is:* The standard async runtime for Rust.

*Why here:* Each connection is an async *task* (a "worker"). You can have dozens
of tasks without dozens of heavy OS threads. Tokio also provides the timers
(progress every 400 ms), channels, `Notify` (used to wake paused workers), and
`spawn_blocking` (used to run the disk-writer and the hashing on a real thread so
they don't stall the network tasks).

*Alternative considered:* Plain OS threads for each connection. Rejected because
async is lighter and Tokio gives timers/channels for free.

### 3.3 The HTTP client: **reqwest** (+ `rustls-tls`, `stream`, `gzip`, `brotli`)

*What it is:* The most popular Rust library for making HTTP requests.

*Why each feature:*

- **`stream`** — lets the app receive the response body *in small frames as they
  arrive* instead of waiting for the whole thing. Essential: you can't hold a
  4 GB file in memory.
- **`rustls-tls`** — HTTPS support using *rustls* (a TLS library written in Rust)
  instead of the system's OpenSSL. The docs say this was chosen "to avoid the
  OpenSSL build headache on Windows".
- **`default-features = false`** — turns off everything not listed, keeping the
  binary lean.
- **`gzip` / `brotli`** — automatic decompression of compressed responses.
  *(See the trade-off note in Part 5: this can conflict with range downloads.)*

*How it is configured (`build_client()` in `downloader.rs`):* one shared client
(one connection pool reused by every worker), up to 32 idle connections kept per
host, a **15 s connect timeout**, and — importantly — a **30 s *read* timeout
rather than a total timeout.** A *total* timeout would kill a perfectly healthy
download that simply takes ten minutes. A *read* timeout only fires when the
connection goes *silent* for 30 seconds, which is a real stall.

### 3.4 The desktop shell: **Tauri v1.6**

*What it is:* A framework for building desktop apps with a **Rust backend** and a
**web-technology frontend**, using the operating system's own built-in web view
(WebView2 on Windows) instead of shipping a whole browser.

*Why here:* The obvious competitor is **Electron**, which bundles a full copy of
Chrome. Electron apps are typically 150–300 MB and use a lot of RAM. A Tauri app
is a few MB. For a background utility that should sit idle quietly, that matters.
Also, because the backend is already Rust, the engine plugs in directly — no
extra process, no translation layer.

*Why v1 and not v2:* The docs say v1 was chosen "for stability". (Tauri 2 is the
newer generation and supports mobile; migrating is a possible future step — see the
handoff file.)

*What Tauri gives the project:*

- **Commands** — Rust functions marked `#[tauri::command]` that JavaScript can call
  with `invoke("name", {args})`.
- **Events** — Rust pushes messages to the UI with `emit_all("event-name", data)`.
- **Native dialog** — the Save-As window (`pick_save_path`).
- **Config (`tauri.conf.json`)** — window size (1100×720, min 820×560), app name
  "IDM Replica", id `com.idmreplica.app`, icons, and a *security allowlist*.
- **`tauri-build`** — a build-time helper (`build.rs`) that embeds icons and config
  into the final executable.

*Tauri features enabled in `Cargo.toml`:* `dialog-all`, `shell-open`, `path-all`
(and the matching allowlist in `tauri.conf.json`). `custom-protocol` is a Tauri
feature flag the template says must not be removed (it switches the app to load
bundled UI files in production builds).

### 3.5 The user interface: **React 18 + TypeScript (strict)**

*React:* a JavaScript library for building UIs out of small reusable pieces
(*components*) that automatically re-draw when their data (*state*) changes. The
whole dashboard — progress bars, buttons, sliders — is React.

*Why here:* The dashboard changes many times per second (speed, percent, per-thread
bars). React's "describe what the screen should look like for this data" model
fits perfectly: the engine sends new numbers, React updates the screen.

*TypeScript:* JavaScript with *type checking*. The project uses `"strict": true` and
also turns on `noUnusedLocals` / `noUnusedParameters`. The TypeScript interfaces in
`App.tsx` (like `ProgressPayload`) are deliberate *mirrors* of the Rust structs
(`ProgressPayload` in `downloader.rs`), so a mismatch gets caught when you type-check
rather than showing up as a mystery blank on screen.

*Style rules in the project (from `CLAUDE.md`):* functional components only, no
`any`.

### 3.6 The build tool: **Vite**

*What it is:* A fast development server and bundler for web frontends.

*Why here:* Gives instant reload while you edit the UI. It is pinned to **port
1420** with `strictPort: true`, because Tauri's config (`devPath:
"http://localhost:1420"`) expects exactly that address. `npm run build` runs the
TypeScript check (`tsc`) and then `vite build`, producing a `dist/` folder that
Tauri packs into the app.

### 3.7 Data formats: **serde** and **serde_json**

*What it is:* Rust's standard system for converting Rust structs to/from formats
like JSON.

*Why here:* Every message to the UI (progress payloads, events) is a Rust struct
turned into JSON. The attribute `#[serde(rename_all = "camelCase")]` makes Rust's
`file_name` appear as JavaScript-style `fileName`. Serde is also used to write the
`.idm` resume file.

### 3.8 Small but important helper crates

| Crate | What it does | Why it's used here |
|---|---|---|
| **bytes** | A cheap-to-copy byte buffer type. | Network frames arrive as `Bytes`; slicing them (to clamp to a chunk's end) doesn't copy data. |
| **futures-util** | Extra tools for async streams. | `StreamExt::next()` pulls the next frame from the response stream. |
| **anyhow** | Easy error handling (`anyhow::Result`). | The project's convention: functions that can fail return `anyhow::Result`. |
| **uuid (v4)** | Random unique IDs. | Every download gets a unique id so UI controls can find it. |
| **num_cpus** | Counts CPU cores. | Suggests a default connection count: `(cores × 2)` clamped to 4–16. |
| **async-trait** | Allows `async fn` inside traits. | Needed for the `Extractor` plug-in interface in `youtube.rs`. |
| **sha2 + md-5** | Hash functions. | Optional integrity check after download (SHA-256 and MD5). |
| **tracing + tracing-subscriber** | Structured logging. | Logs probe/split/retry events; verbosity set by the `RUST_LOG` environment variable (default: `info`). Deliberately *never* logs inside the per-frame hot path. |
| **axum** *(dev-only)* | A small web server framework. | The tests spin up a fake download server on localhost to test the engine without the internet. |

### 3.7b The release build profile

`Cargo.toml` configures release builds for maximum speed and smallest size:
`opt-level = 3` (max optimization), `lto = true` (whole-program optimization),
`codegen-units = 1` (slower compile, faster program), `panic = "abort"` (smaller
binary, no crash-unwinding), `strip = true` (remove debug symbols).

### 3.9 The two external programs: **yt-dlp** and **ffmpeg**

YouTube is special. High-quality video and audio are stored as **two separate
streams** (video-only and audio-only), and the links to them are protected by a
signature scheme that YouTube changes frequently.

- **yt-dlp** — a maintained command-line tool that knows how to defeat that and
  return direct links. The project *shells out* to it rather than re-implementing
  the logic, because a home-made parser "rots fast" (the docs' words).
- **ffmpeg** — the universal video tool. Used only to **mux** (glue) the video and
  audio files into one MP4 with `-c copy` — meaning *copy the data as-is, don't
  re-encode*. That is near-instant and loses zero quality.

Both must be installed and on the system `PATH`. The app checks for them *before*
starting (`preflight`) and shows an OS-specific install hint (e.g. `winget install
yt-dlp.yt-dlp`) if missing. They are **not** bundled with the app.

### 3.10 Operating-system file APIs

The project writes to the file with different OS calls depending on the platform:
**`seek_write`** on Windows and **`write_at`** on Unix (Linux/macOS). Both do a
"positioned write". Because those are OS-specific, the code has two versions
wrapped in `#[cfg(windows)]` / `#[cfg(unix)]` (Rust's "compile this only on that
OS" markers). The project rule: *any* OS-specific code must have both.

### 3.11 Quality tooling

- **cargo fmt** (auto-formats Rust), **cargo clippy** (a strict Rust linter, run
  with `-D warnings` = treat every warning as an error), **cargo test**.
- **`npx tsc --noEmit`** — type-checks the frontend without producing files.
- **GitHub Actions CI** (`.github/workflows/ci.yml`) — on every push/PR runs a
  "gate" job (type-check, fmt check, clippy, tests) on Linux, and then builds the
  real app on Windows, macOS and Linux.
- **Mermaid diagrams** (`docs/architecture/*.mmd`) — text-based diagrams that
  render as pictures in GitHub/VS Code.

---

## Part 4 — The Algorithms and Design Decisions (the "why" behind the code)

### 4.1 The four phases of a download

1. **Probe.** Send `HEAD` to learn file size and whether ranges are supported.
   Some servers refuse `HEAD`, so the fallback is a tiny `GET` with
   `Range: bytes=0-0`; a `206` reply with a `Content-Range: bytes 0-0/TOTAL`
   header reveals the true size. If ranges aren't supported at all, the engine
   downloads on a *single* connection (no speed-up is possible — a protocol limit,
   not a bug).
2. **Pre-allocate.** `set_len(total)` reserves the whole file first, so the OS
   doesn't grow it piecemeal while many workers write at scattered positions.
3. **Spawn.** Split into *N* equal slices (N = your "connections" slider, 1–16,
   capped so a slice is never smaller than 1 MiB) and start one async worker per
   slice, each with its own `Range:` request.
4. **Dynamic split.** Whenever a worker finishes, the coordinator finds the live
   slice with the **most bytes remaining**, cuts it in half, and starts a new worker
   on the back half. It refuses to split if less than **1 MiB** remains (a new
   connection costs more than it saves).

### 4.2 Why atomics (and no locks) on the hot path

Each slice (`Chunk`) stores two numbers: `current` (next byte needed) and `end`
(where to stop). Both are **`AtomicU64`**. A *split* is literally one instruction:
`victim.end.store(midpoint)`. The victim worker re-reads `end` on every received
frame, so it simply notices the new, smaller boundary and stops there. No locks, no
messages, no interrupting its connection.

If we'd used a lock, every frame from every connection would queue at that lock —
exactly the "throughput cliff" the project is designed to avoid. Locks *are* used,
but only on **rare** events: the list of chunks sits behind a `tokio::Mutex` that is
touched only on splits and progress snapshots.

### 4.3 Why one dedicated writer thread + positioned writes

Many workers writing one file is the hardest correctness problem here.

- **Naive approach:** wrap the file in a `Mutex`, `seek()` then `write()`. Correct,
  but every write serialises on the lock.
- **This project's approach:** workers send `(offset, bytes)` messages through a
  **bounded channel** (depth **512**) to **one** writer thread (started with
  `spawn_blocking`), which does `pwrite` at that offset. Positioned writes don't
  share a file cursor, so two regions can never clobber each other.
- **Back-pressure:** if the disk is slower than the network, the channel fills and
  workers pause automatically. RAM use stays bounded. (That's why the channel must
  never be unbounded.)
- **Idempotent:** writing the same bytes to the same offset twice leaves the file
  unchanged. This matters because right after a split, the old worker may have
  already streamed a few bytes past the new midpoint, and the new worker re-fetches
  that tiny overlap. Harmless — *as long as writes stay idempotent*. This is one of
  the project's "do not break" rules.

### 4.4 Resilience: timeouts, retries, pause, resume, cancel

- **Retry with exponential backoff:** on a dropped connection or error, wait
  250 ms, then 500, 1000, 2000, 4000, 8000 (max 6 retries), then mark the slice
  failed. Crucially it re-requests from `current`, never from the slice's start; and
  any real progress resets the retry counter.
- **Pause:** sets a flag; workers drop their connection and sleep on a `Notify`.
  **Resume** wakes them and they re-issue a range request from `current`.
- **Cancel:** sets a flag, wakes sleepers, flushes the writer, deletes the partial
  file *and* the `.idm` sidecar.
- **Cross-restart resume (`.idm` sidecar):** about every 400 ms the coordinator
  writes a tiny JSON file recording every slice's start/current/end (written to a
  temp file then renamed, so a crash can't leave a half-written file). If you start
  the same URL → same destination again later, the engine validates the sidecar
  (same URL, same size, file still exists at that size) and continues from the saved
  offsets. The sidecar is deleted on success/cancel and kept on error.

### 4.5 Speed limiting: a token bucket, at the right place

A **token bucket** is a classic rate-limiting algorithm: tokens (bytes of
allowance) refill at a fixed rate; each chunk of data must "spend" tokens, and if
there aren't enough, wait. All workers of one download share one bucket, so their
*combined* speed respects the cap. When the limit is 0 (unlimited) the fast path
returns immediately with no lock at all.

**Where it sits matters:** it's applied *before consuming a network frame*, not
around the disk write. Throttling the disk would just back up memory; throttling
the network read makes the OS/TCP slow the sender down for real. This is another
"do not break" rule.

### 4.6 The queue and the scheduler (`main.rs`)

A `QueueManager` lets only *N* downloads run at once (default 3, adjustable 1–10).
New downloads wait in a list; each time one finishes, `promote()` starts the next.
An entry can carry a `start_at` time (epoch seconds) for scheduled starts. The
queue position streams to the UI through a `queue-status` event.

### 4.7 The passive-UI rule

State flows **Rust → React only through events** (`download-progress`,
`youtube-status`, `queue-status`). The UI never polls and never holds the "real"
state. React only sends explicit *control commands* (start, pause, resume, cancel,
set speed limit, set max concurrent, pick save path). This keeps the UI responsive
even while 16 connections hammer the engine, and means there is one source of truth.

### 4.8 The engine is GUI-free (`ProgressSink`)

The engine doesn't import Tauri. It reports progress through a small **trait**
(an interface) called `ProgressSink` with one method, `emit(payload)`. `main.rs`
provides a Tauri-backed implementation; tests provide an in-memory one. That is
what lets the engine be tested with no window.

### 4.9 Pluggable site support (`Extractor` trait)

Stream sites are handled behind an `Extractor` trait (`name`, `detect`, async
`extract`). Today there is one implementation, `YtDlpExtractor`, which recognises
YouTube, Vimeo, Twitch, Dailymotion, TikTok, Facebook watch, and Twitter/X URLs.
Adding a new site means implementing the trait and listing it in `providers()` —
the engine and muxer don't change.

### 4.10 The YouTube pipeline

Detect → preflight (are yt-dlp & ffmpeg installed?) → extract a video-only URL and
an audio-only URL → download **both at the same time** (each with full dynamic
segmentation) → `ffmpeg -c copy` mux → delete temp files → one `.mp4`. Temporary
files are hidden-dot names like `.<id>.video.mp4`. If either download fails, both
temp files are deleted.

### 4.11 Integrity verification

Optional. After a successful download the file is streamed through SHA-256 and MD5
on a blocking thread (so it can't stall the network). If you pasted an expected
checksum, the UI shows ✓ verified or ✗ MISMATCH.

### 4.12 Mirrors and custom headers

- **Headers:** any `Key: Value` lines you type (e.g. `Authorization: Bearer …`,
  `Cookie: …`) are attached to *every* request — the probe and all workers.
- **Mirrors:** extra URLs for the *same* file. Workers pick their source with
  `chunk.id % number_of_urls`, spreading ranges across servers to combine their
  bandwidth.

---

## Part 5 — Honest Trade-offs and Things to Be Aware Of

1. **Rust has a learning curve** — the compiler is strict. The payoff is safety and
   speed; the cost is slower initial development.
2. **Tauri v1 is the older generation.** It works, but v2 is the actively developed
   line (and the one that supports mobile).
3. **yt-dlp and ffmpeg aren't bundled.** Users must install them for YouTube
   downloads; plain file downloads don't need them.
4. **The `gzip`/`brotli` reqwest features may conflict with range downloads.** If a
   server compresses a response, "bytes N–M" refer to the *compressed* bytes, while
   the engine counts *decompressed* bytes. Servers rarely compress already-packed
   downloads, but for highly compressible files this is a latent risk. A common fix
   is to disable those features or send `Accept-Encoding: identity`.
5. **Mirrors are trusted blindly.** The code doesn't verify that a mirror really
   serves an identical file (size/ETag). A bad mirror would silently corrupt the
   result — which is why the optional checksum matters.
6. **Resume across restarts is URL-exact.** It works when you re-enter the same
   URL and destination. YouTube's links expire and change per session, so
   cross-restart resume isn't practical for the YouTube path.
7. **The Windows toolchain note:** the docs recommend the MSVC Rust toolchain
   (Tauri's officially tested path); GNU may work for dev only.
8. **First build is slow** (compiles Tauri + reqwest). Also, icons must exist under
   `src-tauri/icons/` before it compiles.

---

## Part 6 — One-Page Cheat Sheet

| Question | Answer |
|---|---|
| What is it? | An IDM-style multi-connection download manager with a YouTube video+audio merge path. |
| Engine language | Rust (edition 2021), async via Tokio |
| HTTP | reqwest 0.12 with rustls |
| Desktop shell | Tauri 1.6 (native WebView, tiny binary) |
| UI | React 18 + strict TypeScript, bundled by Vite (port 1420) |
| Core algorithm | Dynamic segmentation: split the largest remaining slice when a connection frees up |
| Concurrency safety | Atomics on the hot path; one writer thread; positioned writes; bounded channel (512) |
| Resilience | Read timeout 30 s, 6 retries w/ exponential backoff, pause/resume/cancel, `.idm` resume file |
| Extras | Speed limit (token bucket), queue + scheduling, SHA-256/MD5 verify, headers, mirrors, Save-As dialog |
| YouTube | `yt-dlp` to find streams → 2 parallel segmented downloads → `ffmpeg -c copy` |
| Quality gates | `cargo fmt`, `cargo clippy -D warnings`, `cargo test`, `npx tsc --noEmit`, CI on 3 OSes |
