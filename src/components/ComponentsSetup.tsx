import { invoke } from "@tauri-apps/api/tauri";
import { fmtBytes } from "../lib/format";
import { percent, phaseText, type ProgressMap } from "../lib/components";
import type { ComponentsInfo } from "../types";

interface Props {
  info: ComponentsInfo;
  progress: ProgressMap;
  onSkip: () => void;
}

/**
 * First-run screen: downloads the video tools (yt-dlp, QuickJS, FFmpeg) that
 * are missing, with a bar per tool. Plain file downloads don't need them, so
 * it can be skipped.
 */
export function ComponentsSetup({ info, progress, onSkip }: Props) {
  const failed = !info.settingUp && info.error !== null;
  const idle = !info.settingUp && info.error === null;

  return (
    <div className="modal-backdrop">
      <div className="modal setup" role="dialog" aria-label="Setting up components">
        <h2>Setting up components</h2>
        <p className="dim">
          IDM Replica needs a few tools to download videos from sites like YouTube. They are
          downloaded once ({fmtBytes(info.missingBytes)}), checked against their published
          hashes, and kept up to date for you.
        </p>
        <div className="setup-list">
          {info.components.map((c) => {
            const p = progress[c.id];
            const installed = c.source !== null;
            const pct = installed ? 100 : percent(p);
            return (
              <div className="setup-row" key={c.id}>
                <div className="setup-head">
                  <b>{c.name}</b>
                  <span className="dim">
                    {phaseText(p, installed)}
                    {!installed && p?.phase === "downloading" && p.total > 0
                      ? ` ${fmtBytes(p.downloaded)} / ${fmtBytes(p.total)}`
                      : ""}
                  </span>
                </div>
                <div className="bar-outer">
                  <div className="bar-inner" style={{ width: `${pct}%` }} />
                </div>
                <div className="dim small">{c.purpose}</div>
              </div>
            );
          })}
        </div>
        {failed && <div className="error-line">Setup stopped: {info.error}</div>}
        <div className="modal-actions">
          {(failed || idle) && (
            <button className="primary" onClick={() => invoke("install_components").catch(() => {})}>
              {failed ? "Retry" : "Install"}
            </button>
          )}
          <button className="ghost" onClick={onSkip}>
            {info.settingUp ? "Continue in the background" : "Skip for now"}
          </button>
        </div>
      </div>
    </div>
  );
}
