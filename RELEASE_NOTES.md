# Release notes

## 0.1.0 — first Windows installer (unreleased, 2026-10-15)

First build meant for other people's PCs. Windows 10/11 x64.

### Install
- Per-user NSIS installer, no admin prompt, `%LOCALAPPDATA%\IDM Replica`.
- **LITE** (~1.7 MB): the app downloads yt-dlp, QuickJS-NG and FFmpeg on first run and
  verifies each SHA-256. **FULL** (~66 MB): the same pinned tools inside, for offline PCs.
- WebView2 is installed by Microsoft's bootstrapper only if it is missing.
- Start-menu shortcut, optional desktop shortcut, license page (MIT + third-party notices).
- Uninstall removes the program, shortcuts, its registry keys and the *Start with Windows*
  entry; optionally the history, settings and video tools. Downloaded files are kept.

### New
- *Install the browser extension* step in the installer and on the app's first run:
  store buttons (enabled once published), and a manual developer-mode path with the
  unpacked extension shipped in the install folder. Detects Chrome, Edge and Brave.
- Extension store kit (`extension/store/`): store zip without the dev `key`, listing text,
  privacy policy, permission justifications.
- Video pages are refused up front with `js_runtime_missing` if no JavaScript runtime is
  available for yt-dlp.
- The tray "still running" notice now appears on top.

### Changed
- Release build: `opt-level = "s"`, `panic = "unwind"` (app exe 5.15 MB).

### Known limitations
- Not code-signed: SmartScreen warns on first run.
- The extension is not on the Chrome Web Store or Edge Add-ons yet; use Manual install.
- No in-app updater for the app itself (yt-dlp does update itself).
