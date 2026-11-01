import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import { useBridge } from "../hooks/useBridge";
import { hostOf, popupKeyAction, requestType, SOURCE_LABEL } from "../lib/bridge";
import { fmtBytes } from "../lib/format";
import type { PendingRequest } from "../types";

type Command = "accept_bridge_request" | "dismiss_bridge_request";

/**
 * The "Download request" popup (its own always-on-top window, `#confirm`):
 * one browser request at a time, oldest first, with Start / Reject. It closes
 * itself when nothing is left to answer. State comes from the same events as
 * the main window's cards, so answering in either place updates both.
 */
export function ConfirmWindow() {
  // `loaded`: the backlog arrived, so an empty list really means "nothing left".
  const { pending, loaded } = useBridge();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const current: PendingRequest | undefined = pending[0];

  useEffect(() => {
    if (loaded && !current && !error) {
      invoke("close_confirm_window").catch(() => {});
    }
  }, [loaded, current, error]);

  const act = (cmd: Command, ids: string[]) => {
    setBusy(true);
    Promise.all(ids.map((id) => invoke(cmd, { id })))
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!current || busy || error) return;
      const a = popupKeyAction(e.key);
      if (a === "start") act("accept_bridge_request", [current.id]);
      if (a === "reject") act("dismiss_bridge_request", [current.id]);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (error) {
    return (
      <div className="confirm-window">
        <h1>The download could not start</h1>
        <p className="error-line">{error}</p>
        <div className="confirm-actions">
          <button className="primary" onClick={() => setError(null)}>
            OK
          </button>
        </div>
      </div>
    );
  }

  if (!current) {
    return <div className="confirm-window dim">No downloads waiting.</div>;
  }

  const more = pending.length - 1;
  const from = current.pageUrl ? hostOf(current.pageUrl) : hostOf(current.url);
  return (
    <div className="confirm-window">
      <h1>Download request{more > 0 ? ` (1 of ${pending.length})` : ""}</h1>
      <div className="confirm-file" title={current.fileName}>
        {current.fileName}
      </div>
      <dl className="confirm-info">
        <dt>Address</dt>
        <dd title={current.url}>{current.url}</dd>
        <dt>From</dt>
        <dd>
          {from}
          {SOURCE_LABEL[current.source] && ` · ${SOURCE_LABEL[current.source]}`}
        </dd>
        <dt>Type</dt>
        <dd>{requestType(current)}</dd>
        <dt>Size</dt>
        <dd>{current.size !== null && current.size > 0 ? fmtBytes(current.size) : "unknown"}</dd>
        {current.hasCookies && (
          <>
            <dt>Sign-in</dt>
            <dd title="Sent only for this download, never saved">uses your browser cookies</dd>
          </>
        )}
      </dl>
      <div className="confirm-actions">
        <button
          className="primary"
          autoFocus
          disabled={busy}
          onClick={() => act("accept_bridge_request", [current.id])}
        >
          Start download
        </button>
        <button className="ghost" disabled={busy} onClick={() => act("dismiss_bridge_request", [current.id])}>
          Reject
        </button>
        {more > 0 && (
          <>
            <span className="spacer" />
            <button
              disabled={busy}
              onClick={() => act("accept_bridge_request", pending.map((r) => r.id))}
            >
              Start all {pending.length}
            </button>
            <button
              className="ghost"
              disabled={busy}
              onClick={() => act("dismiss_bridge_request", pending.map((r) => r.id))}
            >
              Reject all
            </button>
          </>
        )}
      </div>
      <p className="dim small">Enter = start · Esc = reject. Closing this window keeps the requests in IDM Replica.</p>
    </div>
  );
}
