import { expect, test, type Page } from "@playwright/test";
import {
  mockWorkflowConversations,
  mockWorkflowFileContext,
} from "../support/conversation-api";

declare global {
  interface Window {
    refreshWorkflowFixtureQuery(queryKey: readonly unknown[]): Promise<void>;
  }
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.route("**/api/**", (route) => route.abort());
  await page.route("https://fonts.googleapis.com/**", (route) => route.abort());
  await mockWorkflowFileContext(page);
});

async function openConversation(page: Page) {
  await page.goto("/visual.html?mode=file-run");
  await page.getByTestId("workflow-run-node-build").click();
  const panel = page.getByTestId("workflow-conversation-panel");
  const editor = panel.locator(
    '[data-chatbox-container] [data-lexical-editor="true"]',
  );
  await expect(editor).toBeEditable();
  await expect(panel).toContainText("Original reply for analysis-session");
  return { panel, editor };
}

function deferred() {
  let resolve = () => {};
  const promise = new Promise<void>((accept) => {
    resolve = accept;
  });
  return { promise, resolve: () => resolve() };
}

test("slow or failed repositories do not block history, editing or switching the bound session", async ({
  page,
}) => {
  const api = await mockWorkflowConversations(page);
  const repos = deferred();
  let repoReads = 0;
  await page.route(
    "**/api/workspaces/visual-workspace/repos",
    async (route) => {
      repoReads += 1;
      await repos.promise;
      await route.fulfill({
        status: 500,
        json: { success: false, message: "Repository metadata is unavailable" },
      });
    },
  );
  const { panel, editor } = await openConversation(page);
  expect(repoReads).toBeGreaterThan(0);
  await expect
    .poll(() => api.hasConversationConnection("analysis-session"))
    .toBe(true);
  await editor.fill("The conversation works before repository discovery");

  await page
    .getByTestId("workflow-conversation-work-selector")
    .selectOption("subtask:visual-batch:research");
  await expect(panel).toHaveAttribute("data-session-id", "research-session");
  await expect(panel).toContainText("Original reply for research-session");
  await expect(panel).not.toContainText("Original reply for analysis-session");
  await editor.fill("Independent research draft");
  const editorHandle = await editor.elementHandle();

  repos.resolve();
  await expect(panel).toContainText("Changes unavailable");
  await expect(editor).toHaveText("Independent research draft");
  expect(await editorHandle!.evaluate((element) => element.isConnected)).toBe(
    true,
  );
  await expect(panel).not.toContainText("Conversation is unavailable");

  await panel.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("menuitem", { name: "Inspector", exact: true }).click();
  const tools = panel.getByTestId("conversation-tools");
  await tools.getByRole("tab", { name: "Changes", exact: true }).click();
  await expect(tools).toContainText("Repository metadata is unavailable");
  expect(api.requests.filter((request) => request.startsWith("POST "))).toEqual(
    [],
  );
});

for (const source of ["workspaceRecord", "workspaceSessions"] as const) {
  test(`a failed ${source} refresh retains the current editor and history with a retry`, async ({
    page,
  }) => {
    await mockWorkflowConversations(page);
    const { panel, editor } = await openConversation(page);
    await editor.fill("Keep this draft during the refresh");
    const editorHandle = await editor.elementHandle();
    const historyHandle = await panel
      .locator("[data-semantic-key]")
      .filter({ hasText: "Original reply for analysis-session" })
      .elementHandle();
    const refresh = deferred();
    let refreshStarted = false;
    const endpoint =
      source === "workspaceRecord"
        ? "**/api/workspaces/visual-workspace"
        : "**/api/sessions?*";
    await page.route(endpoint, async (route) => {
      refreshStarted = true;
      await refresh.promise;
      await route.fulfill({
        status: 503,
        json: { success: false, message: "Temporary connection failure" },
      });
    });
    await page.evaluate((source) => {
      void window.refreshWorkflowFixtureQuery([source]);
    }, source);
    await expect.poll(() => refreshStarted).toBe(true);
    await expect(editor).toHaveText("Keep this draft during the refresh");
    await expect(panel).toContainText("Original reply for analysis-session");

    refresh.resolve();
    await expect(panel).toContainText("Workspaces may be out of date");
    await expect(editor).toHaveText("Keep this draft during the refresh");
    expect(await editorHandle!.evaluate((element) => element.isConnected)).toBe(
      true,
    );
    expect(
      await historyHandle!.evaluate((element) => element.isConnected),
    ).toBe(true);

    await page.unroute(endpoint);
    await panel.getByRole("button", { name: "Retry", exact: true }).click();
    await expect(panel).not.toContainText("Workspaces may be out of date");
    await expect(editor).toHaveText("Keep this draft during the refresh");
    expect(await editorHandle!.evaluate((element) => element.isConnected)).toBe(
      true,
    );
  });
}

test("cached metadata never keeps the conversation editable after access is revoked", async ({
  page,
}) => {
  await mockWorkflowConversations(page);
  const { panel, editor } = await openConversation(page);
  await page.route("**/api/workspaces/visual-workspace", (route) =>
    route.fulfill({
      status: 403,
      json: { success: false, message: "Workspace access was revoked" },
    }),
  );
  await page.evaluate(() => {
    void window.refreshWorkflowFixtureQuery(["workspaceRecord"]);
  });
  await expect(panel).toContainText("Conversation is unavailable");
  await expect(panel).toContainText("Workspace access was revoked");
  await expect(editor).toHaveCount(0);
  await expect(panel).not.toContainText("Original reply for analysis-session");
});
