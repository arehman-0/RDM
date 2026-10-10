// HTTP client for the app's bridge (127.0.0.1 only). `fetch` is injected so
// the status/error mapping is unit-tested without a browser.

import { codeMessage, MSG } from "./messages";
import type { DownloadPayload } from "./request";

export const PORT_RANGE = 10; // the app tries DEFAULT_PORT … DEFAULT_PORT+10
export type FetchFn = (input: string, init?: RequestInit) => Promise<Response>;

export type ConnState =
  | { state: "connected"; port: number; version: string }
  | { state: "not-running" }
  | { state: "port-mismatch"; foundPort: number }
  | { state: "wrong-app" }
  | { state: "origin-blocked" }
  | { state: "no-token"; port: number; version: string }
  | { state: "token-invalid"; port: number; version: string };

export function base(port: number): string {
  return `http://127.0.0.1:${port}`;
}

async function json(resp: Response): Promise<Record<string, unknown>> {
  try {
    const v: unknown = await resp.json();
    return typeof v === "object" && v !== null ? (v as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

type Ping = { kind: "app"; version: string } | { kind: "origin-blocked" } | { kind: "other" } | { kind: "down" };

async function ping(port: number, fetchFn: FetchFn, timeoutMs: number): Promise<Ping> {
  try {
    const resp = await fetchFn(`${base(port)}/v1/ping`, { signal: AbortSignal.timeout(timeoutMs) });
    const body = await json(resp);
    if (resp.status === 403 && body.error === "origin not allowed") return { kind: "origin-blocked" };
    if (resp.ok && body.app === "idm-replica") return { kind: "app", version: String(body.version ?? "") };
    return { kind: "other" };
  } catch {
    return { kind: "down" };
  }
}

/**
 * Ping the configured port; if nothing is there, look for the app on the
 * fallback ports. With a token, verify it with a request the bridge rejects
 * *after* the token check (an empty batch → 400 = token OK, 401 = wrong).
 */
export async function checkConnection(
  port: number,
  token: string,
  fetchFn: FetchFn,
  defaultPort: number
): Promise<ConnState> {
  const p = await ping(port, fetchFn, 2000);
  if (p.kind === "origin-blocked") return { state: "origin-blocked" };
  if (p.kind === "other") return { state: "wrong-app" };
  if (p.kind === "down") {
    for (let other = defaultPort; other <= defaultPort + PORT_RANGE; other++) {
      if (other === port) continue;
      const q = await ping(other, fetchFn, 600);
      if (q.kind === "app" || q.kind === "origin-blocked") return { state: "port-mismatch", foundPort: other };
    }
    return { state: "not-running" };
  }
  if (!token) return { state: "no-token", port, version: p.version };
  try {
    // GET /v1/auth: 204 = token OK, 401 = wrong, no side effects.
    let resp = await fetchFn(`${base(port)}/v1/auth`, {
      headers: { "X-IDM-Token": token },
      signal: AbortSignal.timeout(2000),
    });
    if (resp.status === 404) {
      // Older app without /v1/auth: an empty batch fails after the token check.
      resp = await fetchFn(`${base(port)}/v1/batch`, {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-IDM-Token": token },
        body: JSON.stringify({ items: [] }),
        signal: AbortSignal.timeout(2000),
      });
    }
    if (resp.status === 401) return { state: "token-invalid", port, version: p.version };
    if (resp.status === 403) return { state: "origin-blocked" };
    return { state: "connected", port, version: p.version };
  } catch {
    return { state: "not-running" };
  }
}

/** One line for the popup / options status. */
export function describeConn(c: ConnState, extensionId: string, configuredPort: number): string {
  switch (c.state) {
    case "connected":
      return `Connected to IDM Replica ${c.version} on port ${c.port}`;
    case "not-running":
      return MSG.notRunning;
    case "port-mismatch":
      return MSG.portMismatch(c.foundPort, configuredPort);
    case "wrong-app":
      return MSG.wrongApp(configuredPort);
    case "origin-blocked":
      return MSG.originBlocked(extensionId);
    case "no-token":
      return MSG.noToken;
    case "token-invalid":
      return MSG.badToken;
  }
}

export type SendResult =
  /** `queued`: started (or waiting for a queue slot); false = waiting for the user in the app. */
  | { ok: true; id: string; fileName: string; queued: boolean; duplicate?: boolean }
  /** `code`: the bridge's code, or "app_unreachable" / "no_token" from here. */
  | { ok: false; error: string; code?: string };

/**
 * Map a bridge error to a sentence the user can act on: by its stable `code`
 * first (README "Error codes"), else by HTTP status / text (older apps).
 */
export function explainError(status: number, error: string, extensionId: string, code = ""): string {
  const byCode = codeMessage(code, extensionId);
  if (byCode) return byCode;
  if (status === 401) return MSG.badToken;
  if (status === 403 && error === "origin not allowed") return MSG.originBlocked(extensionId);
  if (status === 429) return MSG.tooMany;
  if (status === 400 && /only http and https/.test(error)) return MSG.blockedUrl;
  if (status === 400 && /supported video site|pageUrl alone/.test(error)) return MSG.siteUnsupported;
  return error ? `IDM Replica: ${error}` : `IDM Replica answered HTTP ${status}.`;
}

async function post(
  port: number,
  token: string,
  path: string,
  body: unknown,
  fetchFn: FetchFn
): Promise<{ status: number; body: Record<string, unknown> } | null> {
  try {
    const resp = await fetchFn(`${base(port)}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-IDM-Token": token },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(15000),
    });
    return { status: resp.status, body: await json(resp) };
  } catch {
    return null;
  }
}

function str(v: unknown): string {
  return typeof v === "string" ? v : "";
}

function toResult(r: Record<string, unknown>, status: number, extensionId: string): SendResult {
  if (r.ok === true && typeof r.id === "string") {
    return {
      ok: true,
      id: r.id,
      fileName: String(r.fileName ?? ""),
      queued: r.queued === true,
      duplicate: r.duplicate === true,
    };
  }
  const code = str(r.code);
  const text = str(r.message) || str(r.error);
  return { ok: false, error: explainError(status, text, extensionId, code), code: code || undefined };
}

export async function sendDownload(
  port: number,
  token: string,
  payload: DownloadPayload,
  fetchFn: FetchFn,
  extensionId: string
): Promise<SendResult> {
  if (!token) return { ok: false, error: MSG.noToken, code: "no_token" };
  const r = await post(port, token, "/v1/download", payload, fetchFn);
  if (!r) return { ok: false, error: MSG.notRunning, code: "app_unreachable" };
  return toResult(r.body, r.status, extensionId);
}

/** "Download all": one batch request; per-item results in order. */
export async function sendBatch(
  port: number,
  token: string,
  items: DownloadPayload[],
  fetchFn: FetchFn,
  extensionId: string
): Promise<SendResult[]> {
  if (!token) return items.map(() => ({ ok: false, error: MSG.noToken, code: "no_token" }));
  const r = await post(port, token, "/v1/batch", { items }, fetchFn);
  if (!r) return items.map(() => ({ ok: false, error: MSG.notRunning, code: "app_unreachable" }));
  const results = r.body.results;
  if (r.status !== 200 || !Array.isArray(results)) {
    const code = str(r.body.code);
    const err = explainError(r.status, str(r.body.message) || str(r.body.error), extensionId, code);
    return items.map(() => ({ ok: false, error: err, code: code || undefined }));
  }
  return results.map((x: unknown) =>
    toResult(typeof x === "object" && x !== null ? (x as Record<string, unknown>) : {}, 200, extensionId)
  );
}
