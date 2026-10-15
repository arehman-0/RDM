# Bridge / app changes for the extension — status (updated 2026-10-12)

## Resolved 2026-10-12
- **New error code `js_runtime_missing`** (503). The preflight for a video page now checks
  yt-dlp → JavaScript runtime → ffmpeg and reports the first one missing. A runtime is
  QuickJS from the component manager, else `deno` on `PATH`, else `node` on `PATH`
  (yt-dlp is then run with `--js-runtimes node`). Manifest (`.m3u8` / `.mpd`) jobs skip
  the runtime check. The extension shows `MSG.jsRuntimeMissing`.
- **One-click pairing** (`POST /v1/pair`) and the pre-allowed extension ID are in; see the
  README. The app is brought to the front for the Allow / Deny prompt.
- **Tray notice**: it used to open behind other windows. It is now owned by the app
  window and the window hides on OK.

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
- **App auto-update (evaluated 2026-10-15, not enabled).** Tauri v1's updater needs: the
  `updater` cargo feature (pulls in minisign signature checking and its own update
  download/install code; the exe size cost was not measured), a signing keypair
  (`tauri signer generate`; the private key + password become CI secrets), a hosted JSON
  endpoint (e.g. a static `latest.json` on GitHub Releases or Pages) and, per release, the
  extra `*.nsis.zip` + `.sig` artifacts. On Windows it runs the new NSIS installer in
  passive mode. That is a hosting decision, two new secrets and a release process, so it
  stays off. Proposal: (a) keep it off and show "a newer version is available" by
  comparing with the GitHub releases API (small, no keys, but still a network call to
  decide); or (b) enable the Tauri updater once releases are published and signing is
  set up. Note that yt-dlp already updates itself; the app rarely *must* update.
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
- **Custom FFmpeg build** (written up 2026-10-12; nothing was built or measured). Today the
  app installs BtbN's LGPL *shared* build: 77 MB download, 156 MB on disk. The app only
  ever stream-copies, so almost all of that is unused.

  What is actually used:
  - *By the app* (`muxer.rs`): `ffmpeg -version`, then
    `ffmpeg -y -hide_banner -loglevel error -i <video> -i <audio> -c:v copy -c:a copy
    -map 0:v:0 -map 1:a:0 -movflags +faststart <out>.mp4`. The inputs are local files that
    yt-dlp reported as `mp4`/`m4a` (preferred by our format selector) or `webm`.
  - *By yt-dlp* (`stream_dl.rs`, format `bv*+ba/b`, found through `--ffmpeg-location`):
    the merger (`-c copy -map 0:v:0 -map 1:a:0`, output mp4, or mkv/webm when the codecs
    don't fit mp4), and the fix-ups after its own HLS/DASH download (MPEG-TS → mp4 with
    `-bsf:a aac_adtstoasc`, DASH m4a → mp4, timestamp/duration fix, all `-c copy`). yt-dlp
    downloads HLS/DASH segments itself; it only lets ffmpeg *download* for live streams and
    a few HLS variants its native downloader refuses.
  - *Not used anywhere*: decoding, encoding, filters, scaling, resampling, devices,
    subtitles/thumbnail embedding, audio extraction (`-x`), `ffprobe`, `ffplay`.

  | Kind | Needed |
  |---|---|
  | Protocols | `file`, `pipe` (mux only); plus `http`, `https`, `tcp`, `tls`, `crypto`, `data` for the ffmpeg-as-HLS-downloader fallback |
  | Demuxers | `mov` (mp4/m4a/fMP4), `matroska` (mkv/webm), `mpegts`, `aac`, `mp3`, `ac3`, `eac3`, `flac`, `ogg`, `flv`, `concat`; plus `hls` for the fallback |
  | Muxers | `mp4`, `mov`, `matroska`, `webm`, `mpegts` |
  | Parsers | `h264`, `hevc`, `vp8`, `vp9`, `av1`, `aac`, `aac_latm`, `opus`, `vorbis`, `mpegaudio`, `ac3`, `flac` |
  | Bitstream filters | `aac_adtstoasc`, `extract_extradata`, `h264_mp4toannexb`, `hevc_mp4toannexb`, `vp9_superframe`, `vp9_superframe_split`, `av1_frame_merge` |
  | Decoders / encoders / filters | none (only the few no-op filters the `ffmpeg` tool itself selects) |

  Configure line (MinGW-w64 cross build, static, LGPL v2.1+ — no `--enable-gpl`,
  no `--enable-nonfree`):

  ```
  ./configure --target-os=mingw32 --arch=x86_64 --cross-prefix=x86_64-w64-mingw32- \
    --enable-static --disable-shared --extra-ldflags=-static --enable-small \
    --disable-everything --disable-autodetect --disable-doc --disable-debug \
    --disable-ffplay --disable-ffprobe \
    --disable-avdevice --disable-swscale --disable-swresample \
    --enable-schannel \
    --enable-protocol=file,pipe,http,https,tcp,tls,crypto,data \
    --enable-demuxer=mov,matroska,mpegts,hls,aac,mp3,ac3,eac3,flac,ogg,flv,concat \
    --enable-muxer=mp4,mov,matroska,webm,mpegts \
    --enable-parser=h264,hevc,vp8,vp9,av1,aac,aac_latm,opus,vorbis,mpegaudio,ac3,flac \
    --enable-bsf=aac_adtstoasc,extract_extradata,h264_mp4toannexb,hevc_mp4toannexb,vp9_superframe,vp9_superframe_split,av1_frame_merge
  ```

  - Mux-only variant: drop `--enable-schannel`, the network protocols and `hls`, and add
    `--disable-network`. Then live HLS (and the few streams yt-dlp hands to ffmpeg) fail.
  - Estimated size: **about 2–4 MB** for one static `ffmpeg.exe` (mux-only near the low
    end), against 156 MB today. This is an estimate from comparable minimal remux builds;
    it has not been built.
  - To check when it is built: (1) the `ffmpeg` tool needs libavfilter, so it stays on —
    confirm configure accepts the build without swscale/swresample on the pinned version;
    (2) with no decoders, FFmpeg can warn that it cannot find codec parameters for some
    MPEG-TS inputs — test an HLS (AAC in TS) fix-up, and add `--enable-decoder=aac,h264`
    (about +1 MB) if it fails; (3) `--disable-autodetect` also drops zlib, which only
    matters for compressed MP4 headers / Matroska tracks (rare).
  - Cost: we own the build (a pinned source tag, a CI job, the SHA-256 in
    `components.json`) and must publish the source + this configure line for the LGPL.
  - **Decision needed:** (a) keep BtbN's build, (b) ship our own mux-only build, or
    (c) ship our own build with the HLS fallback (the line above). Recommended: (c).
