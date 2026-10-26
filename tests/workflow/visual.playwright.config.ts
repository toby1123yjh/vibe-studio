import { defineConfig } from "@playwright/test";
import { resolve } from "node:path";
import base from "./playwright.config";

const port = 4198;

export default defineConfig({
  ...base,
  testMatch: ["workflow-visual.spec.ts", "workflow-conversation-tools.spec.ts"],
  testIgnore: [],
  outputDir: `${process.cwd()}/test-results/workflow-visual`,
  reporter: "list",
  use: { ...base.use, baseURL: `http://127.0.0.1:${port}` },
  webServer: {
    cwd: resolve(__dirname, "../.."),
    // Use the application's cwd so Tailwind resolves the same content sources.
    command: `pnpm --dir packages/local-web exec vite --host 127.0.0.1 --port ${port} --strictPort --config ../../tests/workflow/visual.vite.config.ts`,
    url: `http://127.0.0.1:${port}/visual.html`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
