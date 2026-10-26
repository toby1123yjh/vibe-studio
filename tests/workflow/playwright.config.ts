import { defineConfig, devices } from "@playwright/test";

const port = 4175;
const repoRoot = process.cwd();

export default defineConfig({
  testDir: "./specs",
  testIgnore: ["workflow-visual.spec.ts", "workflow-conversation-tools.spec.ts"],
  outputDir: `${repoRoot}/test-results/workflow`,
  timeout: 30_000,
  expect: {
    timeout: 5_000,
  },
  fullyParallel: false,
  workers: 1,
  reporter: [
    ["list"],
    [
      "html",
      {
        open: "never",
        outputFolder: `${repoRoot}/playwright-report/workflow`,
      },
    ],
  ],
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    screenshot: "only-on-failure",
    trace: "on-first-retry",
  },
  webServer: {
    command: `pnpm exec vite --host 127.0.0.1 --port ${port} --strictPort --config tests/workflow/vite.config.ts`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    url: `http://127.0.0.1:${port}`,
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        ...(process.env.CI ? {} : { channel: "chrome" }),
      },
    },
  ],
});
