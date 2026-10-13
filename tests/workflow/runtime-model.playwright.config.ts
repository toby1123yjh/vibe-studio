import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./specs",
  testMatch: [
    "workflow-runtime-model.spec.ts",
    "workflow-file-driven-model.spec.ts",
  ],
  outputDir: `${process.cwd()}/test-results/workflow-runtime-model`,
  workers: 1,
  reporter: "list",
});
