import type { Settings } from "../hooks/useSettings";

interface Props {
  settings: Settings;
  /** Speed slider moved (applied live to running downloads by the parent). */
  onSpeedLimit: (mb: number) => void;
  /** Max-simultaneous slider moved. */
  onMaxConcurrent: (n: number) => void;
}

/** Connections / speed / concurrency / folder / integrity / advanced options. */
export function SettingsPanel({ settings: s, onSpeedLimit, onMaxConcurrent }: Props) {
  return (
    <>
      <section className="controls">
        <label className="slider-wrap">
          <span>
            Connections: <b>{s.connections}</b>
          </span>
          <input
            type="range"
            min={1}
            max={16}
            value={s.connections}
            onChange={(e) => s.setConnections(Number(e.target.value))}
          />
        </label>
        <label className="slider-wrap">
          <span>
            Speed limit: <b>{s.speedLimitMB === 0 ? "unlimited" : `${s.speedLimitMB} MB/s`}</b>
          </span>
          <input
            type="range"
            min={0}
            max={100}
            value={s.speedLimitMB}
            onChange={(e) => onSpeedLimit(Number(e.target.value))}
          />
        </label>
        <label className="slider-wrap">
          <span>
            Max simultaneous: <b>{s.maxConcurrent}</b>
          </span>
          <input
            type="range"
            min={1}
            max={10}
            value={s.maxConcurrent}
            onChange={(e) => onMaxConcurrent(Number(e.target.value))}
          />
        </label>
        <label className="dir-wrap">
          <span>Save to</span>
          <input
            className="dir-input"
            value={s.saveDir}
            onChange={(e) => s.setSaveDir(e.target.value)}
            spellCheck={false}
          />
        </label>
      </section>

      <section className="controls">
        <label className="check-wrap">
          <input type="checkbox" checked={s.verify} onChange={(e) => s.setVerify(e.target.checked)} />
          <span>Verify integrity (SHA-256 / MD5) after download</span>
        </label>
        {s.verify && (
          <label className="dir-wrap">
            <span>Expected checksum (optional)</span>
            <input
              className="dir-input"
              placeholder="paste a SHA-256 or MD5 hash to compare"
              value={s.expectedChecksum}
              onChange={(e) => s.setExpectedChecksum(e.target.value)}
              spellCheck={false}
            />
          </label>
        )}
      </section>

      <section className="controls">
        <button className="linklike" onClick={() => s.setShowAdvanced(!s.showAdvanced)}>
          {s.showAdvanced ? "▾ Advanced" : "▸ Advanced (headers, mirrors)"}
        </button>
      </section>

      {s.showAdvanced && (
        <section className="controls advanced">
          <label
            className="dir-wrap"
            title="Sent on every request. For video sites they are also passed to yt-dlp and to both stream downloads."
          >
            <span>
              Custom headers — one "Key: Value" per line (auth, cookies; also used for video sites)
            </span>
            <textarea
              className="dir-input area"
              rows={3}
              placeholder={"Authorization: Bearer …\nCookie: session=…"}
              value={s.headersText}
              onChange={(e) => s.setHeadersText(e.target.value)}
              spellCheck={false}
            />
          </label>
          <label className="dir-wrap">
            <span>Mirror URLs — same file, one per line (combined bandwidth; plain files only)</span>
            <textarea
              className="dir-input area"
              rows={3}
              placeholder={"https://mirror1.example/file.iso\nhttps://mirror2.example/file.iso"}
              value={s.mirrorsText}
              onChange={(e) => s.setMirrorsText(e.target.value)}
              spellCheck={false}
            />
          </label>
        </section>
      )}
    </>
  );
}
