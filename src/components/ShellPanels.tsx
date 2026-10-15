import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import { describeOutcome, sourceText } from "../lib/components";
import type { AppSettings, ComponentsInfo, PairingPrompt as Prompt, YtdlpInfo } from "../types";

/** Settings: the video tools, their versions, and yt-dlp updates. */
export function ComponentsPanel({
  info,
  onSetup,
  flash,
}: {
  info: ComponentsInfo | null;
  onSetup: () => void;
  flash: (msg: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [yt, setYt] = useState<YtdlpInfo | null>(null);
  const [checking, setChecking] = useState(false);

  // The running version is only asked for while the panel is open.
  useEffect(() => {
    if (open) {
      invoke<YtdlpInfo>("ytdlp_version")
        .then(setYt)
        .catch(() => {});
    }
  }, [open, info]);

  if (!info) return null;
  const missing = info.missing.length;

  const check = () => {
    setChecking(true);
    invoke<YtdlpInfo>("check_ytdlp_update")
      .then((r) => {
        setYt(r);
        flash(describeOutcome(r.outcome));
      })
      .catch((e) => flash(String(e)))
      .finally(() => setChecking(false));
  };

  return (
    <section className="controls bridge-panel">
      <button className="linklike" onClick={() => setOpen(!open)}>
        {open ? "▾ Video components" : "▸ Video components"}
        <span className={`bridge-dot ${missing === 0 ? "on" : "off"}`} aria-hidden />
        <span className="bridge-status-inline">
          {missing === 0 ? "Ready" : `${missing} not installed`}
        </span>
      </button>
      {open && (
        <div className="bridge-body">
          {info.components.map((c) => (
            <div className="bridge-row" key={c.id} title={c.purpose}>
              <b>{c.name}</b>
              <span>
                {c.id === "yt-dlp" && yt?.version ? yt.version : c.installedVersion ?? c.pinnedVersion}
              </span>
              <span>· {sourceText(c.source)}</span>
              <span>· {c.license}</span>
            </div>
          ))}
          {missing > 0 && (
            <div className="bridge-row">
              <button onClick={onSetup}>Install missing components</button>
            </div>
          )}
          <label className="check-wrap">
            <input
              type="checkbox"
              checked={info.autoUpdate}
              onChange={(e) =>
                invoke("set_ytdlp_auto_update", { enabled: e.target.checked }).catch(() => {})
              }
            />
            <span>Keep yt-dlp up to date (checked once a day)</span>
          </label>
          <div className="bridge-row">
            <button onClick={check} disabled={checking}>
              {checking ? "Checking…" : "Check now"}
            </button>
            <span>yt-dlp version: {yt?.version ?? "not installed"}</span>
          </div>
        </div>
      )}
    </section>
  );
}

/** Settings: start with Windows / start minimized. */
export function StartupPanel({
  settings,
  onChange,
  onExtensionSetup,
}: {
  settings: AppSettings | null;
  onChange: (next: Pick<AppSettings, "startWithWindows" | "startMinimized">) => void;
  onExtensionSetup: () => void;
}) {
  if (!settings) return null;
  return (
    <section className="controls">
      <label className="check-wrap">
        <input
          type="checkbox"
          checked={settings.startWithWindows}
          onChange={(e) =>
            onChange({ startWithWindows: e.target.checked, startMinimized: settings.startMinimized })
          }
        />
        <span>Start with Windows</span>
      </label>
      <label className="check-wrap">
        <input
          type="checkbox"
          checked={settings.startMinimized}
          disabled={!settings.startWithWindows}
          onChange={(e) =>
            onChange({ startWithWindows: settings.startWithWindows, startMinimized: e.target.checked })
          }
        />
        <span>Start minimized to the tray (when started with Windows)</span>
      </label>
      <span className="dim small">
        Closing the window keeps IDM Replica in the tray; use Quit in the tray menu to exit.
      </span>
      <button className="linklike" onClick={onExtensionSetup}>
        Install the browser extension…
      </button>
    </section>
  );
}

/** "Allow this extension?" — shown once per pairing request, never stacked. */
export function PairingPrompt({ prompt }: { prompt: Prompt | null }) {
  if (!prompt) return null;
  const answer = (allow: boolean) =>
    invoke("answer_pairing", { id: prompt.id, allow }).catch(() => {});
  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" aria-label="Allow browser extension">
        <h2>Allow {prompt.browser} extension?</h2>
        <p>
          The IDM Replica extension in {prompt.browser} wants to send downloads to this app.
        </p>
        <p className="dim small">Extension ID: {prompt.extensionId}</p>
        <div className="modal-actions">
          <button className="primary" onClick={() => answer(true)}>
            Allow
          </button>
          <button className="ghost" onClick={() => answer(false)}>
            Deny
          </button>
        </div>
      </div>
    </div>
  );
}
