// Toolbar popup: detected media for the current tab, app status, paste box.

import type { SendResult } from "./lib/bridge";
import type { MediaItem } from "./lib/items";
import { MSG } from "./lib/messages";
import { connAction } from "./lib/bridge";
import type { AllReply, ConnReply, MediaList, PairReply } from "./lib/protocol";
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

/** Ping the app and offer the one useful next step (start it / connect / options). */
async function refreshConnection(): Promise<void> {
  const r = await ask<ConnReply>({ type: "check-connection" });
  paintConn($("dot"), $("conn"), r);
  const cta = $("cta");
  const btn = $<HTMLButtonElement>("cta-btn");
  const action = r ? connAction(r.conn) : "none";
  cta.hidden = action === "none";
  btn.disabled = false;
  if (action === "start-app") {
    $("cta-title").textContent = "Start IDM Replica";
    $("cta-text").textContent = "The app isn't running, so downloads stay in the browser. Open IDM Replica, then try again.";
    btn.textContent = "Try again";
    btn.onclick = () => void refreshConnection();
  } else if (action === "connect") {
    $("cta-title").textContent = "Connect to IDM Replica";
    $("cta-text").textContent = "One click here, then Allow in the app.";
    btn.textContent = "Connect to IDM Replica";
    btn.onclick = async () => {
      btn.disabled = true;
      $("cta-text").textContent = MSG.pairWaiting;
      const p = await ask<PairReply>({ type: "pair" });
      say(p?.ok ? MSG.paired : p?.error ?? "No answer from the extension.", p?.ok === true);
      await refreshConnection();
    };
  } else if (action === "options") {
    $("cta-title").textContent = "Check the settings";
    $("cta-text").textContent = "";
    btn.textContent = "Open Options";
    btn.onclick = () => void chrome.runtime.openOptionsPage();
  }
}

async function main(): Promise<void> {
  $("options").addEventListener("click", (e) => {
    e.preventDefault();
    void chrome.runtime.openOptionsPage();
  });
  // Ping the app each time the popup opens (no background timers).
  void refreshConnection();

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
