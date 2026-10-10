# IDM Replica — browser extension (Chrome, Manifest V3)

Detects videos and files on the pages you browse and sends them to the IDM Replica
desktop app: an IDM-style floating **⬇ Download** button on playing videos, a toolbar
popup listing what the page loaded, a right-click menu item, and (optionally) taking
over Chrome's own downloads. The app's paste-a-link flow is unchanged.

**Extension ID: `pcmcmknffhfehgpkoedecnabbphgnabg`.** It stays the same for every
install because `manifest.json` carries a `key`.

## Build

```bash
cd extension
npm install
npm run build        # → extension/dist (load this folder)
npm run typecheck    # strict TS: src + tests
npm test             # Vitest (pure logic)
```

No runtime dependencies: esbuild bundles each entry point into a self-contained script.
There is no remote code and no analytics.

## Install and pair (once)

1. Start IDM Replica.
2. Chrome → `chrome://extensions` → turn on **Developer mode** (top right) →
   **Load unpacked** → select `extension/dist`.
3. Copy the extension ID `pcmcmknffhfehgpkoedecnabbphgnabg` (also shown in the
   extension's Options). In IDM Replica → **Browser integration** → *Allowed extension
   IDs*, paste it → **Save IDs**.
4. In IDM Replica → Browser integration → **Copy token**. Open the extension's
   **Options** (right-click the toolbar icon → Options), paste the token, then
   **Test connection**. You should see *"Connected to IDM Replica 0.1.0 on port 38917"*.
   If the app shows a different port, put that port in Options.

## Using it

- **Floating button:** hover a playing video that is at least ~200×120 px. A
  **⬇ Download** button appears at its top-right. Click it to see what was found on the
  page, plus **Download page video (best)**, which uses the hovered video's own file.
  On YouTube and other known sites, "best" lets the app's yt-dlp pick the best
  quality. The menu also offers *Don't show this button on <site>*.
- **Toolbar popup:** lists the current tab's media (name, type, quality, size), with a
  Download button per row, **Download all**, the app's connection status, and a box
  to paste any link. The badge shows how many items were found. The list clears when
  you navigate.
- **Right-click → "Download with IDM Replica"** on a link, a video, an audio element,
  or the page itself.
- **Every Chrome download goes to IDM Replica (ON by default).** When you click a file
  link, the extension:
  1. decides first: Chrome keeps downloads the app can't fetch again (`blob:`/`data:`/`filesystem:`/`file:`
     URLs, form/POST downloads, incognito) and anything your Options exclude;
  2. sends it to the app (final URL after redirects, referrer, User-Agent, that domain's
     cookies, file name, type, size) as `source:"takeover"`, which the app starts at once
     without a confirm card;
  3. **only after the app answers ok**, cancels Chrome's copy and removes it from
     Chrome's download list.

  If the app isn't running, the token is wrong, or the app refuses, Chrome keeps the
  download, and the toolbar icon shows **!** with *"IDM Replica: not running — Chrome handled
  this download."* Options: on/off, a minimum size (default 0 = every file), "only these
  types", "let Chrome handle these types", and sites to never take over.

For everything else (floating button, context menu, popup), the app asks first by default
(an Accept/Dismiss card); turn that off in the app. Take-overs have their own app
setting, *Confirm browser take-over downloads* (default off).

## What works / what doesn't

| Case | Result |
|---|---|
| Direct file links, `<video src="…mp4">`, media loaded over the network (mp4, webm, mkv, mp3, m4a, …) | Listed and sent with the page as Referer, your User-Agent, and that URL's cookies |
| Known stream sites: YouTube (watch, Shorts, embeds), Vimeo, Twitter/X, Facebook, Instagram, TikTok, Reddit, Twitch, Dailymotion | Never sniffed. The page URL is sent as `kind:"stream"` and the app's yt-dlp resolves it. The list is in `src/lib/sites.ts` |
| `blob:` / MediaSource players on **other** sites | Listed as "Page stream". Sending it explains it can't be resolved; any real media file the player fetched is listed separately and can be sent |
| HLS/DASH playlists (`.m3u8`, `.mpd`) on any site | Listed once (never their segments). Sent as `kind:"stream"` with the manifest URL; the app downloads and merges it with yt-dlp (best quality by default) |
| `.ts` / `.m4s` segments, files < 100 KB, ad/tracking hosts | Not listed |
| DRM-protected video (Encrypted Media Extensions: Netflix, Prime, Disney+, …) | "This site uses protected video; it can't be downloaded." |
| `file:`, `javascript:`, `data:` links | "This link type can't be downloaded" |

The app accepts the same video-page shapes as `src/lib/sites.ts` (YouTube, Vimeo, X, Facebook
watch/reel/videos/fb.watch, Instagram posts/reels, Reddit posts/v.redd.it, Twitch VODs and clips,
Dailymotion, TikTok). A live Twitch channel is not a video page.

## Permissions (and why)

