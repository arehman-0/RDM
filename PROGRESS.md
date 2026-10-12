# PROGRESS — session 8 (2026-10-10): clean-install prep — PARTLY DONE

Branch: `clean-install-prep` (pushed). `main` is unchanged. Start the next session from this branch.

## Done and tested in the real app
- **Component manager** (`src-tauri/src/components.rs`, pinned manifest `src-tauri/components.json`):
  finds yt-dlp / QuickJS-NG / FFmpeg in the per-user components dir → bundled → PATH. First-run
  "Setting up components" screen downloads what is missing (resumable, SHA-256 checked before
  install). yt-dlp daily update with checksum check, smoke test and rollback. 12 unit tests.
- **Clean-state test:** tools hidden from PATH + empty components dir → setup finished in ~50 s,
  hashes matched, a YouTube video extracted (QuickJS), downloaded and merged.
- **JS runtime:** yt-dlp's docs require one for YouTube. QuickJS-NG added: 2.1 MB (Deno ≈ 100 MB).
- **Pairing** `POST /v1/pair`: no/bad Origin → 403 and no prompt; Deny → no token; Allow → token.
  3 attempts/min, one prompt at a time. Extension has a "Connect to IDM Replica" button.
- **Tray:** closing hides the window; app + bridge keep running; second launch shows it again.
- **Start with Windows:** HKCU Run value appears when on, is removed when off (`reg query`).
- Extension ID ships pre-allowed; manual token / ID fields moved under Advanced.
- `THIRD_PARTY_NOTICES.md` + `scripts/gen-third-party-notices.mjs`; FFmpeg LGPL text in `licenses/`.
- Last full check run (before one small licence-text edit): cargo fmt/clippy/test **116**,
  root tsc + npm test **52**, extension typecheck + vitest **39** + build — all green.

## Size table (release exe, measured)
| Build | Size |
|---|---|
| Before this session (opt 3, abort) | 5.33 MB |
| New code + trims, opt 3, abort (**current profile**) | 4.22 MB |
| Same, panic = unwind | 7.11 MB |
| unwind + opt-level "s" | 5.15 MB |
| unwind + opt-level "z" | 4.69 MB |
Trims: no regex log filter, Windows' own TLS instead of bundled rustls, tokio feature subset,
`num_cpus` removed. Top crates by size (cargo-bloat): std, this app, tauri, tokio, TLS, tao.

## REMAINING from the session-8 prompt
1. **Benchmark** the 500 MB local download for opt 3 vs "s"/"z" (script exists only in the old
   session's temp folder — rewrite it), then decide `opt-level`. Not run.
2. **Decide panic strategy:** still `panic = "abort"` (smallest; safe for data, but any panic
   closes the app). Unwind costs ~0.5–3 MB.
3. **Tray menu** Quit / Pause all / Resume all: written, never clicked. One-time tray notice
   dialog was not seen on screen (its flag was saved) — check by hand.
4. **Re-run all checks** after the last edit, and regenerate the notices.
5. **Docs:** `docs/architecture/*.mmd`, `NOTES_extension.md`, `CLAUDE.md` key-files table.
6. **Frontend size item:** Vite bundle analysis not done (bundle is 171 KB JS + 9 KB CSS, minified,
   no source maps).
7. **FFmpeg item 6:** list of ffmpeg features used + a `--disable-everything` configure line
   not written up.
8. **Manual steps (a)–(d)** write-up for you. Unix build not compiled this session.
9. Preflight does not check the JS runtime separately (only yt-dlp + ffmpeg).

## Needs your decision
- FFmpeg: pinned BtbN LGPL shared build = 77 MB download, 156 MB on disk. A custom mux-only
  build would be a few MB but must be built by us.
- yt-dlp.exe is GPLv3+ as a bundled binary (run unmodified as a separate program; noted).
- Keep Windows TLS (smaller) or go back to bundled rustls.
- panic abort vs unwind (see size table).

## Next step
Run item 4, then items 1–2, then build the installer (the session after).
