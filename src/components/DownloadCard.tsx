import type { DownloadCard as Card, Priority } from "../types";
import { fmtBytes, fmtEta, fmtSpeed } from "../lib/format";
import { cardActions, missingHeaders, statusText, type CardAction } from "../lib/status";
import { SegmentBars } from "./SegmentBars";

/** Everything a card can ask the backend to do. */
export type CardCommand =
  | { kind: Exclude<CardAction, "reorder"> }
  | { kind: "move"; delta: number }
  | { kind: "priority"; priority: Priority };

const LABELS: Record<Exclude<CardAction, "reorder">, string> = {
  pause: "Pause",
  resume: "Resume",
  retry: "Retry",
  cancel: "Cancel",
  open: "Open",
  reveal: "Show in folder",
  remove: "Remove",
};

export function DownloadCard({
  card,
  onCommand,
}: {
  card: Card;
  onCommand: (card: Card, cmd: CardCommand) => void;
}) {
  const isActive = card.status === "downloading";
  const blocked = missingHeaders(card);
  const warnings = Object.values(card.jobs).flatMap((j) => j.warnings ?? []);

  const button = (action: Exclude<CardAction, "reorder">) => {
    const retryLabel = card.status === "interrupted" ? "Resume" : "Retry";
    const title =
      action === "retry"
        ? blocked
          ? "Custom header values are not saved to disk — add the download again with its headers"
          : card.status === "interrupted"
          ? "Continue from where it stopped"
          : "Start again"
        : action === "pause" && card.status !== "downloading"
        ? "Hold in the queue (keeps its place)"
        : action === "open" || action === "reveal"
        ? card.fileName
        : undefined;
    return (
      <button
        key={action}
        className={action === "cancel" ? "danger" : action === "remove" ? "ghost" : undefined}
        disabled={action === "retry" && blocked}
        title={title}
        onClick={() => onCommand(card, { kind: action })}
      >
        {action === "retry" ? retryLabel : LABELS[action]}
      </button>
    );
  };

  return (
    <div className={`card status-${card.status}`}>
      <div className="card-head">
        <div className="title">
          <span className={`badge ${card.isYoutube ? "yt" : "file"}`}>
            {card.isYoutube ? "YT" : "FILE"}
          </span>
          <span className="fname" title={card.fileName}>
            {card.fileName}
          </span>
        </div>
        <div className="actions">
          {cardActions(card).map((a) =>
            a === "reorder" ? (
              <QueueControls
                key="reorder"
                priority={card.priority ?? "normal"}
                onMove={(delta) => onCommand(card, { kind: "move", delta })}
                onPriority={(priority) => onCommand(card, { kind: "priority", priority })}
              />
            ) : (
              button(a)
            )
          )}
        </div>
      </div>

      {/* Overall progress */}
      <div className="bar-outer">
        <div className="bar-inner" style={{ width: `${card.progress}%` }} />
        <span className="bar-text">{card.progress.toFixed(1)}%</span>
      </div>

      <div className="meta">
        <span>
          {fmtBytes(card.downloaded)} / {card.total > 0 ? fmtBytes(card.total) : "?"}
        </span>
        <span className="status-text">{statusText(card)}</span>
        <span>{isActive ? fmtSpeed(card.speed) : ""}</span>
        <span>{isActive ? `ETA ${fmtEta(card.eta)}` : ""}</span>
        <span>{card.connections} conn</span>
        {card.speedLimit > 0 && <span>≤ {fmtSpeed(card.speedLimit)}</span>}
      </div>

      {(card.sha256 || card.verified != null) && (
        <div
          className={`integrity ${
            card.verified === true ? "ok" : card.verified === false ? "bad" : ""
          }`}
        >
          {card.verified === true && "✓ checksum verified · "}
          {card.verified === false && "✗ checksum MISMATCH · "}
          {card.sha256 && <code title={card.sha256}>SHA-256 {card.sha256.slice(0, 16)}…</code>}
        </div>
      )}

      {warnings.map((w, i) => (
        <div className="integrity warn" key={`warn-${i}`} title={w}>
          ⚠ {w}
        </div>
      ))}

      <SegmentBars jobs={card.jobs} isYoutube={card.isYoutube} />

      {blocked && !card.error && (
        <div className="error-line">
          ⚠ Used custom headers ({card.headerNames?.join(", ")}), which are not saved to disk.
          Add it again with the headers to continue.
        </div>
      )}
      {card.error && <div className="error-line">⚠ {card.error}</div>}
    </div>
  );
}

/** ▲ / ▼ within the queue and a priority picker (waiting entries only). */
function QueueControls({
  priority,
  onMove,
  onPriority,
}: {
  priority: Priority;
  onMove: (delta: number) => void;
  onPriority: (p: Priority) => void;
}) {
  return (
    <span className="queue-controls">
      <button title="Move up in the queue" aria-label="Move up" onClick={() => onMove(-1)}>
        ▲
      </button>
      <button title="Move down in the queue" aria-label="Move down" onClick={() => onMove(1)}>
        ▼
      </button>
      <select
        className="priority"
        title="Queue priority (higher starts first)"
        aria-label="Queue priority"
        value={priority}
        onChange={(e) => {
          const v = e.target.value;
          if (v === "high" || v === "normal" || v === "low") onPriority(v);
        }}
      >
        <option value="high">High</option>
        <option value="normal">Normal</option>
        <option value="low">Low</option>
      </select>
    </span>
  );
}
