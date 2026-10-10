# PROGRESS — session 7 (2026-10-09): app ↔ extension gaps, take-over by default

Sessions 1–6 done. Contract: README "Browser bridge HTTP API" (endpoints, fields, error codes).

## Done
- **Single instance:** `tauri-plugin-single-instance` (git, v1 branch). A second launch
  focuses the window, passes an http(s) argv URL as a confirm card, and exits. Second
  defence: `instance_lock.rs` holds an OS file lock (`File::try_lock`, so `rust-version`
  is now 1.89) on `app.lock` *before* any file is opened. The OS releases it on a crash.
- **HLS/DASH:** `stream_dl.rs`. A `.m3u8`/`.mpd` URL (`kind` stream or auto, or an HLS/DASH
  `mediaType`) is downloaded by yt-dlp:
  - `--add-header` for Referer/UA/Cookie, `-f` from `quality` (best|audio|144–4320).
  - Progress through the normal sink (one card); cancel kills the process tree.
  - Private temp dir removed afterwards; output outside the folder is refused.
  - DRM → "This stream is DRM-protected…". New `JobKind::Manifest`; quality persisted.
- **Error codes:** `{ok:false, code, message(+error)}`, 14 stable codes (README table);
  `GET /v1/auth`; tool codes from a structured preflight. Cookie cap 32 KB.
- **Sites:** `is_video_page` covers FB watch/reel/videos/share/fb.watch, Instagram, Reddit,
  Twitch VODs/clips, Dailymotion, TikTok. The extension's `sites.ts` uses the same rules.
- **Take-over (extension):** ON by default, min size 0, empty lists, plus
  "let Chrome handle these types" and "never take over" sites.
  - Pure `decideTakeover` runs first; the send waits for `ok`, and only then Chrome's
    copy is cancelled and erased.
  - Each download id is handled once (OnceSet); POSTs are tracked via `onBeforeRequest`.
  - Falls back for blob/data/filesystem/file, POST and incognito, with a "!" badge and toast.
- **Take-over (app policy):** `source` takeover|user-action|page-scan|command-line.
  - A take-over starts at once only from the allowed extension origin + token, up to
    20 per 10 s (the excess goes pending).
  - Same URL within 5 s → first result (`duplicate`); non-take-over pending max 50 (`app_busy`).
  - App setting "Confirm browser take-over downloads" (off). `join_inside` guard in the engine.

## Verified (actually run)
- All green: fmt --check, clippy -D warnings, `cargo test` **95** (new: 3 lock, 9 stream_dl
  with a fake yt-dlp, 11 bridge policy, detection, join_inside), root tsc, `npm test` **48**,
  extension typecheck, vitest **36**, build.
- **Real app binary:**
  - Second launch → "handing over", the first logged "focusing" and turned the CLI URL
    into a confirm card (3 runs).
  - curl `.m3u8` → yt-dlp → `index.mp4` 12.0 s h264+aac, no temp left (deleted after).
  - Live codes: bad_token, origin_not_allowed, bad_url, unsupported_site, bad_request,
    unsupported_media_type, ytdlp_missing (yt-dlp off PATH); `/v1/auth` 204.
- **Real Chrome** (throwaway profile): paired via `/v1/auth`; the test page lists `index.m3u8`
  once (no `.ts`); sending it reached the app as `manifest=true source=UserAction`.

## NOT verified
- **Take-over in a browser:** Chrome under CDP fetches the file but never creates a
  download (same as last session), so cancel/erase, the badge hint and POST/blob
  fall-backs are unit-tested only (decision + mocked chrome/fetch). See README step 7.
- In 1 of 3 two-launch runs the *first* instance had also exited (no log line).
  I couldn't reproduce it in 2 more runs. Watch for it in manual test (a).
- The focus itself (no dev UI was loaded), macOS/Linux plugin behaviour, real DRM HLS.

## Needs your decision
Details in NOTES_extension.md:
- persist download defaults in Rust for browser jobs;
- a quality picker in the extension;
- pause/resume for yt-dlp stream jobs.

## Next — run the manual steps (a)–(d) from this session's report, then pick from "needs decision".
