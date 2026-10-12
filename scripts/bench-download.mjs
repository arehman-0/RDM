// Throughput benchmark for the real release exe (Windows).
//
// Serves a file from a local HTTP range server, starts the app, hands it the
// URL through the browser bridge exactly as the extension does for a Chrome
// take-over (`source:"takeover"` + the extension Origin, so it starts at
// once), and times the download until the engine removes its `.idm` sidecar.
// Every result is checked against the expected SHA-256.
//
//   node scripts/bench-download.mjs                       # the built release exe, 3 runs
//   node scripts/bench-download.mjs --opt 3,s,z           # build + run each opt-level
//   node scripts/bench-download.mjs --exe path\to\app.exe --size-mb 500 --runs 3
//
// Notes:
// * IDM Replica must NOT be running (the script starts and stops it itself).
// * It uses your real profile: the token comes from bridge.json, the files
//   land in your Downloads folder (and are deleted again), and each run leaves
//   a "complete" row in the app's list (Clear completed removes them).
// * `--opt` builds with `cargo build --release --features custom-protocol`
//   plus a `--config profile.release.opt-level=…` override; it does not edit
//   Cargo.toml. Run `npm run build` first so `dist/` exists.

import { createHash, randomBytes } from "node:crypto";
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const TAURI = path.join(ROOT, "src-tauri");
const RELEASE_EXE = path.join(TAURI, "target", "release", "idm-replica.exe");
const APP_ID = "com.idmreplica.app";
const BRIDGE_PORT = 38917;
const PORT_FALLBACKS = 10;
const MIB = 1024 * 1024;

function args() {
  const out = { exe: null, opt: null, sizeMb: 500, runs: 3, json: null };
  const a = process.argv.slice(2);
  for (let i = 0; i < a.length; i++) {
    const next = () => a[++i] ?? fail(`${a[i - 1]} needs a value`);
    if (a[i] === "--exe") out.exe = path.resolve(next());
    else if (a[i] === "--opt") out.opt = next().split(",").map((s) => s.trim());
    else if (a[i] === "--size-mb") out.sizeMb = Number(next());
    else if (a[i] === "--runs") out.runs = Number(next());
    else if (a[i] === "--json") out.json = path.resolve(next());
    else fail(`unknown argument ${a[i]}`);
  }
  if (!(out.sizeMb > 0) || !(out.runs > 0)) fail("--size-mb and --runs must be positive");
  return out;
}

function fail(msg) {
  console.error(`bench: ${msg}`);
  process.exit(1);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------------------
// Range server: byte `o` of every file is PATTERN[o % PATTERN.length].
// ---------------------------------------------------------------------------

const PATTERN = randomBytes(MIB);

function expectedSha(size) {
  const h = createHash("sha256");
  for (let o = 0; o < size; o += PATTERN.length) {
    h.update(PATTERN.subarray(0, Math.min(PATTERN.length, size - o)));
  }
  return h.digest("hex");
}

function startServer(size) {
  const server = http.createServer((req, res) => {
    const base = { "Accept-Ranges": "bytes", "Content-Type": "application/octet-stream" };
    let start = 0;
    let end = size - 1;
    let status = 200;
    const m = /^bytes=(\d+)-(\d*)$/.exec(req.headers.range ?? "");
    if (m) {
      start = Number(m[1]);
      if (m[2] !== "") end = Math.min(Number(m[2]), size - 1);
      if (start > end) {
        res.writeHead(416, { ...base, "Content-Range": `bytes */${size}` }).end();
        return;
      }
      status = 206;
      base["Content-Range"] = `bytes ${start}-${end}/${size}`;
    }
    res.writeHead(status, { ...base, "Content-Length": end - start + 1 });
    if (req.method === "HEAD") {
      res.end();
      return;
    }
    let pos = start;
    const pump = () => {
      while (pos <= end) {
        const at = pos % PATTERN.length;
        const n = Math.min(PATTERN.length - at, end - pos + 1);
        pos += n;
        if (!res.write(PATTERN.subarray(at, at + n))) {
          res.once("drain", pump);
          return;
        }
      }
      res.end();
    };
    res.on("error", () => {});
    pump();
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve({ server, port: server.address().port }));
  });
}

