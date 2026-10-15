# PROGRESS — session 10 (2026-10-15): Windows installer

Branch: `installer` (from `main`), committed locally, **not pushed**. Nothing published.

## Installers (built here, `release\`)
| File | Size | SHA-256 |
|---|---|---|
| `IDM-Replica-0.1.0-x64-lite-setup.exe` | 1,815,416 B (1.73 MB) | `286353cf…87cb` |
| `IDM-Replica-0.1.0-x64-full-setup.exe` | 69,614,584 B (66.39 MB) | `dcf2d7fc…340d` |
Full hashes: `release\SHA256SUMS.txt`. Breakdown (uncompressed → in the installer, LZMA solid):
app exe 5.23 MB + unpacked extension 0.09 MB + NSIS stub/plugins ≈ 1.73 MB (LITE);
FULL adds yt-dlp 17.0 + QuickJS 2.1 + FFmpeg (exe + 7 DLLs) 149.5 MB = 168.5 MB → +64.7 MB.
App exe grew 5.15 → 5.23 MB (extension setup + component adoption code).

## Done
- NSIS, per-user (`%LOCALAPPDATA%\IDM Replica`, no admin), WebView2 download bootstrapper,
  publisher/copyright/descriptions, icon, English, license page = LICENSE + notices.
  Template = Tauri v1.6's own + marked `IDM:` changes: extension page, upgrade-safe
  uninstall (`/UPDATE` keeps the Run value), removes Run value + all its HKCU keys,
  clearer "also delete data" box, silent `/DELETEAPPDATA`. App registers no protocols.
- `scripts/build-installer.ps1 -Variant lite|full`, `fetch-components.ps1` (SHA-256 + size,
  fails loudly), `check-versions.mjs`, `smoke-test.ps1`. Binaries/outputs git-ignored.
- FULL: app moves bundled tools into the components dir on first run (pinned hash for
  single-file tools) so yt-dlp stays updatable. Component downloads now retry 3× with resume.
- Extension: `extension/store/` (zip without `key`, listing, privacy policy, permission
  justifications), `src-tauri/extension-store.json` = single config for store IDs/URLs
  (TODO until published), browser detection (Chrome/Edge/Brave via App Paths), first-run
  "Install the browser extension" screen + installer page. No policies/registry installs.
- CI: Linux (cfg(unix) clippy+tests) + Windows (all tests, both installers, smoke tests,
  artifacts incl. SHA256SUMS.txt, Rust pinned 1.99.0). Docs: `docs/SIGNING.md`,
  `docs/CLEAN_INSTALL_TEST.md`, README (install, FAQ), `RELEASE_NOTES.md`, updater eval.

## Verified (how)
- DoD run: npm ci, tsc, npm test **55**, cargo fmt/clippy/test **127**, extension
  typecheck + vitest **39** + build, versions aligned, both installer builds — green.
- `smoke-test.ps1` on the final builds: FULL **23/23**, LITE **22/22** (silent install, bridge
  ping, tools present + hashes, ffmpeg runs, tray Quit, silent uninstall, no files/keys/
  shortcuts/Run value/app data left). Your data was moved aside and restored (hash-checked).
- First-run extension screen seen in the installed app (screenshot): found Chrome + Edge,
  store buttons disabled. Store zip opened with .NET: 12 files, no `key`.

## NOT done / not verified
- Real clean PC / Windows Sandbox (this PC has WebView2, Chrome, Edge, tools on PATH).
- The installer's GUI pages (license, extension page, desktop checkbox, uninstall
  checkbox) were not clicked through; only /S paths ran. The manual-install buttons
  (opening chrome://extensions, the folder) were not clicked in app or installer.
- Store flow (accounts, review, real IDs), signing, CI workflow itself (not run on GitHub).
- One LITE smoke run failed before the retry fix (FFmpeg did not arrive in 10 min; cause
  not logged); one background build failed once with "nul byte… RC.EXE", not reproducible.
- No permission could be removed from the extension (all used; see store/permissions.md).

## Decisions needing your approval
- **Product name for the stores**: "IDM" may be read as Internet Download Manager.
- Publisher shown in the installer: "Abdul Rehman" (from LICENSE). OK?
- Signing route (SignPath vs Azure Trusted Signing vs certificate) — `docs/SIGNING.md`.
- App auto-update: off; proposal in `NOTES_extension.md`.
- FULL moves (not copies) the tools out of the install folder to avoid 2× disk use.
- Still open: custom FFmpeg build; tray test hook in release; manifest jobs skip JS check.
**Next step:** run `docs/CLEAN_INSTALL_TEST.md` in Windows Sandbox, decide name/signing.
