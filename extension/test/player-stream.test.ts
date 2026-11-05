// blob:/MediaSource players: finding the playlist behind them and sending it
// the way the page's player fetched it.
import { describe, expect, it } from "vitest";
import { streamFileName } from "../src/lib/filename";
import { bestItem, bestStream, mergeItem, type MediaItem } from "../src/lib/items";
import { manifestFromText, parseProbeReport, PROBE_MESSAGE, worthProbing } from "../src/lib/probe";
import { buildPayload, targetForItem } from "../src/lib/request";

const item = (over: Partial<MediaItem>): MediaItem => ({
  key: "k",
  url: "https://cdn.example.net/a.mp4",
  kind: "video",
  source: "network",
  fileName: "a.mp4",
  mediaType: null,
  size: null,
  quality: null,
  pageUrl: "https://player.example.org/embed/1",
  drm: false,
  seenAt: 1,
  ...over,
});

describe("playlists recognised by content", () => {
  it("HLS master vs media playlist, DASH, and everything else", () => {
    expect(manifestFromText("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nlow.m3u8")).toEqual({ kind: "hls", master: true });
    expect(manifestFromText("\uFEFF  #EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\nseg0.ts")).toEqual({ kind: "hls", master: false });
    expect(manifestFromText('<?xml version="1.0"?>\n<MPD xmlns="urn:mpeg:dash:schema:mpd:2011">')).toEqual({ kind: "dash", master: true });
    expect(manifestFromText("<!-- generated -->\n<MPD type=\"static\">")).toEqual({ kind: "dash", master: true });
    expect(manifestFromText('{"sources":[{"file":"x.m3u8"}]}')).toBeNull();
    expect(manifestFromText("<html><body>#EXTM3U</body></html>")).toBeNull();
    expect(manifestFromText("")).toBeNull();
  });

  it("only reads small, non-media responses", () => {
    expect(worthProbing("text/plain", 900)).toBe(true);
    expect(worthProbing("image/png", 4096)).toBe(true); // disguised playlists exist
    expect(worthProbing("", null)).toBe(true);
    expect(worthProbing("video/mp2t", 900)).toBe(false);
    expect(worthProbing("video/mp4", null)).toBe(false);
    expect(worthProbing("audio/aac", 10)).toBe(false);
    expect(worthProbing("text/plain", 50 * 1024 * 1024)).toBe(false);
  });

  it("validates what the page posts", () => {
    const ok = { type: PROBE_MESSAGE, url: "https://cdn.example.net/x/stream.txt", kind: "hls", master: true };
    expect(parseProbeReport(ok)).toEqual(ok);
    expect(parseProbeReport({ ...ok, master: "yes" })?.master).toBe(false);
    expect(parseProbeReport({ ...ok, url: "javascript:alert(1)" })).toBeNull();
    expect(parseProbeReport({ ...ok, url: "file:///c:/x" })).toBeNull();
    expect(parseProbeReport({ ...ok, kind: "mp4" })).toBeNull();
    expect(parseProbeReport({ ...ok, type: "other" })).toBeNull();
    expect(parseProbeReport({ ...ok, url: `https://x.example/${"a".repeat(9000)}` })).toBeNull();
    expect(parseProbeReport("hello")).toBeNull();
    expect(parseProbeReport(null)).toBeNull();
  });
});

describe("blob player → its playlist", () => {
  const blobPage = item({ key: "page:p", kind: "page", url: "https://movies.example.com/watch/1", source: "page" });
  const ad = item({ key: "ad", kind: "video", quality: "1080p", size: 30_000_000 });
  const variant = item({ key: "v", kind: "hls", url: "https://cdn.example.net/720/index.m3u8", seenAt: 5 });
  const master = item({ key: "m", kind: "hls", url: "https://cdn.example.net/list.txt", master: true, fromPlayer: true, seenAt: 3 });

  it("prefers the master playlist the player loaded, then newer ones", () => {
    expect(bestStream([variant, master])?.key).toBe("m");
    expect(bestStream([variant, item({ key: "v2", kind: "hls", seenAt: 9 })])?.key).toBe("v2");
    expect(bestStream([ad])).toBeNull();
    expect(bestStream([{ ...master, drm: true }])).toBeNull();
  });

  it("a blob player picks the stream, not a bigger stray video file", () => {
    expect(bestItem([blobPage, ad, variant, master], null, true)?.key).toBe("m");
    // Without a blob player the old ranking stands (a real file first).
    expect(bestItem([ad, master])?.key).toBe("ad");
    // Nothing found yet: still no answer (the user is told to play it first).
    expect(bestItem([blobPage], null, true)).toBeNull();
  });

  it("merging keeps the probe's facts (master, exact frame)", () => {
    const sniffed = item({ key: "m", kind: "hls", url: "https://cdn.example.net/list.txt", pageUrl: "https://movies.example.com/watch/1" });
    const merged = mergeItem([sniffed], master)[0];
    expect(merged.master).toBe(true);
    expect(merged.fromPlayer).toBe(true);
    expect(merged.pageUrl).toBe("https://player.example.org/embed/1");
  });
});

describe("stream naming and sending", () => {
  it("names a stream after the tab title", () => {
    expect(streamFileName("Some Film (2025) | Watch: Online", "https://c.example/x/index.m3u8", "hls")).toBe(
      "Some Film (2025) _ Watch_ Online.mp4"
    );
    expect(streamFileName("", "https://c.example/x/index.m3u8", "hls")).toBe("index.mp4");
    expect(streamFileName(null, "https://c.example/x/", "dash")).toBe("x.mp4");
  });

  it("sends a cross-origin playlist with the player's Origin and an origin-only Referer", async () => {
    const m = item({
      kind: "hls",
      url: "https://cdn.example.net/list.txt?token=abc",
      fileName: "Some Film.mp4",
      mediaType: "application/vnd.apple.mpegurl",
      master: true,
      fromPlayer: true,
    });
    const r = await buildPayload(targetForItem(m), { getCookies: async () => [], userAgent: "UA" });
    expect(r.ok && r.payload).toEqual({
      url: "https://cdn.example.net/list.txt?token=abc",
      pageUrl: "https://player.example.org/embed/1",
      referer: "https://player.example.org/",
      headers: { Origin: "https://player.example.org" },
      userAgent: "UA",
      kind: "stream",
      mediaType: "application/vnd.apple.mpegurl",
      fileName: "Some Film.mp4",
      source: "user-action",
    });
  });

  it("a same-origin playlist keeps the full page as Referer and needs no Origin", async () => {
    const m = item({ kind: "hls", url: "https://player.example.org/hls/master.m3u8", mediaType: "application/vnd.apple.mpegurl" });
    const r = await buildPayload(targetForItem(m), { getCookies: async () => [], userAgent: "UA" });
    expect(r.ok && r.payload.referer).toBe("https://player.example.org/embed/1");
    expect(r.ok && r.payload.headers).toBeUndefined();
  });
});
