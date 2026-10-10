// Toolbar popup: detected media for the current tab, app status, paste box.

import type { SendResult } from "./lib/bridge";
import type { MediaItem } from "./lib/items";
import { MSG } from "./lib/messages";
import type { AllReply, ConnReply, MediaList } from "./lib/protocol";
import { $, ask, fmtSize, paintConn } from "./ui";

const list = $<HTMLDivElement>("list");
const msg = $<HTMLDivElement>("msg");
const allBtn = $<HTMLButtonElement>("all");

function say(text: string, ok: boolean): void {
  msg.textContent = text;
  msg.className = `msg ${ok ? "ok" : "err"}`;
}

function sentText(r: SendResult): string {
  return r.ok ? (r.queued ? MSG.sentQueued : MSG.sentPending) : r.error;
}

function render(items: MediaItem[], tabId: number): void {
  list.replaceChildren();
  allBtn.hidden = items.length < 2;
  if (items.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "No media detected on this page yet.";
    list.append(empty);
    return;
  }
  for (const item of items) {
    const row = document.createElement("div");
    row.className = "row";
    const info = document.createElement("div");
    info.className = "info";
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = item.fileName;
    name.title = item.url;
    const kind = item.kind === "page" ? "Page stream" : item.kind.toUpperCase();
    const meta = document.createElement("div");
    meta.className = "meta";
    meta.textContent = [kind, item.quality ?? "", fmtSize(item.size), item.mediaType ?? "", item.drm ? "protected (DRM)" : ""]
      .filter(Boolean)
      .join(" · ");
    info.append(name, meta);
    const btn = document.createElement("button");
    btn.textContent = "Download";
    btn.addEventListener("click", async () => {
      btn.disabled = true;
      const r = await ask<SendResult>({ type: "download-item", key: item.key, tabId });
      say(r ? sentText(r) : "No answer from the extension.", r?.ok === true);
      btn.disabled = false;
    });
    row.append(info, btn);
    list.append(row);
  }
}

async function main(): Promise<void> {
  $("options").addEventListener("click", (e) => {
    e.preventDefault();
    void chrome.runtime.openOptionsPage();
  });
  // Ping the app each time the popup opens (no background timers).
  void ask<ConnReply>({ type: "check-connection" }).then((r) => paintConn($("dot"), $("conn"), r));

  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const tabId = tab?.id;
  if (tabId !== undefined) {
    const media = await ask<MediaList>({ type: "get-media", tabId });
    render(media?.items ?? [], tabId);
    allBtn.addEventListener("click", async () => {
      allBtn.disabled = true;
      const r = await ask<AllReply>({ type: "download-all", tabId });
      const results = r?.results ?? [];
      const ok = results.filter((x) => x.ok).length;
      const firstErr = results.find((x): x is Extract<SendResult, { ok: false }> => !x.ok);
      say(`${ok} of ${results.length} sent.${firstErr ? ` ${firstErr.error}` : ""}`, firstErr === undefined);
      allBtn.disabled = false;
    });
  }

  const urlInput = $<HTMLInputElement>("url");
  const sendBtn = $<HTMLButtonElement>("send");
  const sendUrl = async () => {
    const url = urlInput.value.trim();
    if (!url) return;
    sendBtn.disabled = true;
    const r = await ask<SendResult>({ type: "download-url", url, pageUrl: tab?.url ?? null });
    say(r ? sentText(r) : "No answer from the extension.", r?.ok === true);
    if (r?.ok) urlInput.value = "";
    sendBtn.disabled = false;
  };
  sendBtn.addEventListener("click", () => void sendUrl());
  urlInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") void sendUrl();
  });
}

void main();
