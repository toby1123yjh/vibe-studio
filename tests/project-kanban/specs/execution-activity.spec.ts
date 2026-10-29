import { expect, test, type Page } from "@playwright/test";
import type { ExecutionStatus, ExecutionSummary } from "shared/types";

function execution(id: string, status: ExecutionStatus): ExecutionSummary {
  return {
    id,
    project_id: "project-1",
    task_id: "issue-1",
    parent_execution_id: null,
    title: `Session ${id}`,
    execution_kind: "agent",
    status,
    open_target: {
      kind: "agent",
      workspace_id: `workspace-${id}`,
      session_id: `session-${id}`,
    },
    created_at: "2026-10-28T00:00:00Z",
    updated_at: "2026-10-28T00:00:00Z",
  };
}

const mixedExecutions = () => [
  execution("waiting", "waiting"),
  execution("failed-1", "failed"),
  execution("failed-2", "failed"),
  execution("running-1", "running"),
  execution("running-2", "running"),
  execution("queued", "pending"),
  { ...execution("child", "running"), parent_execution_id: "running-1" },
];

async function openBoard(page: Page, query = "") {
  await page.goto(`/container.html${query}`);
  await expect(page.locator('[data-task-id="issue-1"]')).toBeVisible();
}

test("discovers a newly running session and refreshes canonical status icons without moving the Task", async ({
  page,
}) => {
  await page.clock.install();
  let executions: ExecutionSummary[] = [];
  const requests: URL[] = [];
  await page.route("**/__fixture/executions?*", async (route) => {
    requests.push(new URL(route.request().url()));
    await route.fulfill({ json: { executions, next_cursor: null } });
  });
  await openBoard(page, "?host=selected-host");
  const card = page.locator('[data-task-id="issue-1"]');
  await expect(card.locator(".vk-kanban-issue-card__tasks")).toHaveCount(0);
  for (const [status, label] of [
    ["draft", "Draft"],
    ["pending", "Queued"],
    ["running", "Running"],
    ["waiting", "Waiting"],
    ["succeeded", "Completed"],
    ["failed", "Failed"],
    ["cancelled", "Cancelled"],
  ] as const) {
    executions = [execution("live", status)];
    await page.clock.fastForward(5_000);
    const statusIcon = card.locator(".vk-kanban-task-preview__status");
    await expect(statusIcon).toHaveAccessibleName(label);
    await expect(statusIcon).toHaveAttribute("title", label);
    await expect(statusIcon).toHaveText("");
    await expect(statusIcon.locator("svg")).toHaveCount(1);
    await expect(
      card.getByRole("button", { name: `${label} Session live`, exact: true }),
    ).toBeVisible();
    if (status === "succeeded") {
      await expect(statusIcon.locator("svg")).toHaveClass(/lucide-check/);
    }
    await expect(
      page
        .locator(".vk-kanban-column")
        .first()
        .locator('[data-task-id="issue-1"]'),
    ).toBeVisible();
  }
  expect(requests).toHaveLength(8);
  expect(
    requests.every(
      (url) =>
        url.searchParams.get("host") === "selected-host" &&
        url.searchParams.get("project_id") === "project-1",
    ),
  ).toBe(true);
});

test("shows concurrent running and waiting counts even when failures fill the preview, and preserves opening the same conversation", async ({
  page,
}, testInfo) => {
  await page.route("**/__fixture/executions?*", (route) =>
    route.fulfill({
      json: { executions: mixedExecutions(), next_cursor: null },
    }),
  );
  await openBoard(page);
  const card = page.locator('[data-task-id="issue-1"]');
  const activity = card.getByRole("group", { name: "Execution activity" });
  for (const [label, count] of [
    ["2 running", "2"],
    ["1 waiting", "1"],
    ["1 queued", "1"],
    ["2 failed", "2"],
  ]) {
    const indicator = activity.getByRole("img", { name: label, exact: true });
    await expect(indicator).toHaveText(count);
    await expect(indicator).toHaveAttribute("title", label);
  }
  await expect(card.locator(".vk-kanban-task-preview__status")).toHaveText([
    "",
    "",
  ]);
  await expect(
    card.getByRole("img", { name: "Waiting", exact: true }),
  ).toBeVisible();
  await expect(
    card.getByRole("img", { name: "Failed", exact: true }),
  ).toBeVisible();
  await expect(
    card.getByRole("button", { name: "+4 executions" }),
  ).toBeVisible();
  await card
    .getByRole("button", { name: "Waiting Session waiting", exact: true })
    .click();
  await expect(page.locator("html")).toHaveAttribute(
    "data-opened-execution",
    JSON.stringify(["project-1", "issue-1", "workspace-waiting"]),
  );
  await expect(card).toHaveAttribute("data-dragging", "false");
  await card.screenshot({
    path: testInfo.outputPath("execution-status-light.png"),
    animations: "disabled",
  });
  await page.evaluate(() => {
    document.documentElement.classList.replace("light", "dark");
    document.documentElement.dataset.theme = "dark";
  });
  await card.screenshot({
    path: testInfo.outputPath("execution-status-dark.png"),
    animations: "disabled",
  });
});

