import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import { addPending, mergePending, removePending } from "../lib/bridge";
import type { BridgeInfo, BridgeRequestRemoved, PendingRequest } from "../types";
import { useTauriEvent } from "./useTauriEvent";

/**
 * Browser-bridge state as a passive mirror: fetched once on mount, then kept
 * current by `bridge-status` / `bridge-request` / `bridge-request-removed`.
 */
export function useBridge(): {
  info: BridgeInfo | null;
  pending: PendingRequest[];
  /** The backlog has arrived (until then an empty list means "not known yet"). */
  loaded: boolean;
} {
  const [info, setInfo] = useState<BridgeInfo | null>(null);
  const [pending, setPending] = useState<PendingRequest[]>([]);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    invoke<BridgeInfo>("get_bridge_info")
      .then(setInfo)
      .catch(() => {});
    invoke<PendingRequest[]>("list_bridge_requests")
      .then((backlog) => setPending((live) => mergePending(backlog, live)))
      .catch(() => {})
      .finally(() => setLoaded(true));
  }, []);

  useTauriEvent<BridgeInfo>("bridge-status", setInfo);
  useTauriEvent<PendingRequest>("bridge-request", (req) => setPending((l) => addPending(l, req)));
  useTauriEvent<BridgeRequestRemoved>("bridge-request-removed", ({ id }) =>
    setPending((l) => removePending(l, id))
  );

  return { info, pending, loaded };
}
