import { useState } from "react";
import { useLogs } from "../hooks/useLogs";
import { fmtClock } from "../lib/format";

/** Collapsible backend log (INFO+ from the app, WARN+ from libraries). */
export function LogPanel() {
  const [open, setOpen] = useState(false);
  const lines = useLogs();
  const problems = lines.filter((l) => l.level === "WARN" || l.level === "ERROR").length;

  return (
    <section className="log-panel">
      <button className="linklike" onClick={() => setOpen(!open)}>
        {open ? "▾ Log" : "▸ Log"}
        {problems > 0 && <span className="log-badge">{problems} warning{problems === 1 ? "" : "s"}</span>}
      </button>
      {open && (
        <div className="log-lines" role="log" aria-live="polite">
          {lines.length === 0 && <div className="log-line dim">Nothing logged yet.</div>}
          {[...lines].reverse().map((l, i) => (
            <div className={`log-line lvl-${l.level.toLowerCase()}`} key={`${l.ts}-${i}`}>
              <span className="log-time">{fmtClock(l.ts)}</span>
              <span className="log-level">{l.level}</span>
              <span className="log-msg" title={l.target}>
                {l.message}
              </span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
