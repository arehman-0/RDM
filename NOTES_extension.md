# Bridge / app changes for the extension — status (updated 2026-10-09)

## Resolved this session
1. **Single instance**: `tauri-plugin-single-instance` (v1 branch) plus an OS file lock on
   `app.lock`, taken before any persisted file is opened. A second launch focuses the
   window, passes its URL as a confirm card, and exits.
2. **yt-dlp site list**: `youtube.rs::is_video_page` now matches
   `extension/src/lib/sites.ts`:
   - Instagram posts/reels
   - Reddit posts and v.redd.it
   - Facebook watch / reel / videos / share / fb.watch
   - Twitch VODs and clips (not live channels)
   - Dailymotion, TikTok, YouTube, Vimeo, X

   Both sides are tested against the same URL shapes.
3. **HLS/DASH manifests**: `kind:"stream"` (or `auto`) with an `.m3u8` / `.mpd` URL or an
   HLS/DASH `mediaType` → `stream_dl.rs`:
   - yt-dlp runs with `--add-header` for Referer, UA and Cookie, and `-f` from `quality`.
   - Progress shows as one job, and cancel kills the process tree.
   - A temp dir is used and then removed; output outside the folder is refused.
   - DRM gives a clear job error.
4. **Machine-readable errors**: every error is `{ok:false, code, message}`. The codes are
   listed in the README; the extension maps them to its own wording.
5. **Token check**: `GET /v1/auth` returns 204 or 401. The extension uses it, and falls back
   to the empty-batch probe for older apps.
6. **Cookie cap** raised to 32 KB (app and extension).
7. **Take-over**: new `source` field (`takeover` | `user-action` | `page-scan`, plus app-side
   `command-line`).
   - A take-over starts immediately, but only from the allowed extension origin and with
     a valid token.
   - Rate limit: 20 starts per 10 s; the excess waits as pending-confirm.
   - The same URL within 5 s is answered with the first result.
   - Non-take-over pending cards are capped at 50 (`app_busy`).
   - Setting *Confirm browser take-over downloads* (default off).

## Needs decision (not started)
- **UI defaults for browser downloads** (old item 6): browser jobs still use the suggested
  connections, the OS Downloads folder and no speed cap, because the UI's sliders live
  only in React.
  - Proposal: persist "download defaults" (connections, folder, speed cap) in Rust
    (`settings.json` next to `bridge.json`) and set them through a new command.
  - The UI then reads them back instead of keeping its own state.
  - Medium size: it touches every start path.
- **Quality picker in the extension**: the bridge accepts `quality`, but the extension
  always sends the default (best).
  - Proposal: when the manifest exposes variants, a "1080 / 720 / audio" sub-menu in the
    floating button.
  - That needs parsing the master playlist in the extension (or a new `/v1/probe` that
    asks yt-dlp `-F`).
- **Pause for manifest jobs**: a yt-dlp stream can be cancelled but not paused.
  - Proposal: pause = kill, resume = rerun yt-dlp (it continues `.part` files) with the
    same temp dir.
