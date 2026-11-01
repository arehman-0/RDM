# Roadmap — downloading video from any streaming site (incl. third-party hosts)

Goal: when a user plays a video on **any** site — big platforms and the many third-party
players/hosts embedded in other pages — IDM Replica offers a working download, the way IDM's
"Download this video" button does. Written 2026-11-01. Nothing here is implemented yet.

Hard limits (not on the roadmap, on purpose):
- **DRM** (Widevine / PlayReady / FairPlay — Netflix, Prime, Disney+, most paid TV). We never
  try to decrypt; we detect it and say so (`drm_protected` already exists).
- **No anti-bot or paywall circumvention** (no CAPTCHA solving, no faking logins). We only use
  what the user's own browser session already has (cookies, headers), for downloads the user
  starts.
- Only content the user has the right to download; the README and store listing say so.

## Where we are today

| Piece | What it does | Gap |
|---|---|---|
| `extension/src/lib/media.ts` | Sniffs network responses: direct video/audio files by extension or Content-Type, HLS (`.m3u8`) and DASH (`.mpd`) manifests; skips ad hosts and segment files. | Misses manifests with odd names/types, JSON/API-delivered sources, media inside cross-origin iframes' own requests in some cases, and gives no duration/quality to choose by. |
| `extension/src/lib/sites.ts` + `src-tauri/src/youtube.rs::is_video_page` | A **fixed list** of ~10 sites (YouTube, Vimeo, X, Facebook, Instagram, Reddit, Twitch, Dailymotion, TikTok) whose *page URL* is handed to yt-dlp. | yt-dlp supports ~1,800 sites; we only try 10. Every other site depends on sniffing. |
| `content.ts` | Finds `<video>/<audio>` elements (also in iframes), draws the Download button. `blob:`/MediaSource players are reported as "the page". | For an unknown site, "the page" goes nowhere useful. |
| `stream_dl.rs` | Manifest jobs: one `yt-dlp` run with Referer/UA/Cookie, quality selector, cancel. | No pause/resume, no quality picker in the UI, no live-stream mode. |
| Bridge | Sends url, pageUrl, referer, userAgent, cookies, mediaType, size. | No `Origin` header, no per-request extra headers the site needs (e.g. `X-Requested-With`, auth tokens in headers). |

## Phase 1 — "Try with yt-dlp" for every page (biggest win, small change)

1. **Generic page fallback.** For a page not in the fixed list, offer *Download with yt-dlp*
   anyway: send `kind: "stream"` with the page URL; the app runs `yt-dlp -J` and, if it finds
   formats (site extractor **or** yt-dlp's generic extractor, which reads `<video>`, JSON-LD,
   og:video, embedded players), continues as today.
   - App: relax `unsupported_site` for user-initiated requests (keep the list only to decide
     what is offered *automatically*). New error code `no_video_found` when yt-dlp says
     "Unsupported URL" / "No video formats found".
   - Extension: a popup row "Try to download this page's video (yt-dlp)" on every page with a
     playing video or a sniffed blob player.
2. **Embedded players (iframes).** Third-party hosts are usually an `<iframe>` from another
   domain. Send the **iframe's URL** (with the outer page as Referer) instead of the outer
   page; yt-dlp has extractors for many embed hosts. `content.ts` already runs in all frames —
   report `frameUrl` alongside `pageUrl`.
3. **Single-format results.** `youtube.rs::parse_yt_dlp_json` rejects results without separate
   video+audio streams. Accept a single muxed format (download it directly with the engine) and
   HLS/DASH-only formats (hand them to `stream_dl.rs`). Today these fail with "did not return a
   separate video stream".
4. **Faster failure.** Cap `yt-dlp -J` at e.g. 45 s and show "this site isn't supported" clearly.

Acceptance: on a test set of 30 public pages from sites outside the fixed list (news players,
embedded hosts, small video sites), ≥ 70 % download; the rest fail with a clear message.

## Phase 2 — Better sniffing for players yt-dlp doesn't know

1. **Manifest detection by content, not name.** Some sites serve HLS as `/playlist?id=…` or
   `.txt`, or `application/octet-stream`. For responses ≤ 64 KB with an unknown type, the
   extension can't read bodies via `webRequest`, so: in the content script, wrap
   `fetch`/`XMLHttpRequest` in the page's MAIN world (`"world": "MAIN"` content script) and
   report responses whose body starts with `#EXTM3U` or `<MPD`. Opt-in setting, documented in
   `extension/store/permissions.md` (no new permission needed, but reviewers will ask).
2. **MSE / blob players.** Record which manifest or segment URLs fed `MediaSource` (hook
   `SourceBuffer.appendBuffer` timing + the last manifest seen in that tab) so a blob video
   maps to its manifest instead of "the page".
3. **JSON sources.** Many players load `{"sources":[{"file":"…m3u8"}]}` from an API. Same MAIN
   world hook: scan small JSON responses for media URLs.
