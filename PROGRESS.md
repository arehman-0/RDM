# PROGRESS — session 8 (2026-10-10): clean-install prep (branch `clean-install-prep`, NOT finished)

Stopped early (usage limit). Work is committed on the branch; nothing pushed.

## Done and verified (actually run)
- **Component manager** (`components.rs`, `components.json`): resolver order components dir →
  bundled → PATH; pinned yt-dlp 2026.08.19, QuickJS-NG 0.17.0 (yt-dlp's docs require a JS
  runtime for YouTube; 2.1 MB vs ~100 MB for Deno), FFmpeg n9.0.2 LGPL shared (BtbN, 77 MB zip,
  156 MB unpacked). Resumable download → SHA-256 → move into place; zip extraction by file name;
  yt-dlp daily update with checksum list, smoke test and rollback. 12 tests (local axum server).
- **Real clean-state test**: tools hidden from PATH, empty components dir → setup screen
  downloaded all three in ~50 s, hashes match; YouTube video extracted via QuickJS (4.5 s),
  downloaded and merged (19 s AV1+AAC).
- **Pairing** `POST /v1/pair`: real UI prompt; bad/no Origin → 403 no prompt; Deny → no token;
  Allow → token. Rate limit 3/min, one prompt at a time. Extension "Connect" button + tests.
- **Tray**: close → hides, process and bridge stay up, second launch re-shows the window.
  **Start with Windows**: HKCU Run value appears / disappears (checked with `reg query`).
- Shipped extension ID pre-allowed; manual token/ID moved under Advanced.
- THIRD_PARTY_NOTICES.md + generator script; FFmpeg LGPL text in `licenses/`.
- Checks green at last run: cargo fmt/clippy/test (116), root tsc + npm test (52),
  extension typecheck + vitest (39) + build.

## Size so far
| Build | exe |
|---|---|
| Before (session 7 code) | 5,586,944 B |
| New features + trims (no regex log filter, Windows TLS instead of rustls, tokio subset), opt 3, abort | 4,423,680 B |
Variants unwind / opt-s / opt-z were still building when I stopped.

## NOT done
- Size table for unwind/s/z and the 500 MB throughput benchmark (script written, not run);
  `[profile.release]` still has `panic = "abort"` — decision pending the numbers.
- Tray menu Quit / Pause all not clicked; the one-time tray notice dialog was not seen on
  screen (flag was saved) — check it by hand.
- Final full re-run of all checks after the last edits (licence text in components.json).
- Docs: architecture diagrams, NOTES_extension.md, manual steps write-up.

## Needs decision
- FFmpeg: prebuilt LGPL is 156 MB on disk; a custom `--disable-everything` build (mux/demux only)
  would be a few MB. yt-dlp.exe is GPLv3+ as a bundled binary (run as a separate program).
- Windows now uses schannel TLS (smaller exe); say if you prefer bundled rustls back.