test("keeps localized statuses inside a narrow card and readable with reduced motion", async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 812 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.route("**/__fixture/executions?*", (route) =>
    route.fulfill({
      json: { executions: mixedExecutions(), next_cursor: null },
    }),
  );
  await openBoard(page, "?locale=zh-Hans");
  const card = page.locator('[data-task-id="issue-1"]');
  const activity = card.getByRole("group", { name: "执行活动" });
  await expect(activity.getByRole("img", { name: "2 个运行中" })).toHaveText(
    "2",
  );
  await expect(activity.getByRole("img", { name: "1 个等待处理" })).toHaveText(
    "1",
  );
  await expect(
    card.getByRole("img", { name: "等待处理", exact: true }),
  ).toHaveAttribute("title", "等待处理");
  await expect(
    card.getByRole("img", { name: "失败", exact: true }),
  ).toHaveAttribute("title", "失败");
  await expect(
    card.getByRole("button", { name: "等待处理 Session waiting", exact: true }),
  ).toBeVisible();
  await expect(activity.locator(".vk-task-status-icon--running")).toHaveCSS(
    "animation-name",
    "none",
  );
  const geometry = await card.evaluate((element) => {
    const card = element.getBoundingClientRect();
    return {
      fits: element.scrollWidth <= element.clientWidth,
      childrenFit: Array.from(
        element.querySelectorAll(
          ".vk-kanban-task-preview__status, .vk-kanban-execution-activity",
        ),
      ).every((child) => {
        const bounds = child.getBoundingClientRect();
        return bounds.left >= card.left && bounds.right <= card.right;
      }),
      compactRows: Array.from(
        element.querySelectorAll(".vk-kanban-task-preview"),
      ).every((row) => {
        const title = row
          .querySelector(".vk-kanban-task-preview__title")!
          .getBoundingClientRect();
        const status = row
          .querySelector(".vk-kanban-task-preview__status")!
          .getBoundingClientRect();
        const glyph = row.querySelector("svg")!.getBoundingClientRect();
        return (
          row.children.length === 2 &&
          row.querySelectorAll("svg").length === 1 &&
          status.right <= title.left &&
          status.width === 20 &&
          status.height === 20 &&
          glyph.width === 14 &&
          glyph.height === 14 &&
          glyph.left - status.left === 3 &&
          glyph.top - status.top === 3
        );
      }),
    };
  });
  expect(geometry).toEqual({
    fits: true,
    childrenFit: true,
    compactRows: true,
  });
});

test("pauses automatic refresh while the browser tab is hidden", async ({
  page,
}) => {
  await page.clock.install();
  let requests = 0;
  await page.route("**/__fixture/executions?*", (route) => {
    requests += 1;
    return route.fulfill({
      json: { executions: [execution("live", "running")], next_cursor: null },
    });
  });
  await openBoard(page);
  await expect(
    page.locator(".vk-kanban-task-preview__status"),
  ).toHaveAccessibleName("Running");
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "hidden",
    });
    window.dispatchEvent(new Event("visibilitychange"));
  });
  await page.clock.fastForward(20_000);
  expect(requests).toBe(1);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "visible",
    });
    window.dispatchEvent(new Event("visibilitychange"));
  });
  await expect.poll(() => requests).toBeGreaterThan(1);
});

test("retains all loaded pages after a refresh error and stops polling until explicit retry", async ({
  page,
}) => {
  await page.clock.install();
  let fail = false;
  let requests = 0;
  await page.route("**/__fixture/executions?*", (route) => {
    requests += 1;
    if (fail) return route.fulfill({ status: 503, body: "Unavailable" });
    const nextPage = new URL(route.request().url()).searchParams.has(
      "cursor_id",
    );
    return route.fulfill({
      json: {
        executions: [execution(nextPage ? "second" : "first", "running")],
        next_cursor: nextPage
          ? null
          : { id: "page-2", updated_at: "2026-10-28T00:00:00Z" },
      },
    });
  });
  await openBoard(page);
  await expect(
    page
      .getByRole("group", { name: "Execution activity" })
      .getByRole("img", { name: "2 running" }),
  ).toHaveText("2");
  fail = true;
  await page.clock.fastForward(5_000);
  await expect(
    page.getByText("Executions could not be refreshed."),
  ).toBeVisible();
  await expect(page.locator(".vk-kanban-task-preview")).toHaveCount(2);
  const failedCount = requests;
  await page.clock.fastForward(20_000);
  expect(requests).toBe(failedCount);
  fail = false;
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(
    page.getByText("Executions could not be refreshed."),
  ).toHaveCount(0);
  await expect(page.locator(".vk-kanban-task-preview")).toHaveCount(2);
  await page.clock.fastForward(5_000);
  await expect.poll(() => requests).toBeGreaterThan(failedCount + 2);
});

test("retains the first page after a continuation error without automatically retrying the failed cursor", async ({
  page,
}) => {
  await page.clock.install();
  let failNextPage = true;
  let nextPageRequests = 0;
  await page.route("**/__fixture/executions?*", (route) => {
    const nextPage = new URL(route.request().url()).searchParams.has(
      "cursor_id",
    );
    if (nextPage) {
      nextPageRequests += 1;
      if (failNextPage)
        return route.fulfill({ status: 503, body: "Unavailable" });
    }
    return route.fulfill({
      json: {
        executions: [execution(nextPage ? "second" : "first", "running")],
        next_cursor: nextPage
          ? null
          : { id: "page-2", updated_at: "2026-10-28T00:00:00Z" },
      },
    });
  });
  await openBoard(page);
  await expect(
    page.getByText("Some executions could not be loaded."),
  ).toBeVisible();
  await expect(page.locator(".vk-kanban-task-preview")).toHaveCount(1);
  await page.clock.fastForward(20_000);
  expect(nextPageRequests).toBe(1);
  failNextPage = false;
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(
    page
      .getByRole("group", { name: "Execution activity" })
      .getByRole("img", { name: "2 running" }),
  ).toHaveText("2");
  await expect(
    page.getByText("Some executions could not be loaded."),
  ).toHaveCount(0);
  expect(nextPageRequests).toBe(2);
});
