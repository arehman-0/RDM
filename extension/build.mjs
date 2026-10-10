// Build: bundle each entry point to a self-contained script in dist/ (no
// runtime dependencies, no remote code) and copy the static files.
import { build } from "esbuild";
import { cpSync, mkdirSync, rmSync } from "node:fs";

const watch = process.argv.includes("--watch");
rmSync("dist", { recursive: true, force: true });
mkdirSync("dist", { recursive: true });
cpSync("static", "dist", { recursive: true });

await build({
  entryPoints: {
    background: "src/background.ts",
    content: "src/content.ts",
    popup: "src/popup.ts",
    options: "src/options.ts",
  },
  outdir: "dist",
  bundle: true,
  // Classic scripts: content scripts can't be ES modules.
  format: "iife",
  target: "chrome116",
  sourcemap: false,
  minify: false,
  logLevel: watch ? "info" : "warning",
});
console.log("built extension/dist");