4. **Rank and label candidates.** For each candidate: duration and resolutions (parse the
   master playlist / MPD in the extension, or a new `GET /v1/probe` that runs `yt-dlp -F`),
   size when known. Hide items < 30 s (ads, previews), sort by duration × resolution, mark the
   one matching the playing `<video>`'s duration as "this video".
5. **Ad filtering.** Extend `AD_HOSTS`, add VAST/VMAP detection (XML responses with `<VAST`),
   and skip pre-roll streams that end before the main video starts.

Acceptance: on 20 pages whose player yt-dlp does not support, the right stream is the top
candidate on ≥ 15.

## Phase 3 — Getting the request right (why third-party hosts return 403)

1. **Send what the browser sent.** Capture, per media request, the request headers the site
   needed: `Referer`, `Origin`, `User-Agent`, `Cookie`, plus custom ones (`Authorization`,
   `X-*`). Use `webRequest.onSendHeaders` with `extraHeaders`; pass them through the bridge
   (`headers` already exists; add `origin`). Keep the bridge's limits and forbidden-header list.
2. **Short-lived URLs.** Signed URLs (`expires=`, `token=`) die in minutes. Start immediately
   (no extra probe round-trips), and on 403/410 mid-download ask the extension for a fresh URL
   (re-sniff in that tab) — a new bridge message `refresh_url {id}`.
3. **IP / session-bound tokens.** Download through the same network as the browser (we already
   do: same PC). For hosts that bind to the TLS fingerprint, fall back to yt-dlp with
   `--impersonate` (curl_cffi build) — check licence and size impact first.
4. **HLS details** (yt-dlp handles most; verify): AES-128 keys (key URL needs the same
   cookies/headers), fMP4/CMAF segments, separate audio renditions, discontinuities, byte-range
   playlists.

## Phase 4 — Our own HLS/DASH downloader (speed + pause/resume)

Today manifests go to yt-dlp (one connection per segment, sequential-ish, no pause).

1. New `hls.rs` / `dash.rs` in the engine: parse the playlist, download segments in parallel
   (N connections, same throttle and `ProgressSink`), write each segment at its offset into a
   temp file via the existing flusher, resume from a sidecar listing finished segments.
   Respect the invariants in `CLAUDE.md` (bounded channel, positioned writes, throttle at the
   read step, resume never from 0).
2. Decrypt AES-128-CBC segments (keys fetched with the request's headers). Never SAMPLE-AES
   with DRM key systems.
3. Concatenate (TS) or remux (fMP4 → MP4) with FFmpeg `-c copy`.
4. Pause / resume / cancel like file downloads; live streams = "record from now" with a stop
   button.

Acceptance: a 1-hour HLS VOD downloads ≥ 3× faster than yt-dlp's native downloader on the
same connection, survives pause, app restart and a dropped connection.

## Phase 5 — Quality and UX

- Quality picker in the floating button / popup (1080p / 720p / audio only), using the probe
  from Phase 2.4. The bridge already accepts `quality`.
- The new **Download request** popup shows resolution, duration and an estimated size, and
  lets the user change file name, folder and quality before **Start**.
- Subtitles (`--write-subs`) and thumbnail as options.
- "This site isn't supported" reporting: a button that copies a diagnostic (site, what was
  sniffed, yt-dlp's error — no cookies) for a GitHub issue. No telemetry.

## Phase 6 — Keeping it working

- **Site-compat test suite** (`tests/sites/`): a list of public test URLs per category, run
  nightly in CI with the pinned and the latest yt-dlp (`--simulate -J`), reporting which broke.
  Never in the normal PR gate (network, flaky).
- Local fixtures (`extension/test/media/`) for every sniffing rule: odd manifest names, JSON
  sources, iframes, blob players, ads.
- yt-dlp updates already run daily; add a "yt-dlp nightly" channel toggle for sites that break
  often.
- Firefox / Edge-specific checks for the extension (Edge works today via Chromium; Firefox
  would need an MV3 port of `webRequest` usage and a different store).

## Suggested order

| Step | Effort | Why first |
|---|---|---|
| 1.1 + 1.3 generic yt-dlp fallback, single-format results | S | Unlocks most of yt-dlp's 1,800 sites with little code |
| 1.2 iframe URL | S | Most third-party hosts are embeds |
| 3.1 send the browser's headers (incl. Origin) | M | Fixes most 403s on third-party CDNs |
| 2.4 rank/label candidates + 5 quality picker | M | Users pick the right stream instead of an ad |
| 2.1–2.3 MAIN-world sniffing | M | Players yt-dlp doesn't know |
| 3.2 URL refresh | M | Long downloads from signed URLs |
| 4 own HLS/DASH downloader | L | Speed, pause/resume, live |
| 6 compat suite | M, ongoing | Keeps it working as sites change |

Each step ships with tests (`cargo test`, Vitest) and keeps `extension/store/permissions.md`
accurate — every new capability must be justified there before a store update.
