// Regenerates THIRD_PARTY_NOTICES.md from the real dependency graph:
//   node scripts/gen-third-party-notices.mjs
// Sources: src-tauri/components.json (downloaded tools), `cargo tree` for the
// Rust crates linked into the Windows build, and package-lock.json for the
// npm packages bundled into the UI. Run it after changing dependencies.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => readFileSync(join(root, p), "utf8");

// --- Downloaded tools ------------------------------------------------------
const manifest = JSON.parse(read("src-tauri/components.json"));
const toolNotes = {
  "yt-dlp":
    "Downloaded unmodified from the official GitHub release and run as a separate program; IDM Replica does " +
    "not link against it or include its code. yt-dlp's own code is released under the Unlicense, but its README " +
    "states that the PyInstaller-bundled executable includes GPLv3+ licensed code, so that exe as a whole is " +
    "licensed under GPLv3+ (https://www.gnu.org/licenses/gpl-3.0.txt); see THIRD_PARTY_LICENSES.txt in the " +
    "yt-dlp repository. Source: https://github.com/yt-dlp/yt-dlp at the release tag.",
  qjs:
    "Downloaded unmodified from the official QuickJS-NG GitHub release and run as a separate program. " +
    "Source: https://github.com/quickjs-ng/quickjs.",
  ffmpeg:
    "Downloaded unmodified from BtbN/FFmpeg-Builds (an LGPL build: no GPL or non-free components) and run as a " +
    "separate program; IDM Replica does not link against it. FFmpeg is licensed under the GNU Lesser General " +
    "Public License version 3 or later; the full text is in licenses/FFMPEG-LGPL-3.0.txt. " +
    "Source code: https://github.com/FFmpeg/FFmpeg at the commit named in the version " +
    "(g2a571b6068), and the exact build scripts and patches at https://github.com/BtbN/FFmpeg-Builds " +
    "(release autobuild-2026-09-30-13-08).",
};

// --- Rust crates (what is linked into the Windows release) -----------------
const tree = execFileSync(
  "cargo",
  ["tree", "--manifest-path", join(root, "src-tauri/Cargo.toml"), "--target", "x86_64-pc-windows-msvc",
    "-e", "normal", "--prefix", "none", "--format", "{p}|{l}|{r}"],
  { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 }
);
const crates = new Map();
for (const line of tree.split(/\r?\n/)) {
  const [pkg, license, repo] = line.split("|");
  if (!pkg || !license) continue;
  const m = pkg.match(/^(\S+) v(\S+)/);
  if (!m || m[1] === "idm-replica") continue;
  crates.set(`${m[1]} ${m[2]}`, { name: m[1], version: m[2], license: license.trim(), repo: (repo ?? "").trim() });
}
const rust = [...crates.values()].sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));

// --- npm packages bundled into the UI (production dependencies only) -------
const lock = JSON.parse(read("package-lock.json"));
const npm = [];
for (const [path, info] of Object.entries(lock.packages ?? {})) {
  if (!path.startsWith("node_modules/") || info.dev || info.devOptional) continue;
  const name = path.replace(/^.*node_modules\//, "");
  let license = info.license ?? "";
  let repo = "";
  try {
    const pkg = JSON.parse(read(`${path}/package.json`));
    license ||= typeof pkg.license === "string" ? pkg.license : pkg.license?.type ?? "";
    repo = typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url ?? "";
  } catch {
    // Not installed locally: the lock file's data is used.
  }
  npm.push({ name, version: info.version, license: license || "see package", repo: repo.replace(/^git\+/, "").replace(/\.git$/, "") });
}
npm.sort((a, b) => a.name.localeCompare(b.name));

// --- Write -----------------------------------------------------------------
const byLicense = (list) => {
  const m = new Map();
  for (const c of list) m.set(c.license, (m.get(c.license) ?? 0) + 1);
  return [...m.entries()].sort((a, b) => b[1] - a[1]).map(([l, n]) => `- ${l}: ${n}`).join("\n");
};
const table = (list) =>
  ["| Package | Version | License | Source |", "|---|---|---|---|",
    ...list.map((c) => `| ${c.name} | ${c.version} | ${c.license} | ${c.repo || "—"} |`)].join("\n");

const out = `# Third-party notices

IDM Replica uses the software below. This file is **generated** — do not edit it by hand;
run \`node scripts/gen-third-party-notices.mjs\` after changing dependencies or \`components.json\`.

## 1. Tools downloaded at first run

These are separate programs. IDM Replica downloads each one from the pinned URL in
\`src-tauri/components.json\`, verifies its SHA-256, and runs it as its own process.

${manifest.components
  .map(
    (c) => `### ${c.name} ${c.version}

- License: **${c.license}**
- Homepage: ${c.homepage}
- Pinned download: ${c.url}
- SHA-256: \`${c.sha256}\` (${(c.size / 1024 / 1024).toFixed(1)} MB)
- ${toolNotes[c.id] ?? ""}`
  )
  .join("\n\n")}

## 2. Rust crates linked into the app (${rust.length})

License summary:

${byLicense(rust)}

Most are MIT and/or Apache-2.0 (the standard texts: https://opensource.org/license/mit and
https://www.apache.org/licenses/LICENSE-2.0); each crate's own copyright notice and license file
is in its source repository, linked below. MPL-2.0 crates are used unmodified; their source is at
the linked repository.

${table(rust)}

## 3. npm packages bundled into the UI (${npm.length})

${table(npm)}

The browser extension (\`/extension\`) has no runtime dependencies.
`;
writeFileSync(join(root, "THIRD_PARTY_NOTICES.md"), out);
console.log(`THIRD_PARTY_NOTICES.md: ${manifest.components.length} tools, ${rust.length} crates, ${npm.length} npm packages`);
