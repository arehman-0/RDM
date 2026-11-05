# Permission justifications (paste into the store's "Privacy practices" form)

The extension only talks to the IDM Replica desktop app on this computer
(`http://127.0.0.1:38917`, or the next free port up to 38927). It sends nothing to any
other server, has no analytics, and loads no remote code.

**Single purpose:** send the downloads and videos the user chooses on web pages to the
IDM Replica download manager running on the same computer.

| Permission | Why it is needed | Where in the code |
|---|---|---|
| `host_permissions: <all_urls>` | The user can download from any site. Three features depend on it: (1) the content script shows a Download button on the videos the user plays; (2) `webRequest` sees which media files and HLS/DASH playlists a page loads, so the popup can list them; (3) `cookies` reads the cookies for the one URL the user sends, so downloads behind a login work. A fixed site list would break all of these on every other site. | `manifest.json`, `background.ts`, `content.ts` |
| Content script on `<all_urls>` (`all_frames`) | Finds `<video>` elements, including ones inside embedded players (iframes), and draws the floating Download button. It reads no page text and sends nothing until the user clicks. | `content.ts` |
| `webRequest` | Observes (never blocks or changes) requests and response headers, to spot media files and stream playlists a page loads, which are often not in the page markup. The list stays in memory per tab and is cleared when the tab closes. | `background.ts` (`onHeadersReceived`, `onBeforeRequest`) |
| `downloads` | Optional take-over (a setting): when Chrome starts a download, hand it to the app and cancel Chrome's copy. If the app is not running, Chrome keeps the download. | `background.ts` (`onCreated`, `onDeterminingFilename`, `cancel`, `erase`) |
| `cookies` | Reads the cookies Chrome would send to the exact URL being downloaded and passes them to the local app only, so it can fetch files that need a login. Never stored by the extension; the app never writes them to disk. | `lib/request.ts`, `background.ts` |
| Player probe: content script in the page's `MAIN` world (`page-probe.js`, `document_start`, all frames) | Many video players load their HLS/DASH playlist from a URL that doesn't look like one (e.g. `.txt`) and play it through a `blob:` MediaSource, so the network sniffer can't tell what is playing. The probe wraps `fetch`/`XMLHttpRequest` only to read the **first 4 KB of small, non-media responses**; if one starts with `#EXTM3U` or `<MPD`, its URL is posted to the extension's own content script in the same frame. It changes, blocks and sends nothing else, and responses reach the page untouched. No new permission is needed. | `page-probe.ts`, `lib/probe.ts` |
| `contextMenus` | The right-click item "Download with IDM Replica" on links, images, audio and video. | `background.ts` |
| `storage` | The extension's settings (port, take-over on/off, file types) and the pairing token from the app, in local extension storage. | `lib/settings.ts`, `options.ts` |

Not requested: `tabs` (only `tabs.query`, `tabs.get`, `tabs.sendMessage` and tab events
are used, which need no permission; tab URLs are covered by the host permission),
`scripting`, `nativeMessaging`, `history`, `bookmarks`, `management`,
`declarativeNetRequest`.

## Review (2026-10-15)

Each permission was checked against the code: every one is used, and removing any of
them breaks a feature above. **No permission was removed.** A possible further reduction
is to make `cookies` an optional permission asked for at the first download that needs a
login; that changes the user flow, so it is left as a proposal.

## Remote code

None. esbuild bundles every script into the package; the extension CSP is
`script-src 'self'; object-src 'self'`.
