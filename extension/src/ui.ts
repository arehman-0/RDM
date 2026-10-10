// Small helpers shared by the popup and options pages.

import type { ConnReply, Request } from "./lib/protocol";

export function $<T extends HTMLElement>(id: string): T {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing #${id}`);
  return el as T;
}

export function ask<T>(msg: Request): Promise<T | null> {
  return new Promise((resolve) => {
    chrome.runtime.sendMessage(msg, (reply: T) => resolve(chrome.runtime.lastError ? null : reply));
  });
}

/** Paint a connection status line (dot colour + sentence). */
export function paintConn(dot: HTMLElement, text: HTMLElement, r: ConnReply | null): void {
  dot.className = "dot";
  if (!r) {
    text.textContent = "The extension's background worker didn't answer — reload the extension.";
    dot.classList.add("err");
    return;
  }
  text.textContent = r.text;
  const s = r.conn.state;
  dot.classList.add(s === "connected" ? "ok" : s === "not-running" ? "err" : "warn");
}

export function fmtSize(n: number | null): string {
  if (!n) return "";
  const units = ["B", "KB", "MB", "GB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
}
