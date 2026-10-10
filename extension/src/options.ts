// Options page: pairing (token, port), floating button, interception.

import { MSG } from "./lib/messages";
import type { ConnReply, PairReply } from "./lib/protocol";
import { DEFAULT_PORT, normalizeSettings, parseExtList, type Settings } from "./lib/settings";
import { $, ask, paintConn } from "./ui";

const token = $<HTMLInputElement>("token");
const port = $<HTMLInputElement>("port");
const floating = $<HTMLInputElement>("floating");
const disabled = $<HTMLTextAreaElement>("disabled");
const intercept = $<HTMLInputElement>("intercept");
const minmb = $<HTMLInputElement>("minmb");
const allow = $<HTMLInputElement>("allow");
const deny = $<HTMLInputElement>("deny");
const neverSites = $<HTMLTextAreaElement>("neversites");
const minLabel = $<HTMLElement>("minmb-label");

function showMin(): void {
  const mb = Number(minmb.value);
  minLabel.textContent = mb === 0 ? "none (every file)" : `${mb} MB`;
}
minmb.addEventListener("input", showMin);
const msg = $<HTMLSpanElement>("msg");

function say(text: string, ok: boolean): void {
  msg.textContent = text;
  msg.className = `msg ${ok ? "ok" : "err"}`;
}

async function load(): Promise<void> {
  const s = normalizeSettings((await chrome.storage.local.get("settings")).settings);
  token.value = s.token;
  port.value = String(s.port);
  floating.checked = s.floatingButton;
  disabled.value = s.disabledSites.join("\n");
  intercept.checked = s.intercept;
  minmb.value = String(s.interceptMinMB);
  allow.value = s.allowExts.join(", ");
  deny.value = s.denyExts.join(", ");
  neverSites.value = s.neverSites.join("\n");
  showMin();
}

function read(): Settings | string {
  const t = token.value.trim();
  if (t && !/^[0-9a-f]{64}$/i.test(t)) return "The token is 64 letters/digits. Copy it again from IDM Replica.";
  const p = Number(port.value || DEFAULT_PORT);
  if (!Number.isInteger(p) || p < 1 || p > 65535) return "The port must be a number from 1 to 65535.";
  const mb = Number(minmb.value || 0);
  if (!Number.isFinite(mb) || mb < 0) return "The minimum size must be 0 or more.";
  return normalizeSettings({
    token: t,
    port: p,
    floatingButton: floating.checked,
    disabledSites: disabled.value.split(/\s+/),
    intercept: intercept.checked,
    interceptMinMB: mb,
    allowExts: parseExtList(allow.value),
    denyExts: parseExtList(deny.value),
    neverSites: neverSites.value.split(/\s+/),
  });
}

async function save(): Promise<boolean> {
  const s = read();
  if (typeof s === "string") {
    say(s, false);
    return false;
  }
  await chrome.storage.local.set({ settings: s });
  say("Saved.", true);
  return true;
}

async function test(): Promise<void> {
  if (!(await save())) return;
  $("conn").textContent = "Testing…";
  paintConn($("dot"), $("conn"), await ask<ConnReply>({ type: "check-connection" }));
}

$("extid").textContent = chrome.runtime.id;
$("copyid").addEventListener("click", () => {
  void navigator.clipboard.writeText(chrome.runtime.id).then(() => say("Extension ID copied.", true));
});
$("showtoken").addEventListener("click", () => {
  const hidden = token.type === "password";
  token.type = hidden ? "text" : "password";
  $("showtoken").textContent = hidden ? "Hide" : "Show";
});
$("connect").addEventListener("click", async () => {
  // Save first (the port may have been edited), pair, then reload the token field.
  if (!(await save())) return;
  const btn = $<HTMLButtonElement>("connect");
  btn.disabled = true;
  $("conn").textContent = MSG.pairWaiting;
  const r = await ask<PairReply>({ type: "pair" });
  btn.disabled = false;
  if (r?.ok) {
    await load();
    say(MSG.paired, true);
  } else {
    say(r?.error ?? "No answer from the extension.", false);
  }
  paintConn($("dot"), $("conn"), await ask<ConnReply>({ type: "check-connection" }));
});
$("save").addEventListener("click", () => void save());
$("test").addEventListener("click", () => void test());
void load();