| Permission | Why |
|---|---|
| `webRequest` | Observe response headers (Content-Type / Length / Range) to find media the page loads, and note POST requests (their downloads stay in Chrome). Observe-only: nothing is blocked or modified |
| `storage` | Settings in `storage.local`; the per-tab media list in `storage.session` (MV3 workers are killed at any time, so nothing lives only in memory) |
| `downloads` | Take over Chrome's downloads (ON by default): cancel + erase Chrome's copy only after the app accepted it |
| `contextMenus` | "Download with IDM Replica" |
| `cookies` | Read the cookies for **the one URL being sent**, so logged-in downloads work. They are sent only to the app on 127.0.0.1 for that job, never stored or logged by the extension, and never saved to disk by the app |
| host `<all_urls>` | Sniff media on any site, run the content script in all frames, read tab URLs, read cookies for the media URL, and reach `http://127.0.0.1` |

Deliberately **not** requested:
- `tabs` and `activeTab`: the host permission already gives tab URLs.
- `scripting`: the content script is declared in the manifest, and toasts are messaged to it.
- `webRequestBlocking`, `nativeMessaging`: not needed.

## Privacy

- Nothing leaves your computer except requests to IDM Replica at `127.0.0.1`. There is
  no telemetry, no analytics, no remote code, and no external servers.
- Cookies are read only when you send a download, only for that URL, and passed only
  to the app for that job.
- The token is stored in `chrome.storage.local` and shown masked. It is sent only in
  the `X-IDM-Token` header to 127.0.0.1.
- The per-tab media list lives in session storage and is cleared on navigation, tab
  close, or browser restart.

## Troubleshooting

| Message | Fix |
|---|---|
| "IDM Replica isn't running. Start IDM Replica and try again." | Start the app. If Chrome was asked to download, it already did so normally |
| "IDM Replica doesn't allow this extension yet. Paste this ID …" | Add the ID in the app → Browser integration → Allowed extension IDs → Save IDs |
| "Wrong token …" / "Not paired yet …" | Copy token in the app → paste in Options → Test connection. Regenerating the token in the app breaks the old one |
| "IDM Replica is running on port X, but Options says Y" | Put X in Options. The app moves to 38918–38927 if 38917 is busy, e.g. when a second copy of the app is running |
| "Something else is answering on port …" | Another program uses that port. Check the port shown in the app |
| "This site uses protected video …" | DRM. Not downloadable by design (also reported on the app's card for DRM HLS streams) |
| "IDM Replica needs yt-dlp / ffmpeg …" | Install the tool (the app shows the command), then restart the app |
| "IDM Replica is busy …" | More than 50 browser requests are waiting for Accept/Dismiss in the app |
| Toolbar shows **!** after a download | Chrome kept that download; hover the icon for why (app not running, not paired, blob/POST download) |
| No button appears | The video must be playing, or have a source, and be at least 200×120 px. Check Options → floating button and the per-site list. In fullscreen the button is only shown when the player (not the bare `<video>`) is fullscreen |
| Popup is empty | Start playing the video. Some sites only load media on play. After updating the extension, reload the page |

## Manual test (local test page)

```bash
cd extension
npm run serve-test
```

This serves `http://127.0.0.1:8080/test-page.html` with HTTP Range support, and makes
the test videos with ffmpeg on the first run. Then, with the app running and paired:

1. **Test page:** the badge shows **6** after about 5 s. The popup lists `sample.mp4`,
   `index.m3u8` (HLS, once — no `.ts` segments), `iframe.mp4`, `blob-source.mp4`,
   `dynamic.mp4` (it appears about 3 s after load) and one "Page stream" entry.
   `tiny.mp4` must **not** be listed.
2. **Hover the first video:** **⬇ Download** appears at its top-right, and clicking the
   video itself still pauses/plays it. Open the menu → **Download page video (best)**.
   You get the toast *"Sent to IDM Replica — confirm it in the app."* and the app shows
   an Accept/Dismiss card for `sample.mp4`.
3. **Popup → the "Page stream" row → Download:** the blob explanation appears.
4. **Right-click the "Download test-2mb.bin" link → Download with IDM Replica:** a
   card appears in the app.
5. **HLS stream (section 7):** hover the HLS video (or use the popup) → **index.m3u8** →
   Accept the card in the app. The card fills in and ends as `index.mp4` (~12 s,
   h264 + aac) in your download folder.
6. **YouTube:** open a watch page.
   - The badge shows **1**: a page stream named after the video.
   - Hover the player → **⬇ Download** → **Download page video (best)**. The app
     receives the watch URL (yt-dlp).
   - Click a recommended video. The list switches to the new video (no stale entry)
     and the button follows. The same applies to `/shorts/…`.
7. **Take-over (default ON).** Each should start in the app with no confirm card, and
   leave nothing in Chrome's download list (`chrome://downloads`):
   - a **.zip**: the test page link `test-2mb.bin`, or any zip on the web;
   - an **.exe**: e.g. a 7-Zip or Python installer from its official site;
   - a **.pdf**: on a page where Chrome *downloads* the PDF (a "download" link), not one
     it opens in its viewer;
   - a **file behind a login**: a private attachment from a site you're logged into.
     The app must get it: the cookies for that domain are forwarded.
   - a **blob download**: e.g. "Export/Save" in a web app that builds the file in the
     page. Chrome must keep it, and the toolbar shows **!** ("only exists inside the page").
   - **with the app closed**: click the zip again. Chrome downloads it normally, and the
     toolbar shows **!** "IDM Replica: not running — Chrome handled this download".

