import { useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import type { Settings } from "../hooks/useSettings";
import type { DownloadCard, StartResult } from "../types";
import { blankCard, groupId } from "../lib/cards";
import {
  guessName,
  isPrerequisiteError,
  parseHeaders,
  parseMirrors,
  scheduleToEpoch,
  validateUrl,
} from "../lib/input";

interface Props {
  settings: Settings;
  /** A download was accepted: seed its card so the UI feels instant. */
  onStarted: (card: DownloadCard) => void;
  flash: (msg: string) => void;
}

/** URL box + Save-As + optional schedule, and the error states around them. */
export function AddDownloadForm({ settings: s, onStarted, flash }: Props) {
  const [url, setUrl] = useState("");
  const [urlError, setUrlError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Explicit Save-As destination (full path) for the next download.
  const [savePath, setSavePath] = useState("");
  // Optional scheduled start ("YYYY-MM-DDTHH:mm", local time); "" = now.
  const [scheduleAt, setScheduleAt] = useState("");
  // yt-dlp / ffmpeg missing: kept on screen (with install hints) until dismissed.
  const [prereqError, setPrereqError] = useState<string | null>(null);

  const submit = async () => {
    const problem = validateUrl(url);
    if (problem) {
      setUrlError(problem);
      return;
    }
    const startAt = scheduleToEpoch(scheduleAt);
    if (startAt !== null && !Number.isFinite(startAt)) {
      flash("Invalid schedule time");
      return;
    }
    const scheduled = startAt !== null && startAt > Date.now() / 1000;
    const speedLimit = Math.round(s.speedLimitMB * 1024 * 1024);
    setBusy(true);
    try {
      const res = await invoke<StartResult>("start_download", {
        url: url.trim(),
        connections: s.connections,
        saveDir: s.saveDir || null,
        savePath: savePath || null,
        speedLimit,
        verify: s.verify,
        expectedChecksum: s.expectedChecksum.trim() || null,
        headers: parseHeaders(s.headersText),
        mirrors: parseMirrors(s.mirrorsText),
        startAt,
      });
      const status = scheduled ? "scheduled" : res.kind === "youtube" ? "extracting" : "starting";
      onStarted({
        ...blankCard(groupId(res.id), res.fileName, status),
        startAt: scheduled ? startAt : null,
        connections: s.connections,
        isYoutube: res.kind === "youtube",
        speedLimit,
      });
      setUrl("");
      setSavePath(""); // a Save-As path applies to a single download
      setScheduleAt(""); // and so does a schedule
      setPrereqError(null);
    } catch (e) {
      const msg = String(e);
      if (isPrerequisiteError(msg)) setPrereqError(msg);
      else flash(`Failed: ${msg}`);
    } finally {
      setBusy(false);
    }
  };

  const chooseSavePath = async () => {
    try {
      const picked = await invoke<string | null>("pick_save_path", {
        defaultName: guessName(url),
        defaultDir: s.saveDir || null,
      });
      if (picked) setSavePath(picked);
    } catch (e) {
      flash(`Save-As failed: ${e}`);
    }
  };

  const recheck = async () => {
    try {
      await invoke("check_prerequisites");
      setPrereqError(null);
      flash("yt-dlp and ffmpeg found — video downloads are ready.");
    } catch (e) {
      setPrereqError(String(e));
    }
  };

  return (
    <>
      <section className="composer">
        <input
          className={`url-input${urlError ? " invalid" : ""}`}
          placeholder="Paste a download or YouTube URL…"
          value={url}
          onChange={(e) => {
            setUrl(e.target.value);
            setUrlError(null);
          }}
          onKeyDown={(e) => e.key === "Enter" && submit()}
          aria-invalid={urlError !== null}
          aria-describedby={urlError ? "url-error" : undefined}
          spellCheck={false}
        />
        <button className="ghost" onClick={chooseSavePath} title="Choose destination">
          Save As…
        </button>
        <button className="primary" onClick={submit} disabled={busy}>
          {busy ? "Starting…" : scheduleAt ? "Schedule" : "Download"}
        </button>
      </section>

      {urlError && (
        <div className="field-error" id="url-error" role="alert">
          {urlError}
        </div>
      )}

      {prereqError && <PrerequisiteNotice message={prereqError} onRecheck={recheck} onDismiss={() => setPrereqError(null)} />}

      {savePath && (
        <div className="savepath-hint">
          Next download → <code>{savePath}</code>{" "}
          <button className="linklike" onClick={() => setSavePath("")}>
            clear
          </button>
        </div>
      )}

      <section className="controls">
        <label className="dir-wrap schedule-wrap">
          <span>Start at (optional) — leave empty to start now</span>
          <input
            type="datetime-local"
            className="dir-input"
            value={scheduleAt}
            onChange={(e) => setScheduleAt(e.target.value)}
          />
        </label>
        {scheduleAt && (
          <button className="linklike" onClick={() => setScheduleAt("")}>
            clear schedule
          </button>
        )}
      </section>
    </>
  );
}

/** "Video downloads need yt-dlp / ffmpeg" with the backend's install hint. */
function PrerequisiteNotice({
  message,
  onRecheck,
  onDismiss,
}: {
  message: string;
  onRecheck: () => void;
  onDismiss: () => void;
}) {
  // Backend format: "Missing required tool(s) for video downloads: X. <hint>"
  const split = message.indexOf(". ");
  const what = split > 0 ? message.slice(0, split + 1) : message;
  const hint = split > 0 ? message.slice(split + 2) : "";
  return (
    <div className="notice error" role="alert">
      <div>
        <strong>Video downloads need extra tools.</strong> {what}
        {hint && <div className="notice-hint">{hint}</div>}
        <div className="notice-hint">Plain file links work without them.</div>
      </div>
      <div className="notice-actions">
        <button className="ghost" onClick={onRecheck}>
          Check again
        </button>
        <button className="linklike" onClick={onDismiss}>
          Dismiss
        </button>
      </div>
    </div>
  );
}
