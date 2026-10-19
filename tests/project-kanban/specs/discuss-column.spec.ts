import { expect, test } from "@playwright/test";

test("Tasks move into and out of Discuss with their existing details and executions", async ({
  page,
}) => {
  await page.goto("/?discuss");
  const discuss = page.getByRole("region", { name: "Discuss", exact: true });
  const todo = page.getByRole("region", { name: "Todo", exact: true });
  const card = page.locator('[data-task-id="issue-1"]');
  await expect(todo.locator('[data-task-id="issue-1"]')).toBeVisible();
  await card.focus();
  await page.keyboard.press("Space");
  await expect(card).toHaveAttribute("data-dragging", "true");
  // dnd-kit attaches its document keyboard listener after activation.
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await page.keyboard.press("ArrowLeft");
  await expect(
    page.getByText(
      "Draggable item issue-1 was moved over droppable area discuss",
    ),
  ).toBeAttached();
  await page.keyboard.press("Space");
  await expect(page.getByTestId("move-count")).toHaveText("1");
  await expect(discuss.locator('[data-task-id="issue-1"]')).toBeVisible();
  expect(
    JSON.parse((await page.getByTestId("last-move").textContent()) ?? "[]"),
  ).toContainEqual({ id: "issue-1", statusId: "discuss", sortOrder: 1001 });

  await card
    .getByRole("heading", { name: "Keyboard and pointer interaction" })
    .click();
  await expect(
    page.getByRole("complementary", { name: "Task details" }),
  ).toContainText("issue-1");
  await page.getByRole("button", { name: "Close panel" }).click();
  await card
    .getByRole("button", { name: /Run the canonical agent task/ })
    .click();
  await expect(page.getByTestId("task-open-count")).toHaveText("1");
  await expect(page.getByTestId("move-count")).toHaveText("1");

  await card.focus();
  await page.keyboard.press("Space");
  await expect(card).toHaveAttribute("data-dragging", "true");
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await page.keyboard.press("ArrowRight");
  await expect(
    page.getByText(
      "Draggable item issue-1 was moved over droppable area issue-2",
    ),
  ).toBeAttached();
  await page.keyboard.press("Space");
  await expect(page.getByTestId("move-count")).toHaveText("2");
  await expect(todo.locator('[data-task-id="issue-1"]')).toBeVisible();
  await expect(discuss.locator(".vk-kanban-column__count")).toHaveText("0");
});

test("pointer drag into Discuss uses the regular Task movement path", async ({
  page,
}) => {
  await page.goto("/?discuss");
  const card = page.locator('[data-task-id="issue-2"]');
  const discuss = page.getByRole("region", { name: "Discuss", exact: true });
  const cardBox = await card.boundingBox();
  const target = await discuss.boundingBox();
  if (!cardBox || !target) throw new Error("Task drag geometry is missing");
  await page.mouse.move(cardBox.x + 24, cardBox.y + 24);
  await page.mouse.down();
  await page.mouse.move(cardBox.x + 40, cardBox.y + 24, { steps: 2 });
  await page.mouse.move(target.x + 120, target.y + 160, { steps: 8 });
  await page.mouse.up();
  await expect(page.getByTestId("move-count")).toHaveText("1");
  await expect(discuss.locator('[data-task-id="issue-2"]')).toBeVisible();
  expect(
    JSON.parse((await page.getByTestId("last-move").textContent()) ?? "[]"),
  ).toContainEqual({ id: "issue-2", statusId: "discuss", sortOrder: 1001 });
});
