import { expect, test, type Page } from "@playwright/test";

async function gotoFixture(page: Page) {
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Canonical task title" }),
  ).toBeVisible();
}

async function openInspector(page: Page) {
  await page.getByRole("button", { name: "Inspector", exact: true }).click();
  await expect(page.locator(".vk-agent-workbench__inspector")).toHaveAttribute(
    "data-visible",
    "true",
  );
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    if (sessionStorage.getItem("fixture-initialized")) return;
    localStorage.clear();
    sessionStorage.setItem("fixture-initialized", "true");
  });
});

test("header keeps canonical task title above task and workspace context", async ({
  page,
}) => {
  await gotoFixture(page);
  await expect(
    page.getByText("Task VIB-42 / /workspaces/task-1 / feature/task-1"),
  ).toBeVisible();
});

test("tools start collapsed and a file action opens them without replacing the draft", async ({
  page,
}) => {
  await gotoFixture(page);
  const inspector = page.locator(".vk-agent-workbench__inspector");
  await expect(inspector).toHaveAttribute("data-visible", "false");
  await expect(page.getByTestId("inspector-heavy-panel")).toHaveCount(0);
  const editor = page.getByPlaceholder("Message session-1");
  await editor.fill("Continue this conversation");
  const editorNode = await editor.elementHandle();
  await page.getByRole("button", { name: "Open workspace file" }).click();
  await expect(inspector).toHaveAttribute("data-visible", "true");
  await expect(
    page.getByRole("tab", { name: "Files", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(editor).toHaveValue("Continue this conversation");
  expect(await editorNode!.evaluate((node) => node.isConnected)).toBe(true);
  await page.getByRole("button", { name: "Close Inspector" }).click();
  // The same already-selected mode must reopen, rather than toggling it off.
  await page.getByRole("button", { name: "Open workspace file" }).click();
  await expect(inspector).toHaveAttribute("data-visible", "true");
});

test("Inspector collapse preserves tab, content, width, and focus", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await gotoFixture(page);
  await openInspector(page);

  const inspector = page.locator(".vk-agent-workbench__inspector");
  await page.getByRole("tab", { name: "Terminal" }).click();
  await page.getByLabel("Terminal buffer").fill("pnpm run check");
  const widthBefore = (await inspector.boundingBox())?.width;

  await page.getByRole("button", { name: "Close Inspector" }).click();
  await expect(inspector).toHaveAttribute("data-visible", "false");
  const open = page.getByRole("button", { name: "Inspector" });
  await expect(open).toBeFocused();
  await open.click();

  await expect(page.getByRole("tab", { name: "Terminal" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.getByLabel("Terminal buffer")).toHaveValue(
    "pnpm run check",
  );
  await expect(
    page.getByRole("button", { name: "Close Inspector" }),
  ).toBeFocused();
  expect((await inspector.boundingBox())?.width).toBe(widthBefore);
});

test("Inspector separator supports keyboard resizing", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await gotoFixture(page);
  await openInspector(page);

  const inspector = page.locator(".vk-agent-workbench__inspector");
  const separator = page.getByRole("separator", { name: "Resize Inspector" });
  await separator.focus();
  await expect(separator).toHaveAttribute("aria-valuenow", "380");

  await separator.press("ArrowLeft");
  await expect(separator).toHaveAttribute("aria-valuenow", "396");
  expect((await inspector.boundingBox())?.width).toBe(396);

  await separator.press("Home");
  await expect(separator).toHaveAttribute("aria-valuenow", "360");
  await separator.press("ArrowRight");
  await expect(separator).toHaveAttribute("aria-valuenow", "360");
  await separator.press("End");
  await expect(separator).toHaveAttribute("aria-valuenow", "480");
});

test("Inspector loads tools on demand and preserves the mounted tool when collapsed", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await gotoFixture(page);

  const heavyPanel = page.getByTestId("inspector-heavy-panel");
  await expect(heavyPanel).toHaveCount(0);
  await openInspector(page);
  await expect(heavyPanel).toHaveCount(1);
  await expect(heavyPanel).toHaveAttribute("data-panel", "files");

  await page.getByRole("tab", { name: "Terminal" }).click();
  await expect(heavyPanel).toHaveCount(1);
  await expect(heavyPanel).toHaveAttribute("data-panel", "terminal");
  const mountedTool = await heavyPanel.elementHandle();

  await page.getByRole("button", { name: "Close Inspector" }).click();
  await expect(heavyPanel).toBeHidden();
  expect(await mountedTool!.evaluate((node) => node.isConnected)).toBe(true);
  await page.getByRole("button", { name: "Inspector" }).click();
  await expect(heavyPanel).toHaveCount(1);
  await expect(heavyPanel).toHaveAttribute("data-panel", "terminal");
});

test("Inspector resizing respects the conversation minimum and temporary container constraints", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);
  await openInspector(page);
  const inspector = page.locator(".vk-agent-workbench__inspector");
  const conversation = page.locator(".vk-agent-workbench__conversation");
  const separator = page.getByRole("separator", { name: "Resize Inspector" });
  const editor = page.getByPlaceholder("Message session-1");
  await editor.fill("Keep the same editor while resizing");
  const editorNode = await editor.elementHandle();
  await separator.press("End");
  await expect(separator).toHaveAttribute("aria-valuenow", "480");

  // Simulate the width left by the app sidebar, rather than just the viewport.
  await page.locator("#root").evaluate((node) => (node.style.width = "900px"));
  await expect(inspector).toHaveAttribute("data-overlay", "false");
  await expect(separator).toHaveAttribute("aria-valuemax", "420");
  await expect(separator).toHaveAttribute("aria-valuenow", "420");
  await expect
    .poll(async () => (await conversation.boundingBox())!.width)
    .toBe(480);
  expect(
    await page.evaluate(() =>
      localStorage.getItem("vk-agent-workbench-inspector-width"),
    ),
  ).toBe("480");

  await page.locator("#root").evaluate((node) => (node.style.width = "800px"));
  await expect(inspector).toHaveAttribute("data-overlay", "true");
  await expect
    .poll(async () => (await conversation.boundingBox())!.width)
    .toBe(800);
  const panelBounds = (await inspector.boundingBox())!;
  expect(panelBounds.x).toBeGreaterThanOrEqual(0);
  expect(panelBounds.x + panelBounds.width).toBeLessThanOrEqual(800);

  await page.locator("#root").evaluate((node) => (node.style.width = "1440px"));
  await expect(inspector).toHaveAttribute("data-overlay", "false");
  await expect(separator).toHaveAttribute("aria-valuenow", "480");
  expect(await editorNode!.evaluate((node) => node.isConnected)).toBe(true);
  await expect(editor).toHaveValue("Keep the same editor while resizing");
});