// ---------------------------------------------------------------------------
// The app + its bridge
// ---------------------------------------------------------------------------

async function ping(port) {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/v1/ping`, { signal: AbortSignal.timeout(500) });
    const v = await r.json();
    return v.app === "idm-replica";
  } catch {
    return false;
  }
}

async function findBridge() {
  for (let p = BRIDGE_PORT; p <= BRIDGE_PORT + PORT_FALLBACKS; p++) {
    if (await ping(p)) return p;
  }
  return null;
}

function bridgeConfig() {
  const file = path.join(process.env.APPDATA ?? "", APP_ID, "bridge.json");
  let cfg;
  try {
    cfg = JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    fail(`cannot read ${file} — start IDM Replica once so it creates its token`);
  }
  if (cfg.enabled === false) fail("browser integration is switched off in IDM Replica");
  if (cfg.confirmTakeover) fail('"Confirm browser take-over downloads" is on; switch it off to benchmark');
  const id = cfg.allowedExtensionIds?.[0];
  if (!cfg.token || !id) fail("bridge.json has no token / allowed extension ID");
  return { token: cfg.token, origin: `chrome-extension://${id}` };
}

/** The user's Downloads folder (it can be moved, so ask the shell). */
function downloadsDir() {
  try {
    const out = execFileSync(
      "reg",
      [
        "query",
        "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\User Shell Folders",
        "/v",
        "{374DE290-123F-4565-9164-39C4925E467B}",
      ],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
    );
    const m = /REG_(?:EXPAND_)?SZ\s+(.+)/.exec(out);
    if (m) return m[1].trim().replace(/%([^%]+)%/g, (_, n) => process.env[n] ?? "");
  } catch {
    // not set: the default location
  }
  return path.join(os.homedir(), "Downloads");
}

async function startApp(exe) {
  if ((await findBridge()) !== null) fail("IDM Replica is already running — quit it from the tray first");
  const child = spawn(exe, [], { stdio: "ignore", detached: false });
  child.on("error", (e) => fail(`could not start ${exe}: ${e.message}`));
  for (let i = 0; i < 150; i++) {
    const port = await findBridge();
    if (port !== null) return { child, port };
    if (child.exitCode !== null) fail(`${exe} exited with code ${child.exitCode}`);
    await sleep(200);
  }
  child.kill();
  fail("the app's bridge did not answer within 30 s");
}

async function stopApp(child) {
  // Give the debounced history save a moment, then end the process.
  await sleep(1500);
  child.kill();
  for (let i = 0; i < 50 && (await findBridge()) !== null; i++) await sleep(100);
}

async function sha256File(file) {
  const h = createHash("sha256");
  for await (const chunk of fs.createReadStream(file, { highWaterMark: 4 * MIB })) h.update(chunk);
  return h.digest("hex");
}

/** One timed download; returns seconds. */
async function timeDownload({ bridgePort, serverPort, size, sha, cfg, dir, name }) {
  const dest = path.join(dir, name);
  const sidecar = `${dest}.idm`;
  for (const f of [dest, sidecar]) fs.rmSync(f, { force: true });

  const t0 = process.hrtime.bigint();
  const r = await fetch(`http://127.0.0.1:${bridgePort}/v1/download`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Origin: cfg.origin, "X-IDM-Token": cfg.token },
    body: JSON.stringify({ url: `http://127.0.0.1:${serverPort}/${name}`, source: "takeover" }),
  });
  const v = await r.json();
  if (!v.ok || !v.queued) fail(`the bridge did not start the download: ${JSON.stringify(v)}`);

  // Done = the file has its full size and the engine removed the sidecar
  // (it does that after the flusher drained and fsynced).
  let seen = false;
  for (;;) {
    const have = fs.existsSync(sidecar);
    seen ||= have;
    if (seen && !have && fs.statSync(dest, { throwIfNoEntry: false })?.size === size) break;
    if (Number(process.hrtime.bigint() - t0) / 1e9 > 600) fail("download did not finish within 10 minutes");
    await sleep(5);
  }
  const secs = Number(process.hrtime.bigint() - t0) / 1e9;

  const got = await sha256File(dest);
  fs.rmSync(dest, { force: true });
  if (got !== sha) fail(`checksum mismatch for ${name}: ${got} != ${sha}`);
  return secs;
}

