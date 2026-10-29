import { expect, test } from "@playwright/test";
import { mockWorkflowConversations } from "../support/conversation-api";

test("workspace creation retains feedback until Session discovery and imports the actual first Run", async ({
  page,
}) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.route("https://fonts.googleapis.com/**", (route) => route.abort());
  const api = await mockWorkflowConversations(page, {
    initialRunStatus: "running",
  });
  let releaseSessions = () => {};
  const sessionsGate = new Promise<void>((resolve) => {
    releaseSessions = resolve;
  });
  let sessionsRequested = false;
  await page.route("**/api/sessions?**", async (route) => {
    sessionsRequested = true;
    await sessionsGate;
    await route.fallback();
  });
  let releaseHistory = () => {};
  const historyGate = new Promise<void>((resolve) => {
    releaseHistory = resolve;
  });
  await page.route("**/api/sessions/*/conversation", async (route) => {
    await historyGate;
    await route.fallback();
  });
  await page.route("**/api/workspaces/start", async (route) => {
    await route.fulfill({
      json: {
        success: true,
        data: {
          workspace: {
            id: "visual-workspace",
            container_ref: "C:/fixture",
            workspace_kind: "managed_directory",
            container_ownership: "managed",
            branch: "direct-folder",
            setup_completed_at: null,
            created_at: "2026-10-29T00:00:00Z",
            updated_at: "2026-10-29T00:00:00Z",
            archived: false,
            pinned: false,
            name: "Creation handoff",
            worktree_deleted: false,
          },
          agent_run: api.acknowledgement("main-session"),
        },
        error_data: null,
        message: null,
      },
    });
  });
  await page.goto("/visual.html?mode=creation-handoff");
  await page
    .getByRole("button", { name: "Create conversation", exact: true })
    .click();
  await expect.poll(() => sessionsRequested).toBe(true);
  await expect(
    page.getByRole("button", { name: "Starting conversation…", exact: true }),
  ).toBeVisible();
  releaseSessions();
  // Conversation history is still held: only the returned Run can keep this
  // first composer active. No synthetic timer or prompt-derived lifecycle.
  const stop = page.getByRole("button", { name: "Stop", exact: true });
  await expect(stop).toBeVisible();
  await expect(
    page.locator('[data-composer-activity="running"]'),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Send", exact: true }),
  ).toHaveCount(0);
  releaseHistory();
  await expect
    .poll(() => api.hasConversationConnection("main-session"))
    .toBe(true);
  await expect(stop).toBeVisible();
  api.setRunStatus("main-session", "succeeded");
  await expect(page.getByRole("button", { name: /^Send/ })).toBeVisible();
  await expect(page.locator("[data-composer-activity]")).toHaveCount(0);
});
