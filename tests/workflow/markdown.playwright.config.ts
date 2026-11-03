import { defineConfig } from "@playwright/test";
import base from "./visual.playwright.config";

const port = 4200;

export default defineConfig({
  ...base,
  testMatch: ["chat-markdown.spec.ts"],
  outputDir: `${process.cwd()}/test-results/chat-markdown`,
  use: { ...base.use, baseURL: `http://127.0.0.1:${port}` },
  webServer: {
    ...base.webServer,
    command: `pnpm --dir packages/local-web exec vite --host 127.0.0.1 --port ${port} --strictPort --config ../../tests/workflow/visual.vite.config.ts`,
    url: `http://127.0.0.1:${port}/chat-markdown.html`,
    reuseExistingServer: !process.env.CI,
  },
});
