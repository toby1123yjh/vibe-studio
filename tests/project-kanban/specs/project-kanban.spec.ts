import { expect, test, type Page } from "@playwright/test";

async function gotoFixture(page: Page) {
  await page.goto("/");
  await expect(
    page.getByRole("region", { name: "Fixture project board" }),
  ).toBeVisible();
}

test("columns have a complete lightly rounded frame below the toolbar", async ({
  page,
}) => {
  await gotoFixture(page);
  const columns = page.locator(".vk-kanban-column");
  await expect(columns).toHaveCount(3);
  for (const column of await columns.all()) {
    const header = column.locator(".vk-kanban-column__header");
    await expect(header).toHaveCSS("border-top-left-radius", "3px");
    await expect(header).toHaveCSS("border-top-right-radius", "3px");
    await expect(column).toHaveCSS("border-radius", "4px");
    await expect(column).toHaveCSS("border-top-width", "1px");
    await expect(column).toHaveCSS("border-right-width", "1px");
    await expect(column).toHaveCSS("border-bottom-width", "1px");
    await expect(column).toHaveCSS("border-left-width", "1px");
    await expect(column).toHaveCSS("overflow-y", "visible");
    const geometry = await column.evaluate((element) => {
      const bounds = element.getBoundingClientRect();
      const header = element
        .querySelector(".vk-kanban-column__header")!
        .getBoundingClientRect();
      const scroller = element
        .closest(".vk-kanban-scroll")!
        .getBoundingClientRect();
      return {
        gap: bounds.top - scroller.top,
        headerInset: header.top - bounds.top,
      };
    });
    expect(geometry.gap).toBeGreaterThanOrEqual(12);
    expect(geometry.headerInset).toBe(1);
  }
});

test("uses one shared two-dimensional scroll container with sticky headers", async ({
  page,
}) => {
  await page.setViewportSize({ width: 768, height: 560 });
  await gotoFixture(page);

  const sharedScroll = page.locator(".vk-kanban-scroll");
  const cardLists = page.locator(".vk-kanban-column__cards");
  await expect(sharedScroll).toHaveCSS("overflow-x", "auto");
  await expect(sharedScroll).toHaveCSS("overflow-y", "auto");
  await expect(cardLists.first()).toHaveCSS("overflow-y", "visible");
  expect(await page.locator(".vk-kanban-column").count()).toBe(3);

  await sharedScroll.evaluate((element) => {
    element.scrollTop = 240;
    element.scrollLeft = 180;
  });
  const positions = await page
    .locator(".vk-kanban-column__header")
    .evaluateAll((headers) =>
      headers.map((header) => header.getBoundingClientRect().top),
    );
  expect(Math.max(...positions) - Math.min(...positions)).toBeLessThan(1);
  await expect(page.locator(".vk-kanban-column__header").first()).toHaveCSS(
    "position",
    "sticky",
  );
});

test("keeps interactive task controls out of drag activation", async ({
  page,
}) => {
  await gotoFixture(page);
  await page
    .getByRole("button", { name: /Run the canonical agent task/ })
    .click();
  await expect(page.getByTestId("task-open-count")).toHaveText("1");
  await expect(page.getByTestId("move-count")).toHaveText("0");

  await page
    .getByRole("button", { name: "More actions for VK-1", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Delete task" }),
  ).toBeVisible();
  await expect(page.getByTestId("move-count")).toHaveText("0");
});

test("supports keyboard cross-column movement and emits one Escape event", async ({
  page,
}) => {
  await gotoFixture(page);
  await page.evaluate(() => {
    (window as Window & { escapeEvents?: number }).escapeEvents = 0;
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        const state = window as Window & { escapeEvents?: number };
        state.escapeEvents = (state.escapeEvents ?? 0) + 1;
      }
    });
  });

  const card = page.locator('[data-task-id="issue-1"]');
  await card.focus();
  await page.keyboard.press("Space");
  // dnd-kit's KeyboardSensor installs its keydown listener in the next task.
  await page.evaluate(
    () => new Promise<void>((resolve) => setTimeout(resolve, 0)),
  );
  await page.keyboard.press("Escape");
  expect(
    await page.evaluate(
      () => (window as Window & { escapeEvents?: number }).escapeEvents,
    ),
  ).toBe(1);
  await expect(card).toHaveAttribute("data-dragging", "false");
  await expect(page.locator(".vk-kanban-drag-preview")).toHaveCount(0);

  await card.focus();
  await page.keyboard.press("Space");
  await expect(card).toHaveAttribute("data-dragging", "true");
  await page.evaluate(
    () => new Promise<void>((resolve) => setTimeout(resolve, 0)),
  );
  await page.keyboard.press("ArrowRight");
  await expect(
    page.getByText(
      "Draggable item issue-1 was moved over droppable area issue-doing",
    ),
  ).toBeAttached();
  await page.keyboard.press("Space");
  await expect(page.getByTestId("move-count")).toHaveText("1");
  await expect(
    page
      .locator(".vk-kanban-column")
      .nth(1)
      .locator('[data-task-id="issue-1"]'),
  ).toBeAttached();
});

test("rejects keyboard movement beyond the first column", async ({ page }) => {
  await gotoFixture(page);
  const card = page.locator('[data-task-id="issue-1"]');
  const announcement = page.locator('.vk-visually-hidden[role="status"]');

  await card.focus();
  await page.keyboard.press("Space");
  await expect(card).toHaveAttribute("data-dragging", "true");
  await page.keyboard.press("ArrowLeft");
  await page.keyboard.press("Space");

  await expect(card).toHaveAttribute("data-dragging", "false");
  await expect(page.getByTestId("move-count")).toHaveText("0");
  await expect(announcement).toContainText("No valid destination for VK-1");
});

