import { expect, test } from "@playwright/test";

for (const width of [375, 1440]) {
  test(`Discuss is a peer Task column at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/?surface=board&taskColumns");
    const columns = page.locator(".vk-kanban-columns > .vk-kanban-column");
    await expect(columns.locator("h2")).toHaveText([
      "Discuss",
      "Todo",
      "In Progress",
      "In Review",
      "Done",
    ]);
    await expect(
      columns.first().locator(".vk-kanban-column__cards"),
    ).toBeEmpty();
    await expect(
      columns.first().locator(".vk-kanban-column__count"),
    ).toHaveText("0");
    const discussBox = await columns.first().boundingBox();
    const todoBox = await columns.nth(1).boundingBox();
    expect(discussBox?.y).toBe(todoBox?.y);
    expect(discussBox?.width).toBe(todoBox?.width);
    expect(discussBox?.height).toBe(todoBox?.height);
    const discussDot = columns.first().locator(".vk-kanban-column__dot");
    await expect(discussDot).toHaveAttribute("aria-hidden", "true");
    const discussDotBox = await discussDot.boundingBox();
    const todoDotBox = await columns
      .nth(1)
      .locator(".vk-kanban-column__dot")
      .boundingBox();
    expect(discussDotBox?.width).toBe(todoDotBox?.width);
    expect(discussDotBox?.height).toBe(todoDotBox?.height);
  });
}

test("column creation seeds the matching Task status without starting an agent", async ({
  page,
}) => {
  const writes: string[] = [];
  const sessionReads: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/api/sessions/recent"))
      sessionReads.push(request.url());
    if (
      request.url().includes("/api/") &&
      !["GET", "HEAD", "OPTIONS"].includes(request.method())
    ) {
      writes.push(`${request.method()} ${request.url()}`);
    }
  });
  await page.goto("/?surface=board&taskColumns");
  await page
    .getByRole("button", { name: "New discussion", exact: true })
    .click();
  await expect(page.getByTestId("issue-composer-requests")).toHaveText("1");
  await expect(page.getByTestId("task-composer-status")).toHaveText("status-0");
  await page
    .getByRole("button", { name: "Create task in Todo", exact: true })
    .click();
  await expect(page.getByTestId("issue-composer-requests")).toHaveText("2");
  await expect(page.getByTestId("task-composer-status")).toHaveText("status-1");
  await expect(page.locator("html")).toHaveAttribute("data-fixture-route", "/");
  expect(writes).toEqual([]);
  expect(sessionReads).toEqual([]);
});
