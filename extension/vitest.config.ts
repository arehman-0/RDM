import { defineConfig } from "vitest/config";

// Own config, so the root app's vite.config.ts (React, src/**) is not used.
export default defineConfig({
  test: { include: ["test/**/*.test.ts"], environment: "node" },
});