test("Inspector pointer resize clamps both sides and cancellation keeps the accepted width", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);
  await page.locator("#root").evaluate((node) => (node.style.width = "900px"));
  await openInspector(page);
  const inspector = page.locator(".vk-agent-workbench__inspector");
  const separator = page.getByRole("separator", { name: "Resize Inspector" });
  const bounds = (await separator.boundingBox())!;
  const x = bounds.x + bounds.width / 2;
  const y = bounds.y + bounds.height / 2;
  const originalStyles = await page.evaluate(() => ({
    cursor: document.documentElement.style.cursor,
    userSelect: document.documentElement.style.userSelect,
  }));
  await page.mouse.move(x, y);
  await page.mouse.down();
  expect(await page.evaluate(() => document.documentElement.style.cursor)).toBe(
    "col-resize",
  );
  expect(
    await page.evaluate(() => document.documentElement.style.userSelect),
  ).toBe("none");
  await page.mouse.move(100, y);
  await expect(separator).toHaveAttribute("aria-valuenow", "420");
  await page.mouse.move(890, y);
  await expect(separator).toHaveAttribute("aria-valuenow", "360");
  await page.mouse.move(x - 20, y);
  await expect(separator).toHaveAttribute("aria-valuenow", "400");
  await separator.dispatchEvent("pointercancel", { pointerId: 1, clientX: 0 });
  await page.mouse.move(100, y);
  await page.mouse.up();
  await expect(separator).toHaveAttribute("aria-valuenow", "400");
  await expect(inspector).toHaveCSS("transition-duration", "0s");
  expect(
    await page.evaluate(() => ({
      cursor: document.documentElement.style.cursor,
      userSelect: document.documentElement.style.userSelect,
    })),
  ).toEqual(originalStyles);
});

