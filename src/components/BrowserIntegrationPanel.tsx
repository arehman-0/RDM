import { useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import { bridgeStatusLine, parseExtensionIds } from "../lib/bridge";
import type { BridgeInfo, BridgeSettings } from "../types";

interface Props {
  info: BridgeInfo | null;
  flash: (msg: string) => void;
}

/** Settings for the localhost bridge the browser extension talks to. */
export function BrowserIntegrationPanel({ info, flash }: Props) {
  const [open, setOpen] = useState(false);
  const [showToken, setShowToken] = useState(false);
  // null = not editing: show the backend's list.
  const [idsDraft, setIdsDraft] = useState<string | null>(null);

  // The backend answers with a `bridge-status` event; nothing is set locally.
  const save = (settings: BridgeSettings) =>
    invoke<BridgeInfo>("set_bridge_settings", { settings }).then(
      () => true,
      (e) => {
        flash(String(e));
        return false;
      }
    );

  const copyToken = () => {
    if (!info) return;
    navigator.clipboard.writeText(info.token).then(
      () => flash("Token copied — paste it into the extension's options"),
      () => {
        setShowToken(true);
        flash("Clipboard unavailable — copy the token from the field");
      }
    );
  };

  const regenerate = () => {
    invoke<BridgeInfo>("regenerate_token").then(
      () => flash("New token created — update it in the extension"),
      (e) => flash(String(e))
    );
  };

  const idsText = idsDraft ?? info?.settings.allowedExtensionIds.join("\n") ?? "";

  return (
    <section className="controls bridge-panel">
      <button className="linklike" onClick={() => setOpen(!open)}>
        {open ? "▾ Browser integration" : "▸ Browser integration"}
        <span className={`bridge-dot ${info?.listening ? "on" : "off"}`} aria-hidden />
        <span className="bridge-status-inline">{bridgeStatusLine(info)}</span>
      </button>
      {open && info && (
        <div className="bridge-body">
          <label className="check-wrap">
            <input
              type="checkbox"
              checked={info.settings.enabled}
              onChange={(e) => save({ ...info.settings, enabled: e.target.checked })}
            />
            <span>Accept downloads from the browser extension (127.0.0.1 only)</span>
          </label>
          <label className="check-wrap">
            <input
              type="checkbox"
              checked={info.settings.askBeforeStart}
              onChange={(e) => save({ ...info.settings, askBeforeStart: e.target.checked })}
            />
            <span>Ask before starting downloads from the browser (button, menu, popup)</span>
          </label>
          <label className="check-wrap">
            <input
              type="checkbox"
              checked={info.settings.confirmTakeover}
              onChange={(e) => save({ ...info.settings, confirmTakeover: e.target.checked })}
            />
            <span>Confirm browser take-over downloads (links you click in Chrome)</span>
          </label>

          <div className="bridge-row">
            <span>
              Port: <b>{info.port ?? "—"}</b>
            </span>
            <button onClick={copyToken} disabled={!info.token}>
              Copy token
            </button>
            <button className="ghost" onClick={() => setShowToken(!showToken)}>
              {showToken ? "Hide token" : "Show token"}
            </button>
            <button
              className="ghost"
              onClick={regenerate}
              title="The old token stops working at once"
            >
              Regenerate token
            </button>
          </div>
          {showToken && (
            <input
              className="dir-input bridge-token"
              readOnly
              value={info.token}
              onFocus={(e) => e.target.select()}
              spellCheck={false}
            />
          )}

          <label className="dir-wrap">
            <span>
              Allowed extension IDs — one per line (from chrome://extensions). Empty = refuse all
              browser requests (curl without an Origin still works with the token).
            </span>
            <textarea
              className="dir-input area"
              rows={2}
              value={idsText}
              placeholder="abcdefghijklmnopabcdefghijklmnop"
              onChange={(e) => setIdsDraft(e.target.value)}
              spellCheck={false}
            />
          </label>
          {idsDraft !== null && (
            <div className="bridge-row">
              <button
                onClick={() =>
                  save({ ...info.settings, allowedExtensionIds: parseExtensionIds(idsDraft) }).then(
                    (ok) => ok && setIdsDraft(null)
                  )
                }
              >
                Save IDs
              </button>
              <button className="ghost" onClick={() => setIdsDraft(null)}>
                Discard
              </button>
            </div>
          )}

          <div className={`status-text ${info.error ? "error-line" : ""}`}>
            {bridgeStatusLine(info)}
          </div>
        </div>
      )}
    </section>
  );
}
