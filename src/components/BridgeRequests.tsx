import { invoke } from "@tauri-apps/api/tauri";
import { fmtBytes } from "../lib/format";
import type { PendingRequest } from "../types";

interface Props {
  pending: PendingRequest[];
  flash: (msg: string) => void;
}

/** Accept / Dismiss cards for browser downloads waiting for confirmation. */
export function BridgeRequests({ pending, flash }: Props) {
  if (pending.length === 0) return null;

  // The card disappears on `bridge-request-removed`; an accepted one then
  // shows up as a normal download via `queue-status`.
  const act = (cmd: "accept_bridge_request" | "dismiss_bridge_request", id: string) =>
    invoke(cmd, { id }).catch((e) => flash(String(e)));

  return (
    <section className="bridge-requests">
      {pending.map((r) => (
        <div className="card bridge-request" key={r.id}>
          <div className="card-head">
            <div className="title">
              <span className="badge">{r.kind === "stream" ? "Video" : "Browser"}</span>
              <span className="fname" title={r.fileName}>
                {r.fileName}
              </span>
            </div>
            <div className="actions">
              <button onClick={() => act("accept_bridge_request", r.id)}>Accept</button>
              <button className="ghost" onClick={() => act("dismiss_bridge_request", r.id)}>
                Dismiss
              </button>
            </div>
          </div>
          <div className="meta">
            <span className="status-text" title={r.url}>
              {r.url}
            </span>
          </div>
          <div className="meta">
            <span>Waiting for confirmation</span>
            {r.source !== "user-action" && <span>{SOURCE_LABEL[r.source]}</span>}
            {r.quality && r.quality !== "best" && <span>{r.quality}</span>}
            {r.size !== null && r.size > 0 && <span>{fmtBytes(r.size)}</span>}
            {r.mediaType && <span>{r.mediaType}</span>}
            {r.hasCookies && <span title="Sent only for this download, never saved">with cookies</span>}
            {r.pageUrl && <span title={r.pageUrl}>from {hostOf(r.pageUrl)}</span>}
          </div>
        </div>
      ))}
    </section>
  );
}

const SOURCE_LABEL: Record<PendingRequest["source"], string> = {
  takeover: "taken over from Chrome",
  "user-action": "",
  "page-scan": "found on the page",
  "command-line": "opened with IDM Replica",
};

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}
