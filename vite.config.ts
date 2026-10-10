/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port and disables HMR overlay clutter.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  // Produce a dist/ that matches tauri.conf.json -> build.distDir
  build: {
    target: "esnext",
    outDir: "dist",
  },
  // `npm test`: pure-logic unit tests (no DOM needed).
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
