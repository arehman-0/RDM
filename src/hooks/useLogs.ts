import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import type { LogLine } from "../types";
import { useTauriEvent } from "./useTauriEvent";

const KEEP = 300;

/** Backend log lines for the log panel: the backlog, then live `log-line`s. */
export function useLogs(): LogLine[] {
  const [lines, setLines] = useState<LogLine[]>([]);

  useEffect(() => {
    invoke<LogLine[]>("recent_logs")
      .then((backlog) => {
        // Keep live lines that arrived before the backlog did (minus overlap).
        const key = (l: LogLine) => `${l.ts}|${l.level}|${l.message}`;
        const seen = new Set(backlog.map(key));
        setLines((live) => [...backlog, ...live.filter((l) => !seen.has(key(l)))].slice(-KEEP));
      })
      .catch(() => {});
  }, []);

  useTauriEvent<LogLine>("log-line", (line) => setLines((prev) => [...prev, line].slice(-KEEP)));

  return lines;
}
