import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { checkConnection, connAction, explainError, pair, sendBatch, sendDownload, type FetchFn } from "../src/lib/bridge";
import { cleanName, fileNameFromDisposition, fileNameFromUrl, guessFileName, guessQuality } from "../src/lib/filename";
import { bestItem, mergeItem, type MediaItem } from "../src/lib/items";
import { classify, isAdOrTracker, normalizeUrl, toRecordable, totalSize } from "../src/lib/media";
import { codeMessage, MSG } from "../src/lib/messages";
import { buildPayload, cookieHeader, resolveTarget, targetForItem, type CookieLike } from "../src/lib/request";
import {
  decideTakeover,
  DEFAULTS,
  normalizeSettings,
  OnceSet,
  parseExtList,
  RecentPosts,
  shouldIntercept,
  siteDisabled,
  type DownloadInfo,
} from "../src/lib/settings";
import { hostMatches, isKnownSiteCdn, isKnownVideoPage, knownSiteFor } from "../src/lib/sites";

const MB = 1024 * 1024;

describe("URL normalisation / dedupe", () => {
  it("drops range and volatile params from the key, keeps signatures in the URL", () => {
    const a = normalizeUrl("https://cdn.example.com/v/clip.mp4?id=7&bytestart=0&byteend=999&sig=abc&_=1");
    const b = normalizeUrl("https://cdn.example.com/v/clip.mp4?sig=zzz&id=7&bytestart=1000&byteend=1999#t=3");
    expect(a.key).toBe(b.key);
    expect(a.key).toBe("https://cdn.example.com/v/clip.mp4?id=7");
    expect(a.cleanUrl).toBe("https://cdn.example.com/v/clip.mp4?id=7&sig=abc&_=1");
  });
  it("keeps distinct media distinct", () => {
    expect(normalizeUrl("https://x.example/a.mp4").key).not.toBe(normalizeUrl("https://x.example/b.mp4").key);
    expect(normalizeUrl("https://x.example/v.mp4?q=720").key).not.toBe(normalizeUrl("https://x.example/v.mp4?q=1080").key);
  });
});

describe("media classification", () => {
  it("uses content type first, then the extension", () => {
    expect(classify("https://x/a", "video/mp4")).toEqual({ fragment: false, kind: "video" });
    expect(classify("https://x/a", "audio/mpeg; charset=x")).toEqual({ fragment: false, kind: "audio" });
    expect(classify("https://x/a", "application/vnd.apple.mpegURL")).toEqual({ fragment: false, kind: "hls" });
    expect(classify("https://x/a", "application/x-mpegURL")).toEqual({ fragment: false, kind: "hls" });
    expect(classify("https://x/a", "application/dash+xml")).toEqual({ fragment: false, kind: "dash" });
    expect(classify("https://x/master.m3u8?t=1", "text/plain")).toEqual({ fragment: false, kind: "hls" });
    expect(classify("https://x/movie.mkv", "application/octet-stream")).toEqual({ fragment: false, kind: "video" });
    expect(classify("https://x/song.m4a", null)).toEqual({ fragment: false, kind: "audio" });
    expect(classify("https://x/page.html", "text/html")).toBeNull();
    expect(classify("https://x/movie.mp4", "text/html")).toBeNull();
  });
  it("treats .ts/.m4s segments as fragments (the manifest is listed instead)", () => {
    expect(classify("https://x/seg-00012.ts", "video/mp2t")).toEqual({ fragment: true });
    expect(classify("https://x/chunk-3.m4s", "video/mp4")).toEqual({ fragment: true });
    expect(classify("https://x/seg.ts", "application/octet-stream")).toEqual({ fragment: true });
  });
  it("reads the full size from Content-Range on a 206", () => {
    expect(totalSize("1024", "bytes 0-1023/52428800")).toBe(52428800);
    expect(totalSize("2048", null)).toBe(2048);
    expect(totalSize(null, "bytes */*")).toBeNull();
  });
  it("filters tiny files, ads, fragments, errors and known-site CDNs", () => {
    const base = { statusCode: 200, contentType: "video/mp4" };
    expect(toRecordable({ ...base, url: "https://x.example/a.mp4", contentLength: String(5 * MB) })?.kind).toBe("video");
    expect(toRecordable({ ...base, url: "https://x.example/a.mp4", contentLength: "50000" })).toBeNull();
    // Manifests are small by nature: no size floor.
    expect(toRecordable({ url: "https://x.example/m.m3u8", statusCode: 200, contentLength: "900" })?.kind).toBe("hls");
    expect(toRecordable({ ...base, url: "https://pubads.g.doubleclick.net/x.mp4", contentLength: String(5 * MB) })).toBeNull();
    expect(toRecordable({ ...base, url: "https://x.example/ads/preroll.mp4", contentLength: String(5 * MB) })).toBeNull();
    expect(toRecordable({ ...base, url: "https://x.example/a.mp4", statusCode: 404 })).toBeNull();
    expect(toRecordable({ ...base, url: "https://rr3---sn-x.googlevideo.com/videoplayback?itag=22" })).toBeNull();
    expect(toRecordable({ ...base, url: "blob:https://x.example/123" })).toBeNull();
    // Unknown size is kept (could be big).
    expect(toRecordable({ ...base, url: "https://x.example/a.mp4" })?.size).toBeNull();
    expect(isAdOrTracker("https://imasdk.googleapis.com/js/sdk.mp4")).toBe(true);
  });
});

