import { useEffect, useRef } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Subscribe to a backend event for the component's lifetime.
 *
 * `listen()` resolves asynchronously; if the component unmounts first (React
 * StrictMode mounts twice in dev) the late subscription is dropped at once
 * instead of leaking — otherwise every event would be handled twice.
 */
export function useTauriEvent<T>(name: string, handler: (payload: T) => void): void {
  // Always call the latest handler without re-subscribing on every render.
  const ref = useRef(handler);
  ref.current = handler;

  useEffect(() => {
    let disposed = false;
    let off: UnlistenFn | undefined;
    listen<T>(name, (e) => ref.current(e.payload))
      .then((un) => {
        if (disposed) un();
        else off = un;
      })
      .catch(() => {
        // Not running inside Tauri (e.g. plain browser preview): no events.
      });
    return () => {
      disposed = true;
      off?.();
    };
  }, [name]);
}
