import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import { applyProgress, type ProgressMap } from "../lib/components";
import type {
  AppSettings,
  BridgeRequestRemoved,
  ComponentProgress,
  ComponentsInfo,
  PairingPrompt,
} from "../types";
import { useTauriEvent } from "./useTauriEvent";

/** External components: status once on mount, then `components-status` / `component-progress`. */
export function useComponents(): { info: ComponentsInfo | null; progress: ProgressMap } {
  const [info, setInfo] = useState<ComponentsInfo | null>(null);
  const [progress, setProgress] = useState<ProgressMap>({});

  useEffect(() => {
    invoke<ComponentsInfo>("get_components_status")
      .then(setInfo)
      .catch(() => {});
  }, []);
  useTauriEvent<ComponentsInfo>("components-status", setInfo);
  useTauriEvent<ComponentProgress>("component-progress", (p) =>
    setProgress((m) => applyProgress(m, p))
  );

  return { info, progress };
}

/** Start with Windows / start minimized, and the extension step's flag. */
export function useAppSettings(
  flash: (msg: string) => void
): [
  AppSettings | null,
  (next: Pick<AppSettings, "startWithWindows" | "startMinimized">) => void,
  (done: boolean) => void,
] {
  const [settings, setSettings] = useState<AppSettings | null>(null);

  useEffect(() => {
    invoke<AppSettings>("get_app_settings")
      .then(setSettings)
      .catch(() => {});
  }, []);

  const save = useCallback(
    (next: Pick<AppSettings, "startWithWindows" | "startMinimized">) => {
      invoke<AppSettings>("set_app_settings", { settings: next }).then(setSettings, (e) =>
        flash(String(e))
      );
    },
    [flash]
  );

  const setExtensionDone = useCallback(
    (done: boolean) => {
      invoke<AppSettings>("set_extension_setup_done", { done }).then(setSettings, (e) =>
        flash(String(e))
      );
    },
    [flash]
  );

  return [settings, save, setExtensionDone];
}

/** The "Allow this extension?" prompt, mirrored from the backend. */
export function usePairing(): PairingPrompt | null {
  const [prompt, setPrompt] = useState<PairingPrompt | null>(null);

  useEffect(() => {
    invoke<PairingPrompt | null>("get_pending_pairing")
      .then(setPrompt)
      .catch(() => {});
  }, []);
  useTauriEvent<PairingPrompt>("pairing-request", setPrompt);
  useTauriEvent<BridgeRequestRemoved>("pairing-closed", ({ id }) =>
    setPrompt((p) => (p && p.id === id ? null : p))
  );

  return prompt;
}
