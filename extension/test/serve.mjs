// Local test server for the manual extension checks: serves test/ with HTTP
// Range support (206 + Content-Range), like a real media CDN.
//   node test/serve.mjs            → http://127.0.0.1:8080/test-page.html
// Test videos are generated with ffmpeg on first run (ffmpeg must be on PATH).
import { createServer } from "node:http";
import { createReadStream, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { extname, join, normalize, resolve, sep } from "node:path";
import { randomBytes } from "node:crypto";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)));
const MEDIA = join(ROOT, "media");
const PORT = Number(process.env.PORT ?? 8080);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".mp4": "video/mp4",
  ".bin": "application/octet-stream",
  ".js": "text/javascript",
  ".m3u8": "application/vnd.apple.mpegurl",
  ".ts": "video/mp2t",
};

// [file, ffmpeg test source, seconds, bitrate] — all > 100 KB except tiny.mp4.
const VIDEOS = [
  ["sample.mp4", "testsrc2=size=640x360:rate=25,noise=alls=30:allf=t", 8, "2M"],
  ["blob-source.mp4", "smptebars=size=640x360:rate=25,noise=alls=30:allf=t", 8, "2M"],
  ["dynamic.mp4", "mandelbrot=size=640x360:rate=25,noise=alls=30:allf=t", 8, "2M"],
  ["iframe.mp4", "rgbtestsrc=size=640x360:rate=25,noise=alls=30:allf=t", 8, "2M"],
  ["tiny.mp4", "color=c=gray:size=64x36:rate=5", 1, "20k"],
];

function prepare() {
  mkdirSync(MEDIA, { recursive: true });
  for (const [name, src, secs, rate] of VIDEOS) {
    const out = join(MEDIA, name);
    if (existsSync(out)) continue;
    console.log(`generating ${name} with ffmpeg…`);
    execFileSync("ffmpeg", ["-loglevel", "error", "-y", "-f", "lavfi", "-i", src, "-t", String(secs),
      "-c:v", "libx264", "-pix_fmt", "yuv420p", "-b:v", rate, "-movflags", "+faststart", out]);
  }
  // An HLS rendition (playlist + .ts segments) for the stream-download test.
  const hlsDir = join(MEDIA, "hls");
  if (!existsSync(join(hlsDir, "index.m3u8"))) {
    mkdirSync(hlsDir, { recursive: true });
    console.log("generating hls/index.m3u8 with ffmpeg…");
    execFileSync("ffmpeg", ["-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=25,noise=alls=20:allf=t",
      "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100", "-t", "12", "-c:v", "libx264", "-pix_fmt", "yuv420p",
      "-b:v", "1500k", "-c:a", "aac", "-shortest", "-f", "hls", "-hls_time", "3", "-hls_playlist_type", "vod",
      "-hls_segment_filename", join(hlsDir, "seg%03d.ts"), join(hlsDir, "index.m3u8")]);
  }
  const bin = join(MEDIA, "test-2mb.bin");
  if (!existsSync(bin)) writeFileSync(bin, randomBytes(2 * 1024 * 1024));
}

prepare();

// A "third-party player" imitation for blob-player.html: the HLS playlists
// hide behind .txt URLs served as text/plain, and everything under /disguised/
// is refused without a Referer or Origin (like hotlink-protected CDNs).
function disguised(url, req, res) {
  if (!req.headers.referer && !req.headers.origin) {
    console.log(`403 ${url.pathname} (no Referer/Origin)`);
    res.writeHead(403).end("hotlink protection: Referer or Origin required");
    return true;
  }
  const hls = join(MEDIA, "hls");
  const send = (type, body) => {
    console.log(`${req.method} ${url.pathname} (${type})`);
    res.writeHead(200, { "Content-Type": type, "Access-Control-Allow-Origin": "*", "Cache-Control": "no-store" });
    res.end(req.method === "HEAD" ? undefined : body);
  };
  if (url.pathname === "/disguised/master.txt") {
    send("text/plain", "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1600000,RESOLUTION=640x360\nvariant.txt\n");
    return true;
  }
  if (url.pathname === "/disguised/variant.txt") {
    const list = readFileSync(join(hls, "index.m3u8"), "utf8").replace(/^(seg\d+\.ts)$/gm, "/disguised/$1");
    send("text/plain", list);
    return true;
  }
  const seg = /^\/disguised\/(seg\d+\.ts)$/.exec(url.pathname);
  if (seg && existsSync(join(hls, seg[1]))) {
    send("application/octet-stream", readFileSync(join(hls, seg[1])));
    return true;
  }
  // The extension's MAIN-world probe, loaded first by the test page the way
  // Chrome injects it at document_start.
  if (url.pathname === "/disguised/page-probe.js") {
    send("text/javascript", readFileSync(join(ROOT, "..", "dist", "page-probe.js")));
    return true;
  }
  return false;
}

createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://127.0.0.1");
  if (url.pathname.startsWith("/disguised/") && disguised(url, req, res)) return;
  const rel = normalize(decodeURIComponent(url.pathname === "/" ? "/test-page.html" : url.pathname)).replace(/^([\\/])+/, "");
  const file = resolve(ROOT, rel);
  if (!file.startsWith(ROOT + sep) || !existsSync(file) || !statSync(file).isFile()) {
    res.writeHead(404).end("not found");
    return;
  }
  const size = statSync(file).size;
  const headers = {
    "Content-Type": TYPES[extname(file)] ?? "application/octet-stream",
    "Accept-Ranges": "bytes",
    "Cache-Control": "no-store",
  };
  if (url.pathname.endsWith(".bin")) headers["Content-Disposition"] = `attachment; filename="test-2mb.bin"`;
  const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range ?? "");
  let start = 0;
  let end = size - 1;
  if (range) {
    start = range[1] ? Number(range[1]) : size - Number(range[2]);
    end = range[1] && range[2] ? Math.min(Number(range[2]), size - 1) : size - 1;
    if (start > end || start >= size) {
      res.writeHead(416, { "Content-Range": `bytes */${size}` }).end();
      return;
    }
    res.writeHead(206, { ...headers, "Content-Range": `bytes ${start}-${end}/${size}`, "Content-Length": end - start + 1 });
  } else {
    res.writeHead(200, { ...headers, "Content-Length": size });
  }
  console.log(`${req.method} ${url.pathname} ${range ? `range ${start}-${end}` : "full"}`);
  if (req.method === "HEAD") res.end();
  else createReadStream(file, { start, end }).pipe(res);
}).listen(PORT, "127.0.0.1", () => console.log(`test page: http://127.0.0.1:${PORT}/test-page.html`));
