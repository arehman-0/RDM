// Production zip for the Chrome Web Store / Edge Add-ons upload.
//
//   cd extension && npm run store:zip   →  extension/store/out/idm-replica-extension-<version>.zip
//
// Builds dist/ fresh, then zips it WITHOUT: the manifest "key" (the stores
// assign their own ID and reject uploads that carry one), source maps, tests.
// No dependencies: a small ZIP writer (deflate via node:zlib) is below.
import { execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath } from "node:url";

const ext = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
execSync("node build.mjs", { cwd: ext, stdio: "inherit" });

const dist = path.join(ext, "dist");
const manifest = JSON.parse(fs.readFileSync(path.join(dist, "manifest.json"), "utf8"));
delete manifest.key;

const files = [];
(function walk(dir) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    const full = path.join(dir, e.name);
    const rel = path.relative(dist, full).split(path.sep).join("/");
    if (e.isDirectory()) walk(full);
    else if (!/\.map$/.test(rel) && !/(^|\/)test(s)?\//.test(rel) && !/\.test\.[jt]s$/.test(rel)) files.push(rel);
  }
})(dist);

const entries = files.map((rel) => ({
  name: rel,
  data:
    rel === "manifest.json"
      ? Buffer.from(JSON.stringify(manifest, null, 2) + "\n")
      : fs.readFileSync(path.join(dist, rel)),
}));

const outDir = path.join(ext, "store", "out");
fs.mkdirSync(outDir, { recursive: true });
const out = path.join(outDir, `idm-replica-extension-${manifest.version}.zip`);
fs.writeFileSync(out, zip(entries));
console.log(`${path.relative(process.cwd(), out)}: ${entries.length} files, ${fs.statSync(out).size} bytes, no "key"`);

/** Minimal ZIP (deflate, fixed 1980-01-01 timestamps so builds are reproducible). */
function zip(items) {
  const locals = [];
  const central = [];
  let offset = 0;
  for (const { name, data } of items) {
    const nameBuf = Buffer.from(name, "utf8");
    const deflated = zlib.deflateRawSync(data, { level: 9 });
    const crc = zlib.crc32(data);
    const head = Buffer.alloc(30);
    head.writeUInt32LE(0x04034b50, 0);
    head.writeUInt16LE(20, 4); // version needed
    head.writeUInt16LE(0x0800, 6); // UTF-8 names
    head.writeUInt16LE(8, 8); // deflate
    head.writeUInt16LE(0, 10); // time
    head.writeUInt16LE(0x21, 12); // date 1980-01-01
    head.writeUInt32LE(crc, 14);
    head.writeUInt32LE(deflated.length, 18);
    head.writeUInt32LE(data.length, 22);
    head.writeUInt16LE(nameBuf.length, 26);
    head.writeUInt16LE(0, 28);
    locals.push(head, nameBuf, deflated);

    const cen = Buffer.alloc(46);
    cen.writeUInt32LE(0x02014b50, 0);
    cen.writeUInt16LE(20, 4);
    cen.writeUInt16LE(20, 6);
    cen.writeUInt16LE(0x0800, 8);
    cen.writeUInt16LE(8, 10);
    cen.writeUInt16LE(0, 12);
    cen.writeUInt16LE(0x21, 14);
    cen.writeUInt32LE(crc, 16);
    cen.writeUInt32LE(deflated.length, 20);
    cen.writeUInt32LE(data.length, 24);
    cen.writeUInt16LE(nameBuf.length, 28);
    cen.writeUInt32LE(offset, 42);
    central.push(cen, nameBuf);
    offset += head.length + nameBuf.length + deflated.length;
  }
  const cenBuf = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(items.length, 8);
  end.writeUInt16LE(items.length, 10);
  end.writeUInt32LE(cenBuf.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cenBuf, end]);
}