async function benchExe(exe, label, opts, server) {
  const cfg = bridgeConfig();
  const dir = downloadsDir();
  const { child, port } = await startApp(exe);
  const runs = [];
  try {
    await sleep(2000); // let startup work (window, history load) settle
    for (let i = 1; i <= opts.runs; i++) {
      const name = `idm-bench-${process.pid}-${label}-${i}.bin`;
      const secs = await timeDownload({
        bridgePort: port,
        serverPort: server.port,
        size: opts.size,
        sha: opts.sha,
        cfg,
        dir,
        name,
      });
      const mbps = opts.size / MIB / secs;
      runs.push(mbps);
      console.log(`  ${label} run ${i}: ${secs.toFixed(2)} s  ${mbps.toFixed(1)} MB/s`);
    }
  } finally {
    await stopApp(child);
  }
  const sorted = [...runs].sort((a, b) => a - b);
  return {
    label,
    exe,
    exeBytes: fs.statSync(exe).size,
    runs,
    median: sorted[Math.floor(sorted.length / 2)],
    mean: runs.reduce((a, b) => a + b, 0) / runs.length,
  };
}

function build(level) {
  const value = /^\d$/.test(level) ? level : `'${level}'`;
  console.log(`building opt-level ${level} …`);
  execFileSync(
    "cargo",
    [
      "build",
      "--release",
      "--features",
      "custom-protocol",
      "--manifest-path",
      path.join(TAURI, "Cargo.toml"),
      "--config",
      `profile.release.opt-level=${value}`,
    ],
    { stdio: "inherit" },
  );
  const out = path.join(TAURI, "target", "bench");
  fs.mkdirSync(out, { recursive: true });
  const copy = path.join(out, `idm-replica-opt-${level}.exe`);
  fs.copyFileSync(RELEASE_EXE, copy);
  return copy;
}

async function main() {
  if (process.platform !== "win32") fail("this benchmark drives the Windows release exe");
  const a = args();
  const size = a.sizeMb * MIB;
  const targets = [];
  if (a.opt) {
    for (const level of a.opt) targets.push({ label: `opt-${level}`, exe: build(level) });
  } else {
    const exe = a.exe ?? RELEASE_EXE;
    if (!fs.existsSync(exe)) fail(`${exe} not found — build it or pass --exe`);
    targets.push({ label: "release", exe });
  }

  const server = await startServer(size);
  const sha = expectedSha(size);
  console.log(`serving ${a.sizeMb} MB on 127.0.0.1:${server.port}, ${a.runs} run(s) each`);
  const results = [];
  for (const t of targets) results.push(await benchExe(t.exe, t.label, { size, sha, runs: a.runs }, server));
  server.server.close();

  console.log("\nbuild        exe size   runs (MB/s)                 median");
  for (const r of results) {
    console.log(
      `${r.label.padEnd(12)} ${(r.exeBytes / MIB).toFixed(2).padStart(5)} MB   ` +
        `${r.runs.map((x) => x.toFixed(1)).join(" / ").padEnd(27)} ${r.median.toFixed(1)}`,
    );
  }
  if (a.json) fs.writeFileSync(a.json, JSON.stringify(results, null, 2));
}

await main();
