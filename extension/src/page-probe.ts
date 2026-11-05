// MAIN-world probe (runs in the page's own JavaScript world, every frame,
// from document_start). It wraps fetch and XMLHttpRequest only to *look at*
// small text responses: when one is an HLS/DASH playlist, its URL is posted
// to the extension's content script in the same frame. Nothing is changed,
// blocked or sent anywhere else; responses still reach the page untouched.
// See extension/store/permissions.md ("Player probe").

import { manifestFromText, PROBE_BYTES, PROBE_MESSAGE, worthProbing, type ProbeReport } from "./lib/probe";

(() => {
  const w = window as unknown as { __idmReplicaProbe?: boolean };
  if (w.__idmReplicaProbe) return;
  w.__idmReplicaProbe = true;

  const seen = new Set<string>();
  const report = (url: string, text: string) => {
    if (!/^https?:/i.test(url) || seen.has(url)) return;
    const hit = manifestFromText(text);
    if (!hit) return;
    seen.add(url);
    const msg: ProbeReport = { type: PROBE_MESSAGE, url, kind: hit.kind, master: hit.master };
    window.postMessage(msg, window.location.origin === "null" ? "*" : window.location.origin);
  };
  const lengthOf = (v: string | null) => (v && /^\d+$/.test(v) ? Number(v) : null);

  // --- XMLHttpRequest (hls.js, video.js / VHS, most players) ---------------
  const xhrProto = XMLHttpRequest.prototype;
  const origSend = xhrProto.send;
  xhrProto.send = function (this: XMLHttpRequest, ...args: Parameters<XMLHttpRequest["send"]>) {
    this.addEventListener("load", () => {
      try {
        if (this.status < 200 || this.status >= 300) return;
        const type = this.getResponseHeader("content-type") ?? "";
        if (!worthProbing(type, lengthOf(this.getResponseHeader("content-length")))) return;
        if (this.responseType === "" || this.responseType === "text") {
          report(this.responseURL, this.responseText.slice(0, PROBE_BYTES));
        } else if (this.responseType === "arraybuffer" && this.response instanceof ArrayBuffer) {
          const bytes = new Uint8Array(this.response, 0, Math.min(PROBE_BYTES, this.response.byteLength));
          report(this.responseURL, new TextDecoder().decode(bytes));
        }
      } catch {
        // Never disturb the page.
      }
    });
    return origSend.apply(this, args);
  };

  // --- fetch (Shaka, newer players) -----------------------------------------
  const origFetch = window.fetch;
  window.fetch = async function (...args: Parameters<typeof fetch>): Promise<Response> {
    const res = await origFetch.apply(this, args);
    try {
      if (res.ok && worthProbing(res.headers.get("content-type") ?? "", lengthOf(res.headers.get("content-length")))) {
        // Read only the first chunk of a copy; the page gets the original.
        const reader = res.clone().body?.getReader();
        if (reader) {
          void reader
            .read()
            .then(({ value }) => {
              void reader.cancel().catch(() => {});
              if (value) report(res.url, new TextDecoder().decode(value.subarray(0, PROBE_BYTES)));
            })
            .catch(() => {});
        }
      }
    } catch {
      // Never disturb the page.
    }
    return res;
  };
})();