test("Inspector close button has its own header without covering the tabs at minimum width", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await gotoFixture(page);
  await openInspector(page);
  await page.getByRole("separator", { name: "Resize Inspector" }).press("Home");
  const close = page.getByRole("button", { name: "Close Inspector" });
  const closeBounds = (await close.boundingBox())!;
  const tabs = page.getByRole("tablist", { name: "Inspector tabs" });
  const tabBounds = (await tabs.boundingBox())!;
  expect(closeBounds.y + closeBounds.height).toBeLessThanOrEqual(tabBounds.y);
  await expect(close).toHaveCSS("position", "static");
  expect(
    await tabs.evaluate((node) => node.scrollWidth - node.clientWidth),
  ).toBeLessThanOrEqual(1);
  const inspectorBounds = (await page
    .locator(".vk-agent-workbench__inspector")
    .boundingBox())!;
  for (const tab of await tabs.getByRole("tab").all()) {
    const box = (await tab.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(inspectorBounds.x);
    expect(box.x + box.width).toBeLessThanOrEqual(
      inspectorBounds.x + inspectorBounds.width,
    );
  }
});

test("drafts are session isolated and acknowledgements preserve newer typing", async ({
  page,
}) => {
  await gotoFixture(page);
  const editor = page.getByPlaceholder("Message session-1");
  await editor.fill("session one draft");
  await page.getByRole("button", { name: "Switch session" }).click();
  await page.getByPlaceholder("Message session-2").fill("session two draft");
  await page.getByRole("button", { name: "Switch session" }).click();
  await expect(editor).toHaveValue("session one draft");

  await page.getByRole("button", { name: "Send snapshot" }).click();
  await editor.fill("newer text");
  await page.getByRole("button", { name: "Acknowledge send" }).click();
  await expect(editor).toHaveValue("newer text");

  await page.reload();
  await expect(page.getByPlaceholder("Message session-1")).toHaveValue(
    "newer text",
  );
});

test("runtime controls remain distinct and expose canonical blocked reasons", async ({
  page,
}) => {
  await gotoFixture(page);
  await expect(
    page.getByRole("button", { name: "Send", exact: true }),
  ).toBeEnabled();
  await expect(page.getByRole("button", { name: "Follow up" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Follow up" })).toHaveAttribute(
    "title",
    "provider_capability_missing",
  );
  await expect(page.getByRole("button", { name: "Queue" })).toHaveAttribute(
    "title",
    "runtime_terminal",
  );
  await expect(page.getByRole("button", { name: "Stop" })).toBeEnabled();
});

test("narrow layout opens a full-screen Inspector and honors reduced motion", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.setViewportSize({ width: 390, height: 844 });
  await gotoFixture(page);
  await openInspector(page);

  const inspector = page.locator(".vk-agent-workbench__inspector");
  const box = await inspector.boundingBox();
  expect(box?.width).toBe(390);
  expect(box?.height).toBe(844);
  await expect(inspector).toHaveCSS("position", "fixed");
  const transitionDuration = await inspector.evaluate((node) =>
    parseFloat(getComputedStyle(node).transitionDuration),
  );
  expect(transitionDuration).toBeLessThanOrEqual(0.001);

  await page.getByRole("button", { name: "Close Inspector" }).click();
  await expect(page.getByLabel("Conversation timeline")).toBeVisible();
  await expect(page.getByRole("button", { name: "Inspector" })).toBeFocused();
});
