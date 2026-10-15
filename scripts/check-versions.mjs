// Fails unless every version number in the repo is the same:
// package.json, src-tauri/Cargo.toml, src-tauri/tauri.conf.json,
// extension/package.json and extension/static/manifest.json.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => fs.readFileSync(path.join(root, p), "utf8");
const json = (p) => JSON.parse(read(p));

const cargo = /^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m.exec(read("src-tauri/Cargo.toml"))?.[1];
const versions = {
  "package.json": json("package.json").version,
  "src-tauri/Cargo.toml": cargo,
  "src-tauri/tauri.conf.json": json("src-tauri/tauri.conf.json").package.version,
  "extension/package.json": json("extension/package.json").version,
  "extension/static/manifest.json": json("extension/static/manifest.json").version,
};
const distinct = new Set(Object.values(versions));
if (distinct.size !== 1 || distinct.has(undefined)) {
  for (const [f, v] of Object.entries(versions)) console.error(`${v ?? "(missing)"}\t${f}`);
  console.error("version numbers differ");
  process.exit(1);
}
console.log(`version ${[...distinct][0]} everywhere`);
