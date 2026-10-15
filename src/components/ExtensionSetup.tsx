import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import { manualSteps, preferredBrowser, storeButtons } from "../lib/extensionSetup";
import type { ExtensionSetupInfo, InstalledBrowser } from "../types";

/**
 * "Install the browser extension": store pages when published, else the
 * manual (developer mode) path. Chrome installs extensions only with the
 * user's consent, so this only opens pages and folders.
 */
export function ExtensionSetup({ onDone, flash }: { onDone: () => void; flash: (msg: string) => void }) {
  const [info, setInfo] = useState<ExtensionSetupInfo | null>(null);
  const [picked, setPicked] = useState<InstalledBrowser | null>(null);
  const [manual, setManual] = useState(false);

  useEffect(() => {
    invoke<ExtensionSetupInfo>("extension_setup_info")
      .then((i) => {
        setInfo(i);
        setPicked(preferredBrowser(i.browsers));
      })
      .catch(() => {});
  }, []);

  if (!info) return null;
  const run = (cmd: string, args: Record<string, unknown> = {}) =>
    invoke(cmd, args).catch((e) => flash(String(e)));

  return (
    <div className="modal-backdrop">
      <div className="modal wide" role="dialog" aria-label="Install the browser extension">
        <h2>Install the browser extension</h2>
        <p>
          The extension hands your browser’s downloads and videos to IDM Replica. Browsers only install
          extensions you approve yourself.
        </p>
        <p className="dim small">
          {info.browsers.length > 0
            ? `Found: ${info.browsers.map((b) => b.name).join(", ")}`
            : "No Chrome, Edge or Brave found. The extension works in Chromium-based browsers."}
        </p>
        <div className="modal-actions">
          {storeButtons(info).map((b) => (
            <button
              key={b.store}
              disabled={!b.available}
              title={b.available ? undefined : "Not published yet — use Manual install"}
              onClick={() => run("open_extension_store", { store: b.store })}
            >
              {b.label}
            </button>
          ))}
          <button onClick={() => setManual(!manual)}>Manual install (developer mode)</button>
        </div>
        {manual && (
          <div className="bridge-body">
            {info.browsers.length > 1 && (
              <div className="bridge-row">
                {info.browsers.map((b) => (
                  <label className="check-wrap" key={b.id}>
                    <input type="radio" checked={picked?.id === b.id} onChange={() => setPicked(b)} />
                    <span>{b.name}</span>
                  </label>
                ))}
              </div>
            )}
            <ol>
              {manualSteps(picked, info.unpackedDir).map((s) => (
                <li key={s}>{s}</li>
              ))}
            </ol>
            <div className="bridge-row">
              {picked && (
                <button onClick={() => run("open_browser_extensions_page", { browser: picked.id })}>
                  Open {picked.id}://extensions
                </button>
              )}
              <button disabled={!info.unpackedDir} onClick={() => run("reveal_unpacked_extension")}>
                Show the extension folder
              </button>
            </div>
          </div>
        )}
        <div className="modal-actions">
          <button className="primary" onClick={onDone}>
            Done
          </button>
          <button className="ghost" onClick={onDone}>
            Later
          </button>
        </div>
      </div>
    </div>
  );
}