test("rejects pointer drops outside the board", async ({ page }) => {
  await gotoFixture(page);
  const card = page.locator('[data-task-id="issue-1"]');
  const cardBox = await card.boundingBox();
  if (!cardBox) throw new Error("Task card did not produce geometry");

  await page.mouse.move(cardBox.x + 24, cardBox.y + 24);
  await page.mouse.down();
  await page.mouse.move(cardBox.x + 44, cardBox.y + 24, { steps: 2 });
  await page.mouse.move(4, 4, { steps: 5 });
  await page.mouse.up();

  await expect(page.getByTestId("move-count")).toHaveText("0");
  await expect(
    page
      .locator(".vk-kanban-column")
      .first()
      .locator('[data-task-id="issue-1"]'),
  ).toBeAttached();
});

test("uses the pointer threshold and supports cross-column movement", async ({
  page,
}) => {
  await gotoFixture(page);
  const card = page.locator('[data-task-id="issue-1"]');
  const cardBox = await card.boundingBox();
  if (!cardBox) throw new Error("Task card did not produce geometry");

  await page.mouse.move(cardBox.x + 24, cardBox.y + 24);
  await page.mouse.down();
  await page.mouse.move(cardBox.x + 28, cardBox.y + 24);
  await page.mouse.up();
  await expect(page.getByTestId("move-count")).toHaveText("0");
  await expect(card).toHaveAttribute("data-dragging", "false");

  await card.click();
  await expect(
    page.getByRole("complementary", { name: "Task details" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Close panel" }).click();

  const destination = page.locator(".vk-kanban-column").nth(1);
  const nextCardBox = await card.boundingBox();
  const destinationBox = await destination.boundingBox();
  if (!nextCardBox || !destinationBox) {
    throw new Error("Kanban drag geometry is unavailable");
  }
  await page.mouse.move(nextCardBox.x + 24, nextCardBox.y + 24);
  await page.mouse.down();
  await page.mouse.move(nextCardBox.x + 40, nextCardBox.y + 24, { steps: 2 });
  await page.mouse.move(destinationBox.x + 120, destinationBox.y + 180, {
    steps: 8,
  });
  await page.mouse.up();
  await expect(page.getByTestId("move-count")).toHaveText("1");
  await expect(destination.locator('[data-task-id="issue-1"]')).toBeAttached();
});

test("supports keyboard pickup, cancellation, drop, and mutation rollback", async ({
  page,
}) => {
  await gotoFixture(page);
  const firstCard = page.locator('[data-task-id="issue-1"]');

  await firstCard.focus();
  await page.keyboard.press("Space");
  const announcement = page.locator('.vk-visually-hidden[role="status"]');
  await expect(announcement).toContainText("Picked up VK-1");
  await page.waitForTimeout(25);
  await page.keyboard.press("Escape");
  await expect(announcement).toContainText("Movement cancelled");
  await expect(page.getByTestId("move-count")).toHaveText("0");
  await page.waitForTimeout(50);

  await firstCard.focus();
  await page.keyboard.press("Space");
  await expect(firstCard).toHaveAttribute("data-dragging", "true");
  await page.keyboard.press("ArrowDown");
  await expect(
    page.getByText(
      "Draggable item issue-1 was moved over droppable area issue-2",
    ),
  ).toBeAttached();
  await page.keyboard.press("Space");
  await expect(page.getByTestId("move-count")).toHaveText("1");

  await page
    .getByRole("button", { name: "Toggle mutation failure" })
    .click({ force: true });
  await firstCard.focus();
  await page.keyboard.press("Space");
  await expect(firstCard).toHaveAttribute("data-dragging", "true");
  await page.keyboard.press("ArrowUp");
  await expect(
    page.getByText(
      "Draggable item issue-1 was moved over droppable area issue-2",
    ),
  ).toBeAttached();
  await page.keyboard.press("Space");
  await expect(announcement).toContainText("Move failed");
  await expect(page.getByTestId("move-count")).toHaveText("2");
  await expect(
    page
      .locator(".vk-kanban-column")
      .first()
      .locator('[data-task-id="issue-1"]'),
  ).toBeAttached();
});

test("floating panel preserves board geometry and becomes full-screen on mobile", async ({
  browser,
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);
  const board = page.locator(".vk-kanban-scroll");
  const before = await board.boundingBox();
  await page.locator('[data-task-id="issue-1"]').press("Enter");
  const panel = page.getByRole("complementary", { name: "Task details" });
  await expect(panel).toBeVisible();
  expect(await board.boundingBox()).toEqual(before);
  const desktopBox = await panel.boundingBox();
  expect(desktopBox?.x).toBeGreaterThan(0);
  expect(desktopBox?.y).toBeGreaterThan(0);

  const mobile = await browser.newPage({
    viewport: { width: 375, height: 812 },
    hasTouch: true,
    isMobile: true,
  });
  await gotoFixture(mobile);
  const handle = mobile.locator(
    '[data-task-id="issue-1"] [data-touch-drag-handle]',
  );
  await expect(handle).toBeVisible();
  expect(await handle.boundingBox()).toMatchObject({ width: 44, height: 44 });
  await mobile.locator('[data-task-id="issue-1"]').press("Enter");
  const mobilePanel = mobile.getByRole("complementary", {
    name: "Task details",
  });
  await expect(mobilePanel).toHaveCSS("position", "fixed");
  expect(await mobilePanel.boundingBox()).toEqual({
    x: 0,
    y: 0,
    width: 375,
    height: 812,
  });
  await mobile.close();
});
