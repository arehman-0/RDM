# Clean-install test (real PC or Windows Sandbox)

Automated part: `scripts/smoke-test.ps1` (silent install → bridge ping → components →
Quit → silent uninstall → leftover check). The steps below cover what a script can't:
the UI, the extension, real sites, reboot.

## 0. Windows Sandbox (Windows 10/11 Pro, Enterprise or Education)

A throwaway clean Windows: everything is deleted when you close it.

1. Enable it once (admin PowerShell), then reboot:
   ```powershell
   Enable-WindowsOptionalFeature -Online -FeatureName Containers-DisposableClientVM -All
   ```
2. Start **Windows Sandbox** from the Start menu.
3. Copy `IDM-Replica-<version>-x64-lite-setup.exe` (or `-full-`) and
   `SHA256SUMS.txt` from your PC and paste them into the Sandbox desktop (Ctrl+C / Ctrl+V
   works across the window).
4. Optional check inside the Sandbox:
   `Get-FileHash .\IDM-Replica-*-setup.exe` → compare with `SHA256SUMS.txt`.

Notes: the Sandbox has Edge but no Chrome (install Chrome inside it if you want to test
Chrome), and no reboot (do step 9 on a real PC or VM). Windows 10 Home has no Sandbox:
use a spare PC or a Hyper-V/VirtualBox VM with a fresh Windows image.

## 1. Install

1. Double-click the installer. Expect SmartScreen (unsigned): **More info → Run anyway**.
2. No admin prompt should appear (per-user install).
3. Welcome → License (MIT + third-party notices) → install folder
   (`%LOCALAPPDATA%\IDM Replica`) → Start menu folder → Install.
   - On an old Windows 10 without WebView2 the installer downloads it first (needs internet).
4. **Install the browser extension** page: it lists the browsers it found. The two store
   buttons say "(not published yet)" until the store IDs are configured.
5. Finish: tick *Create desktop shortcut* if you want one; *Run IDM Replica*.

## 2. First run

- LITE: the **Setting up components** screen downloads yt-dlp, QuickJS and FFmpeg
  (~97 MB) and checks their SHA-256. Expect it to finish in about a minute on a normal
  connection. FULL: no download; the tools are ready at once.
- Then **Install the browser extension** appears (Done / Later). "Video components" in the
  main window should say *Ready*.

## 3. Downloads

1. A normal file: paste e.g. a large `.zip` or `.iso` URL → Download. Several thread bars
   move; Pause → Resume continues (does not restart).
2. An HLS stream: paste an `.m3u8` URL (e.g. a public test stream) → one card, finishes
   as a single `.mp4`.
3. A YouTube video: paste a watch URL → *Extracting* → two downloads → *Muxing* → one
   `.mp4` that plays with sound.

## 4. Extension (manual / developer-mode path)

1. In the app: **Install the browser extension… → Manual install (developer mode)**.
2. **Open edge://extensions** (or chrome://) → turn on **Developer mode** → **Load
   unpacked** → choose the folder **Show the extension folder** opened
   (`%LOCALAPPDATA%\IDM Replica\resources\extension`).
3. Click the extension's toolbar icon → **Connect to IDM Replica** → the app asks →
   **Allow**. The popup says *Connected*.
4. Click Connect again and choose **Deny** once: the popup says the connection was not
   allowed. Connect + Allow again.

## 5. Take-over

1. In the extension's Options, turn on taking over browser downloads.
2. In the browser, click a link to a `.zip`. The browser's download is cancelled and the
   app shows the download, starting at once.

## 6. Tray

1. Close the main window with X. First time: a message *IDM Replica is still running*
   appears **on top**; OK hides the window. The tray icon stays.
2. Start another `.zip` from the browser: it still goes to the app.
3. Right-click the tray icon: **Pause all**, **Resume all** (the download continues where it
   was), **Open IDM Replica**, then **Quit** with a download running. Reopen from the Start
   menu: the card says *Interrupted — resumable*; **Retry** continues from where it stopped.

## 7. Start with Windows (real PC/VM only)

1. Tick **Start with Windows** (and optionally *Start minimized*).
2. `reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v "IDM Replica"`
   shows the quoted exe path + `--autostart`.
3. Reboot and sign in: the app runs (in the tray if *Start minimized*); the bridge works
   (the extension popup says *Connected*).

## 8. Update over an existing install (optional)

Run a newer installer: it offers to uninstall the old version first; afterwards
*Start with Windows* is still on and the history is still there.

## 9. Uninstall

1. Settings → Apps → IDM Replica → Uninstall (or Start menu → uninstall).
2. The confirm page has *Also delete download history, settings and the downloaded video
   tools…*. Try once without, once with.
3. Check afterwards:
   - `%LOCALAPPDATA%\IDM Replica` is gone; Start menu and desktop shortcuts are gone.
   - `reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v "IDM Replica"` → not found.
   - `reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\IDM Replica"` → not found.
   - With the box ticked: `%APPDATA%\com.idmreplica.app` and
     `%LOCALAPPDATA%\com.idmreplica.app` are gone. Without it: they stay.
   - **Your downloaded files in Downloads are still there** in both cases.
4. Remove the unpacked extension from the browser yourself (the uninstaller can't, by design).

Record anything unexpected with the step number and a screenshot.