describe("file names and quality", () => {
  it("reads Content-Disposition (RFC 5987 first)", () => {
    expect(fileNameFromDisposition(`attachment; filename="clip one.mp4"`)).toBe("clip one.mp4");
    expect(fileNameFromDisposition(`attachment; filename=plain.mp4`)).toBe("plain.mp4");
    expect(fileNameFromDisposition(`attachment; filename="x.mp4"; filename*=UTF-8''%E2%9C%93%20ok.mp4`)).toBe("✓ ok.mp4");
    expect(fileNameFromDisposition(null)).toBeNull();
  });
  it("uses the URL's last segment and never keeps paths or illegal characters", () => {
    expect(fileNameFromUrl("https://x.example/media/My%20Video.webm?x=1")).toBe("My Video.webm");
    expect(cleanName("..\\..\\evil<1>.mp4")).toBe("evil_1_.mp4");
    expect(cleanName("...")).toBeNull();
    expect(guessFileName("https://x.example/stream/playlist", "hls")).toBe("playlist.m3u8");
    expect(guessFileName("https://x.example/", "video")).toBe("video.mp4");
  });
  it("guesses quality from URL hints", () => {
    expect(guessQuality("https://x/v_1080p.mp4")).toBe("1080p");
    expect(guessQuality("https://x/1280x720/v.mp4")).toBe("720p");
    expect(guessQuality("https://x/movie-4k.mkv")).toBe("2160p");
    expect(guessQuality("https://x/v_hd.mp4")).toBe("HD");
    expect(guessQuality("https://x/v.mp4")).toBeNull();
  });
});

describe("known stream sites", () => {
  it("matches domains and subdomains, never substrings", () => {
    expect(hostMatches("m.youtube.com", "youtube.com")).toBe(true);
    expect(hostMatches("notyoutube.com", "youtube.com")).toBe(false);
    expect(knownSiteFor("https://www.youtube.com/watch?v=abc")?.name).toBe("YouTube");
    expect(knownSiteFor("https://evil-youtube.com.attacker.net/watch")).toBeNull();
    expect(knownSiteFor("https://www.reddit.com/r/x/comments/1/")?.name).toBe("Reddit");
    expect(knownSiteFor("file:///C:/x.html")).toBeNull();
  });
  it("knows which pages hold a video (YouTube watch / Shorts, not the home page)", () => {
    expect(isKnownVideoPage("https://www.youtube.com/watch?v=dQw4w9WgXcQ")).toBe(true);
    expect(isKnownVideoPage("https://www.youtube.com/shorts/abcdefghijk")).toBe(true);
    expect(isKnownVideoPage("https://youtu.be/dQw4w9WgXcQ")).toBe(true);
    expect(isKnownVideoPage("https://www.youtube.com/")).toBe(false);
    expect(isKnownVideoPage("https://www.youtube.com/feed/subscriptions")).toBe(false);
    expect(isKnownVideoPage("https://vimeo.com/123456")).toBe(true);
    expect(isKnownVideoPage("https://example.com/video")).toBe(false);
    expect(isKnownSiteCdn("https://video.twimg.com/x.mp4")).toBe(true);
  });
});

