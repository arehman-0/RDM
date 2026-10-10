// Messages between content script / popup / options and the background worker.

import type { ConnState, SendResult } from "./bridge";
import type { ItemKind, MediaItem } from "./items";
import type { Settings } from "./settings";

/** A media element the content script found in the page. */
export interface DomMedia {
  /** http(s) src, or null for blob:/MSE players. */
  src: string | null;
  kind: "video" | "audio";
  pageUrl: string;
  /** document.title, used as the name of a page-level item. */
  title: string;
  drm: boolean;
  width: number;
  height: number;
}

export type Request =
  | { type: "dom-media"; items: DomMedia[] }
  | { type: "get-media"; tabId?: number }
  | { type: "download-item"; key: string; tabId?: number }
  | { type: "download-best"; pageUrl: string; src?: string | null; tabId?: number }
  | { type: "download-all"; tabId: number }
  | { type: "download-url"; url: string; pageUrl?: string | null }
  | { type: "check-connection" }
  | { type: "get-settings" }
  | { type: "disable-site"; host: string }
  | { type: "toast"; text: string };

export interface MediaList {
  items: MediaItem[];
  /** Hostname of the tab, for the per-site switch. */
  host: string;
}

export type SendReply = SendResult;
export type AllReply = { results: SendResult[] };
export type ConnReply = { conn: ConnState; text: string; extensionId: string; port: number };
export type SettingsReply = { settings: Settings };

export function isItemKind(v: string): v is ItemKind {
  return ["video", "audio", "hls", "dash", "page"].includes(v);
}
