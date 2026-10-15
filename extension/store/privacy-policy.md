# IDM Replica browser extension — Privacy Policy

_Last updated: 2026-10-15_

The IDM Replica extension sends downloads and videos you choose to the IDM Replica
desktop app on **your own computer**.

## What the extension handles

- **The address (URL) of what you download**, the page it came from, the file name and
  your browser's user agent.
- **Cookies for that one address**, so downloads that need you to be signed in work.
- **A list of media files and video playlists the current page loaded**, shown in the
  extension's popup. It is kept in memory and cleared when you close the tab.
- **Its settings and a pairing token** from the desktop app, in your browser's local
  extension storage.

## Where it goes

Only to the IDM Replica app on the same computer, at `127.0.0.1` (the loopback address,
which never leaves your machine). The extension contacts no other server.

- No analytics, telemetry, crash reports or advertising.
- No account, sign-in or tracking identifiers.
- Nothing is sold, rented or shared with anyone.
- No remote code is loaded.

The desktop app then downloads the file from the site you were on, as your browser would,
and keeps a download history on your computer (you can clear it in the app; the
uninstaller can delete it). The app never writes cookie values to disk.

## Your control

- Nothing is sent until you click Download, use the right-click menu, or turn on
  "take over browser downloads".
- Removing the extension deletes its settings and token.
- The app can revoke the token at any time (Browser integration → regenerate token).

## Contact

TODO before publishing: the public repository URL and a contact address. Both stores
require a reachable contact and a public URL where this policy is hosted.