const item = (over: Partial<MediaItem>): MediaItem => ({
  key: "k",
  url: "https://x.example/a.mp4",
  kind: "video",
  source: "network",
  fileName: "a.mp4",
  mediaType: "video/mp4",
  size: null,
  quality: null,
  pageUrl: "https://x.example/page",
  drm: false,
  seenAt: 1,
  ...over,
});

describe("per-tab list", () => {
  it("dedupes by key, keeping the biggest size and the newest URL", () => {
    let l = mergeItem([], item({ size: 100, url: "https://x.example/a.mp4?sig=1" }));
    l = mergeItem(l, item({ size: 5000, url: "https://x.example/a.mp4?sig=2", seenAt: 2 }));
    l = mergeItem(l, item({ size: null, seenAt: 3 }));
    expect(l).toHaveLength(1);
    expect(l[0].size).toBe(5000);
    expect(l[0].seenAt).toBe(3);
  });
  it("best = hovered video's file, else known-site page stream, else highest quality; never DRM", () => {
    const a = item({ key: "a", quality: "480p", size: 9 * MB });
    const b = item({ key: "b", quality: "1080p", size: 5 * MB });
    const c = item({ key: "c", kind: "page", url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ", drm: true });
    expect(bestItem([a, b, c])?.key).toBe("b");
    expect(bestItem([a, b, { ...c, drm: false }])?.key).toBe("c");
    expect(bestItem([c])).toBeNull();
    // The hovered video's own file wins.
    expect(bestItem([a, b, { ...c, drm: false }], "a")?.key).toBe("a");
    // A blob page stream on an unknown site can't be resolved: never "best".
    const blobPage = item({ key: "p", kind: "page", url: "https://x.example/page" });
    expect(bestItem([a, blobPage])?.key).toBe("a");
    expect(bestItem([blobPage])).toBeNull();
  });
});

describe("request building (mocked chrome.cookies)", () => {
  const cookieJar: Record<string, CookieLike[]> = {
    "https://cdn.example.com/v.mp4": [
      { name: "sid", value: "s3cret" },
      { name: "pref", value: "dark" },
    ],
    "https://www.youtube.com/watch?v=dQw4w9WgXcQ": [{ name: "YSC", value: "y1" }],
  };
  const asked: string[] = [];
  const deps = {
    getCookies: async (url: string) => {
      asked.push(url);
      return cookieJar[url] ?? [];
    },
    userAgent: "Mozilla/5.0 Test",
  };

  it("file: url + referer + UA + cookies for exactly that URL", async () => {
    asked.length = 0;
    const r = await buildPayload(
      { url: "https://cdn.example.com/v.mp4", pageUrl: "https://example.com/watch", kind: "auto", fileName: "v.mp4", size: 1234.7, mediaType: "video/mp4" },
      deps
    );
    expect(r).toEqual({
      ok: true,
      payload: {
        url: "https://cdn.example.com/v.mp4",
        pageUrl: "https://example.com/watch",
        referer: "https://example.com/watch",
        userAgent: "Mozilla/5.0 Test",
        cookies: "sid=s3cret; pref=dark",
        kind: "auto",
        fileName: "v.mp4",
        mediaType: "video/mp4",
        size: 1234,
        source: "user-action",
      },
    });
    expect(asked).toEqual(["https://cdn.example.com/v.mp4"]);
  });
  it("blob player on a known site → pageUrl + kind stream (cookies for the page)", async () => {
    const r = await buildPayload({ url: "blob:https://www.youtube.com/1", pageUrl: "https://www.youtube.com/watch?v=dQw4w9WgXcQ", kind: "auto" }, deps);
    expect(r.ok && r.payload).toMatchObject({ pageUrl: "https://www.youtube.com/watch?v=dQw4w9WgXcQ", kind: "stream", cookies: "YSC=y1" });
    expect(r.ok && r.payload.url).toBeUndefined();
    expect(r.ok && r.payload.fileName).toBeUndefined();
  });
  it("explains what can't be sent", async () => {
    expect(resolveTarget({ url: "blob:https://x.example/1", pageUrl: "https://x.example/p", kind: "auto" })).toEqual({ error: MSG.blobUnsupported });
    expect(resolveTarget({ url: "javascript:alert(1)", pageUrl: null, kind: "auto" })).toEqual({ error: MSG.blockedUrl });
    expect(resolveTarget({ url: "file:///etc/passwd", pageUrl: null, kind: "auto" })).toEqual({ error: MSG.blockedUrl });
    expect(resolveTarget({ url: "https://x.example/a.mp4", pageUrl: null, kind: "auto", drm: true })).toEqual({ error: MSG.drm });
    expect(resolveTarget(targetForItem(item({ kind: "hls", url: "https://x.example/m.m3u8" })))).toEqual({ url: "https://x.example/m.m3u8", pageUrl: "https://x.example/page", kind: "stream" });
    expect(resolveTarget({ url: null, pageUrl: "https://www.youtube.com/", kind: "stream" })).toEqual({ error: MSG.nothingFound });
    // A blob player's page entry on an unknown site explains blob:.
    expect(resolveTarget(targetForItem(item({ kind: "page", url: "https://x.example/p" })))).toEqual({ error: MSG.blobUnsupported });
    expect(resolveTarget(targetForItem(item({ kind: "page", url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ" })))).toEqual({
      pageUrl: "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
      kind: "stream",
    });
    // An HLS manifest on a known site goes to yt-dlp via the page.
    expect(resolveTarget(targetForItem(item({ kind: "hls", pageUrl: "https://vimeo.com/123" })))).toEqual({ pageUrl: "https://vimeo.com/123", kind: "stream" });
  });
  it("cookie header skips unsafe cookies and stays under the bridge limit", () => {
    expect(cookieHeader([{ name: "a", value: "1" }, { name: "bad", value: "x\r\nInjected: 1" }, { name: "b", value: "2" }])).toBe("a=1; b=2");
    expect(cookieHeader([])).toBeUndefined();
    const big = Array.from({ length: 100 }, (_, i) => ({ name: `c${i}`, value: "v".repeat(500) }));
    expect((cookieHeader(big) ?? "").length).toBeLessThanOrEqual(32 * 1024);
  });
});

describe("bridge client (mocked fetch)", () => {
  type Route = (url: string, init?: RequestInit) => Response | Promise<Response>;
  const fake = (route: Route): FetchFn => async (url, init) => route(url, init);
  const j = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
  const down = () => Promise.reject(new TypeError("Failed to fetch"));
  const ID = "pcmcmknffhfehgpkoedecnabbphgnabg";

  it("connection states", async () => {
    const ok = fake((u) => (u.endsWith("/v1/ping") ? j(200, { app: "idm-replica", version: "0.1.0", bridge: 1 }) : j(400, { ok: false, error: "items must hold 1–100 downloads" })));
    expect(await checkConnection(38917, "t", ok, 38917)).toEqual({ state: "connected", port: 38917, version: "0.1.0" });
    expect((await checkConnection(38917, "", ok, 38917)).state).toBe("no-token");
    const bad = fake((u) => (u.endsWith("/v1/ping") ? j(200, { app: "idm-replica", version: "0.1.0" }) : j(401, { ok: false, error: "missing or invalid X-IDM-Token" })));
    expect((await checkConnection(38917, "t", bad, 38917)).state).toBe("token-invalid");
    expect((await checkConnection(38917, "t", fake(() => j(403, { ok: false, error: "origin not allowed" })), 38917)).state).toBe("origin-blocked");
    expect((await checkConnection(38917, "t", fake(() => j(200, { hello: 1 })), 38917)).state).toBe("wrong-app");
    expect((await checkConnection(38917, "t", fake(down), 38917)).state).toBe("not-running");
    const moved = fake((u) => (u.startsWith("http://127.0.0.1:38919/") ? j(200, { app: "idm-replica", version: "0.1.0" }) : down()));
    expect(await checkConnection(38917, "t", moved, 38917)).toEqual({ state: "port-mismatch", foundPort: 38919 });
  });

  it("sends the token header and maps errors to readable text", async () => {
    let seen: Headers | undefined;
    const ok = fake((_u, init) => {
      seen = new Headers(init?.headers);
      return j(200, { ok: true, id: "1", fileName: "a.mp4", queued: false });
    });
    expect(await sendDownload(38917, "tok", { url: "https://x/a.mp4" }, ok, ID)).toEqual({ ok: true, id: "1", fileName: "a.mp4", queued: false, duplicate: false });
    expect(seen?.get("X-IDM-Token")).toBe("tok");
    expect(seen?.get("Content-Type")).toBe("application/json");
    expect(await sendDownload(38917, "tok", {}, fake(down), ID)).toEqual({ ok: false, error: MSG.notRunning, code: "app_unreachable" });
    expect(await sendDownload(38917, "", {}, ok, ID)).toEqual({ ok: false, error: MSG.noToken, code: "no_token" });
    expect(explainError(401, "", ID)).toBe(MSG.badToken);
    expect(explainError(403, "origin not allowed", ID)).toContain(ID);
    expect(explainError(400, "url: only http and https URLs are accepted", ID)).toBe(MSG.blockedUrl);
    expect(explainError(400, 'kind "stream" needs a supported video site in url or pageUrl', ID)).toBe(MSG.siteUnsupported);
    expect(explainError(429, "", ID)).toBe(MSG.tooMany);
  });

  it("batch: per-item results, whole-batch errors applied to every item", async () => {
    const per = fake(() => j(200, { ok: false, results: [{ ok: true, id: "1", fileName: "a", queued: true }, { ok: false, queued: false, error: "boom" }] }));
    const r = await sendBatch(38917, "t", [{}, {}], per, ID);
    expect(r[0]).toEqual({ ok: true, id: "1", fileName: "a", queued: true, duplicate: false });
    expect(r[1]).toEqual({ ok: false, error: "IDM Replica: boom" });
    const all = await sendBatch(38917, "t", [{}, {}], fake(() => j(401, { ok: false, error: "x" })), ID);
    expect(all).toEqual([{ ok: false, error: MSG.badToken }, { ok: false, error: MSG.badToken }]);
  });
});

describe("take-over decision (every Chrome download, decided before cancelling)", () => {
  const dl: DownloadInfo = {
    url: "https://x.example/big.zip",
    filename: "C:\\Users\\me\\Downloads\\big.zip",
    fileSize: 50 * MB,
    incognito: false,
    referrer: "https://x.example/downloads",
  };
  const why = (d: DownloadInfo, s = DEFAULTS) => {
    const r = decideTakeover(d, s);
    return r.take ? "take" : r.reason;
  };

  it("is ON by default with no size filter and empty lists", () => {
    expect(DEFAULTS).toMatchObject({ intercept: true, interceptMinMB: 0, allowExts: [], denyExts: [], neverSites: [] });
  });
  it("takes any file type over http(s), of any size, including unknown size", () => {
    for (const name of ["a.zip", "setup.exe", "doc.pdf", "disk.iso", "clip.mp4", "noext", "page.html"]) {
      expect(why({ ...dl, filename: name, url: `https://x.example/${name}` }), name).toBe("take");
    }
    expect(why({ ...dl, fileSize: 10 })).toBe("take");
    expect(why({ ...dl, fileSize: -1 })).toBe("take");
    expect(why({ ...dl, url: "http://x.example/a.zip" })).toBe("take");
    // The redirect target is what counts.
    expect(why({ ...dl, url: "blob:https://x.example/1", finalUrl: "https://cdn.example/a.zip" })).toBe("take");
  });
  it("leaves to Chrome what the app can't fetch again", () => {
    for (const url of [
      "blob:https://x.example/0d2c",
      "data:application/zip;base64,UEsDBA==",
      "filesystem:https://x.example/temporary/a.zip",
      "file:///C:/Users/me/a.zip",
      "ftp://x.example/a.zip",
      "not a url",
    ]) {
      expect(why({ ...dl, url, finalUrl: url }), url).toBe("not-refetchable");
    }
    expect(why({ ...dl, wasPost: true })).toBe("post-request");
    expect(why({ ...dl, incognito: true })).toBe("incognito");
    expect(why({ ...dl, byOurExtension: true })).toBe("own-download");
  });
  it("honours the user's filters", () => {
    expect(why(dl, { ...DEFAULTS, intercept: false })).toBe("disabled");
    expect(why(dl, { ...DEFAULTS, interceptMinMB: 100 })).toBe("too-small");
    expect(why({ ...dl, fileSize: -1 }, { ...DEFAULTS, interceptMinMB: 1 })).toBe("too-small");
    expect(why({ ...dl, fileSize: -1 }, { ...DEFAULTS, interceptMinMB: 1, allowExts: ["zip"] })).toBe("take");
    expect(why(dl, { ...DEFAULTS, denyExts: ["zip"] })).toBe("type-excluded");
    expect(why(dl, { ...DEFAULTS, allowExts: ["iso"] })).toBe("not-allowed-type");
    expect(why(dl, { ...DEFAULTS, allowExts: ["zip"] })).toBe("take");
    expect(why(dl, { ...DEFAULTS, neverSites: ["x.example"] })).toBe("site-excluded");
    expect(why({ ...dl, url: "https://cdn.other/a.zip" }, { ...DEFAULTS, neverSites: ["x.example"] })).toBe("site-excluded");
    expect(why(dl, { ...DEFAULTS, neverSites: ["other.example"] })).toBe("take");
    expect(shouldIntercept(dl, DEFAULTS)).toBe(true);
  });
  it("handles each download id once (onCreated + onDeterminingFilename)", () => {
    const once = new OnceSet(1000);
    expect(once.claim(7, 0)).toBe(true);
    expect(once.claim(7, 10)).toBe(false);
    expect(once.claim(8, 10)).toBe(true);
    expect(once.claim(7, 5000)).toBe(true); // TTL passed
  });
  it("remembers POST requests briefly", () => {
    const posts = new RecentPosts(1000);
    posts.add("https://x.example/export", 0);
    expect(posts.has("https://x.example/export", 10)).toBe(true);
    expect(posts.has("https://x.example/other", 10)).toBe(false);
    expect(posts.has(undefined, 10)).toBe(false);
    expect(posts.has("https://x.example/export", 5000)).toBe(false);
  });
});

describe("take-over end to end with mocked chrome + fetch", () => {
  // The background's decide → send → cancel ordering, against fakes.
  type Call = string;
  async function run(appUp: boolean, d: DownloadInfo, token = "t"): Promise<Call[]> {
    const calls: Call[] = [];
    const decision = decideTakeover(d, DEFAULTS);
    if (!decision.take) return ["keep:" + decision.reason];
    const built = await buildPayload(
      { url: d.finalUrl || d.url, pageUrl: d.referrer ?? null, kind: "file", fileName: "a.zip", source: "takeover" },
      { getCookies: async () => [{ name: "sid", value: "1" }], userAgent: "UA" }
    );
    if (!built.ok) return ["keep:build"];
    calls.push(`send:${built.payload.source}:${built.payload.cookies}`);
    const fetchFn: FetchFn = async () => {
      if (!appUp) throw new TypeError("Failed to fetch");
      return new Response(JSON.stringify({ ok: true, id: "1", fileName: "a.zip", queued: true }), { status: 200 });
    };
    const r = await sendDownload(38917, token, built.payload, fetchFn, "ext");
    if (!r.ok) return [...calls, `keep:${r.code}`];
    return [...calls, "cancel", "erase"];
  }
  const dl: DownloadInfo = { url: "https://x.example/a.zip", filename: "a.zip", fileSize: 5, incognito: false, referrer: "https://x.example/" };
  it("cancels Chrome's copy only after the app said ok", async () => {
    expect(await run(true, dl)).toEqual(["send:takeover:sid=1", "cancel", "erase"]);
  });
  it("never cancels when the app is down or the token is missing", async () => {
    expect(await run(false, dl)).toEqual(["send:takeover:sid=1", "keep:app_unreachable"]);
    expect(await run(true, dl, "")).toEqual(["send:takeover:sid=1", "keep:no_token"]);
  });
  it("never even sends a blob or POST download", async () => {
    expect(await run(true, { ...dl, url: "blob:https://x.example/1" })).toEqual(["keep:not-refetchable"]);
    expect(await run(true, { ...dl, wasPost: true })).toEqual(["keep:post-request"]);
  });
});

describe("bridge error codes → friendly messages", () => {
  const ID = "pcmcmknffhfehgpkoedecnabbphgnabg";
  it("maps every code the app documents", () => {
    expect(codeMessage("bad_token", ID)).toBe(MSG.badToken);
    expect(codeMessage("origin_not_allowed", ID)).toContain(ID);
    expect(codeMessage("bad_url", ID)).toBe(MSG.blockedUrl);
    expect(codeMessage("unsupported_site", ID)).toBe(MSG.siteUnsupported);
    expect(codeMessage("drm_protected", ID)).toBe(MSG.drm);
    expect(codeMessage("ytdlp_missing", ID)).toBe(MSG.ytdlpMissing);
    expect(codeMessage("ffmpeg_missing", ID)).toBe(MSG.ffmpegMissing);
    expect(codeMessage("app_busy", ID)).toBe(MSG.tooMany);
    expect(codeMessage("payload_too_large", ID)).toBe(MSG.badRequest);
    expect(codeMessage("something_new", ID)).toBeNull();
  });
  it("prefers the code over the HTTP status / text", () => {
    expect(explainError(503, "Missing required tool(s)…", ID, "ytdlp_missing")).toBe(MSG.ytdlpMissing);
    expect(explainError(500, "weird", ID, "")).toBe("IDM Replica: weird");
  });
  it("reads {code, message} responses and the /v1/auth check", async () => {
    const r = await sendDownload(
      38917,
      "t",
      {},
      async () => new Response(JSON.stringify({ ok: false, code: "app_busy", message: "too many" }), { status: 429 }),
      ID
    );
    expect(r).toEqual({ ok: false, error: MSG.tooMany, code: "app_busy" });
    const auth: FetchFn = async (u) =>
      u.endsWith("/v1/ping")
        ? new Response(JSON.stringify({ app: "idm-replica", version: "0.2.0" }), { status: 200 })
        : new Response(null, { status: u.endsWith("/v1/auth") ? 204 : 500 });
    expect(await checkConnection(38917, "t", auth, 38917)).toEqual({ state: "connected", port: 38917, version: "0.2.0" });
  });
});

describe("video pages match the app's yt-dlp detection", () => {
  // Same URL shapes as src-tauri/src/youtube.rs detects_supported_hosts_only.
  it("accepts every supported shape", () => {
    for (const url of [
      "https://www.youtube.com/watch?v=abc",
      "https://youtu.be/dQw4w9WgXcQ",
      "https://vimeo.com/123",
      "https://x.com/user/status/1",
      "https://www.facebook.com/watch/?v=1",
      "https://www.facebook.com/reel/123456",
      "https://www.facebook.com/videos/123456",
      "https://www.facebook.com/SomePage/videos/123456/",
      "https://m.facebook.com/share/v/abc/",
      "https://fb.watch/abcDEF/",
      "https://www.instagram.com/p/Cabc123/",
      "https://www.instagram.com/reel/Cabc123/",
      "https://www.reddit.com/r/videos/comments/abc123/title/",
      "https://v.redd.it/abc123",
      "https://www.twitch.tv/videos/123456789",
      "https://www.twitch.tv/somechannel/clip/FunnyClip",
      "https://clips.twitch.tv/FunnyClip",
      "https://www.dailymotion.com/video/x8abc",
      "https://dai.ly/x8abc",
      "https://www.tiktok.com/@user/video/7234567890",
      "https://vm.tiktok.com/ZMabc/",
    ]) {
      expect(isKnownVideoPage(url), url).toBe(true);
    }
  });
  it("rejects non-video pages and look-alikes", () => {
    for (const url of [
      "https://www.youtube.com/",
      "https://www.facebook.com/",
      "https://www.facebook.com/groups/123",
      "https://www.instagram.com/someuser/",
      "https://www.reddit.com/r/videos/",
      "https://www.twitch.tv/somechannel",
      "https://www.dailymotion.com/",
      "https://www.tiktok.com/@user",
      "https://fb.watch/",
      "https://evilfacebook.com/watch?v=1",
      "https://facebook.com.evil.example/reel/1",
    ]) {
      expect(isKnownVideoPage(url), url).toBe(false);
    }
  });
});

describe("settings", () => {
  it("normalises stored settings and lists", () => {
    expect(normalizeSettings(undefined)).toEqual(DEFAULTS);
    expect(normalizeSettings({ port: "x", token: " abc ", disabledSites: ["Example.COM", 5] })).toMatchObject({ port: 38917, token: "abc", disabledSites: ["example.com"] });
    expect(parseExtList(" .ZIP, iso;exe  zip")).toEqual(["zip", "iso", "exe"]);
    expect(siteDisabled("www.example.com", { ...DEFAULTS, disabledSites: ["example.com"] })).toBe(true);
    expect(siteDisabled("example.org", { ...DEFAULTS, disabledSites: ["example.com"] })).toBe(false);
  });
});

describe("manifest", () => {
  it("has a key whose extension ID is the one documented for pairing", () => {
    const m = JSON.parse(readFileSync(new URL("../static/manifest.json", import.meta.url), "utf8")) as { key: string; permissions: string[] };
    const hex = createHash("sha256").update(Buffer.from(m.key, "base64")).digest("hex").slice(0, 32);
    const id = [...hex].map((h) => String.fromCharCode(97 + parseInt(h, 16))).join("");
    expect(id).toBe("pcmcmknffhfehgpkoedecnabbphgnabg");
    expect(m.permissions.sort()).toEqual(["contextMenus", "cookies", "downloads", "storage", "webRequest"]);
  });
});

describe("one-click pairing (mocked fetch)", () => {
  const ID = "pcmcmknffhfehgpkoedecnabbphgnabg";
  const TOKEN = "ab".repeat(32);
  const j = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });

  it("sends no ID and no token, and returns the token only on Allow", async () => {
    let seen: { url: string; init?: RequestInit } | undefined;
    const allow: FetchFn = async (url, init) => {
      seen = { url, init };
      return j(200, { ok: true, token: TOKEN });
    };
    expect(await pair(38917, allow, ID)).toEqual({ ok: true, token: TOKEN });
    expect(seen?.url).toBe("http://127.0.0.1:38917/v1/pair");
    expect(seen?.init?.method).toBe("POST");
    // The app takes our ID from the Origin header the browser adds; the body is empty.
    expect(seen?.init?.body).toBe("{}");
    expect(new Headers(seen?.init?.headers).has("X-IDM-Token")).toBe(false);
  });
  it("maps every refusal to a readable message and never invents a token", async () => {
    const refused = (status: number, code: string): FetchFn => async () => j(status, { ok: false, code, message: "x" });
    expect(await pair(38917, refused(403, "pairing_denied"), ID)).toEqual({ ok: false, error: MSG.pairDenied, code: "pairing_denied" });
    expect(await pair(38917, refused(409, "pairing_pending"), ID)).toEqual({ ok: false, error: MSG.pairPending, code: "pairing_pending" });
    expect(await pair(38917, refused(429, "pairing_rate_limited"), ID)).toEqual({ ok: false, error: MSG.pairRateLimited, code: "pairing_rate_limited" });
    const blocked = await pair(38917, refused(403, "origin_not_allowed"), ID);
    expect(blocked.ok).toBe(false);
    expect(!blocked.ok && blocked.error).toContain(ID);
    const down: FetchFn = async () => {
      throw new TypeError("Failed to fetch");
    };
    expect(await pair(38917, down, ID)).toEqual({ ok: false, error: MSG.notRunning, code: "app_unreachable" });
    // A malformed "success" (no / bad token) is not accepted.
    const bogus = await pair(38917, async () => j(200, { ok: true, token: "short" }), ID);
    expect(bogus.ok).toBe(false);
  });
  it("offers the right next step for each connection state", () => {
    expect(connAction({ state: "connected", port: 1, version: "1" })).toBe("none");
    expect(connAction({ state: "not-running" })).toBe("start-app");
    expect(connAction({ state: "no-token", port: 1, version: "1" })).toBe("connect");
    expect(connAction({ state: "token-invalid", port: 1, version: "1" })).toBe("connect");
    expect(connAction({ state: "port-mismatch", foundPort: 2 })).toBe("options");
    expect(connAction({ state: "origin-blocked" })).toBe("options");
    expect(MSG.notRunning).toContain("Start IDM Replica");
  });
});
