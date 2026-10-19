import { expect, test } from "@playwright/test";
import {
  childrenFor,
  dashboardResponse,
  mockDashboard,
  sessionTarget,
  workRow,
} from "../support/dashboard-api";
import {
  mockWorkflowConversations,
  mockWorkflowFileContext,
} from "../../workflow/support/conversation-api";

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.route("https://fonts.googleapis.com/**", (route) => route.abort());
});

test("uses a semantic operations table, distinct work modes and lazy Git context", async ({
  page,
}) => {
  const api = await mockDashboard(page);
  const historyRequests: string[] = [];
  page.on("request", (request) => {
    if (/agent-runs|executions\/.*logs/.test(request.url()))
      historyRequests.push(request.url());
  });
  await page.goto("/");
  const table = page.getByRole("table", { name: "Agent work monitoring" });
  await expect(table).toBeVisible();
  await expect(table.getByTestId("dashboard-work-row")).toHaveCount(5);
  await expect(page.locator(".vk-dashboard-card")).toHaveCount(0);
  const colours = await page
    .getByTestId("dashboard-work-row")
    .evaluateAll((rows) =>
      rows.map(
        (row) => getComputedStyle(row.firstElementChild!).borderLeftColor,
      ),
    );
  expect(new Set(colours).size).toBe(5);
  await expect(table).toContainText("Tool completed · inspect_project");
  expect(api.contexts).toEqual([]);
  expect(historyRequests).toEqual([]);
  await table
    .getByRole("button", { name: "Show context: Ship release" })
    .click();
  await expect(page.getByTestId("dashboard-context")).toContainText(
    "git@github.com:owner/studio.git",
  );
  await expect(
    page.getByRole("link", { name: "Open repository" }),
  ).toHaveAttribute("href", "https://github.com/owner/studio");
  expect(api.contexts).toHaveLength(1);
});

test("polls one batch, expands workflow work and keeps Node review separate from Agent success", async ({
  page,
}) => {
  const api = await mockDashboard(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Expand Launch workflow" }).click();
  await expect(
    page.getByRole("button", { name: "Research", exact: true }),
  ).toBeVisible();
  const node = page.locator('[data-work-key="child-node"]');
  await expect(node).toContainText("Human review");
  await expect(node).toContainText("Agent: Succeeded");
  await expect(page.locator('[data-work-key="child-subtask"]')).toContainText(
    "Internal work",
  );
  await expect(
    page.locator('[data-work-key="child-arena_candidate"]'),
  ).toHaveAttribute("data-mode", "arena");
  await expect
    .poll(
      () =>
        api.requests.filter((entry) => entry.query.expanded.length === 1)
          .length,
    )
    .toBeGreaterThanOrEqual(2);
  await page.getByRole("button", { name: "Collapse Launch workflow" }).click();
  await expect(page.locator('[data-child="true"]')).toHaveCount(0);
  await expect.poll(() => api.requests.at(-1)?.query.expanded.length).toBe(0);
});

test("keeps snapshot order stable on reply updates and refreshes membership explicitly", async ({
  page,
}) => {
  const response = dashboardResponse();
  const api = await mockDashboard(page, (query) => ({
    ...response,
    page: query.page,
  }));
  await page.goto("/");
  await expect(
    page.getByRole("button", { name: "Ship release", exact: true }),
  ).toBeVisible();
  response.has_updates = true;
  response.rows[0].preview!.text = "A fresh reply without moving the work";
  await expect(
    page.getByRole("button", { name: /A fresh reply/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Updates available · Refresh" }),
  ).toBeVisible();
  expect(
    await page
      .getByTestId("dashboard-work-row")
      .evaluateAll((rows) =>
        rows.map((row) => row.getAttribute("data-work-key")),
      ),
  ).toEqual([
    "task:task-work",
    "workflow:workflow-work",
    "session:session-work",
    "arena:arena-work",
    "unresolved:unresolved-work",
  ]);
  expect(api.requests.at(-1)?.query.snapshot_id).toBe("snapshot-1");
  await page
    .getByRole("button", { name: "Updates available · Refresh" })
    .click();
  await expect
    .poll(() =>
      api.requests.some(
        (entry) => entry.query.refresh && entry.query.snapshot_id === null,
      ),
    )
    .toBe(true);
});

test("server filters reset snapshot and page while search is debounced", async ({
  page,
}) => {
  const api = await mockDashboard(page);
  await page.goto("/");
  const filters = page.getByRole("button", { name: "Filters", exact: true });
  await expect(page.getByRole("combobox")).toHaveCount(0);
  await expect(page.locator(".session-dashboard__tabs")).toHaveCount(0);
  expect(api.requests.at(-1)?.query.filters.status).toBe("current");
  await filters.click();
  await page
    .getByRole("combobox", { name: "Work mode" })
    .selectOption("workflow");
  await expect
    .poll(() => api.requests.at(-1)?.query.filters.kinds)
    .toEqual(["workflow"]);
  expect(api.requests.at(-1)?.query.snapshot_id).toBeNull();
  expect(api.requests.at(-1)?.query.page).toBe(0);
  await page.keyboard.press("Escape");
  await expect(filters).toBeFocused();
  await page
    .getByRole("textbox", { name: "Search work, projects, directories…" })
    .fill(" launch ");
  await expect
    .poll(() => api.requests.at(-1)?.query.filters.search)
    .toBe("launch");
  await filters.click();
  await expect(page.getByRole("combobox")).toHaveCount(4);
  await expect(
    page.getByRole("combobox", { name: "Recent history" }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "Reset filters", exact: true })
    .click();
  await expect.poll(() => api.requests.at(-1)?.query.filters.kinds).toEqual([]);
  expect(api.requests.at(-1)?.query.filters.history_hours).toBe(24);
  expect(api.requests.at(-1)?.query.filters.search).toBe("launch");
  await page.keyboard.press("Escape");
  await expect(filters).toBeFocused();
});

test("root and execution child pages preserve their own snapshot identities", async ({
  page,
}) => {
  const workflow = workRow("workflow", "Launch workflow");
  const all = [
    workflow,
    ...Array.from({ length: 20 }, (_, index) => ({
      ...workRow("session", `Paged session ${index + 1}`),
      work: { kind: "session" as const, id: `paged-${index}` },
    })),
  ];
  const api = await mockDashboard(page, (query) => {
    const response = dashboardResponse(
      all.slice(query.page * 20, (query.page + 1) * 20),
    );
    response.page = query.page;
    response.total = all.length;
    response.latest_total = all.length;
    response.expanded = query.expanded.map((entry) => ({
      work: entry.work,
      workflow_run_id: workflow.workflow_run_id,
      snapshot_id: "child-pages",
      page: entry.page,
      page_size: 2,
      total: 4,
      rows: childrenFor(workflow).slice(entry.page * 2, (entry.page + 1) * 2),
      has_updates: false,
      invalidation: null,
    }));
    return response;
  });
  await page.goto("/");
  await expect(page.getByTestId("dashboard-work-row")).toHaveCount(20);
  await page.getByRole("button", { name: "Expand Launch workflow" }).click();
  await expect(
    page.getByRole("button", { name: "Coordinator", exact: true }),
  ).toBeVisible();
  const children = page.getByRole("navigation", {
    name: "Execution detail pages",
  });
  await children
    .getByRole("button", { name: "Next page", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Research", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Coordinator", exact: true }),
  ).toHaveCount(0);
  expect(api.requests.at(-1)?.query.expanded[0]).toMatchObject({
    page: 1,
    snapshot_id: "child-pages",
  });
  const roots = page.getByRole("navigation", {
    name: "Work pages",
    exact: true,
  });
  await roots.getByRole("button", { name: "Next page", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Paged session 20", exact: true }),
  ).toBeVisible();
  await expect(page.getByTestId("dashboard-work-row")).toHaveCount(1);
  expect(api.requests.at(-1)?.query).toMatchObject({
    page: 1,
    snapshot_id: "snapshot-1",
    expanded: [],
  });
  await roots
    .getByRole("button", { name: "Previous page", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Launch workflow", exact: true }),
  ).toBeVisible();
  await expect(page.locator('[data-child="true"]')).toHaveCount(0);
});

test("returns to a valid page when ongoing work on the last page ends", async ({
  page,
}) => {
  const rows = Array.from({ length: 21 }, (_, index) => ({
    ...workRow("session", `Ongoing session ${index + 1}`),
    work: { kind: "session" as const, id: `ongoing-${index}` },
  }));
  const api = await mockDashboard(page, (query) => ({
    ...dashboardResponse(rows.slice(query.page * 20, (query.page + 1) * 20)),
    page: query.page,
    total: rows.length,
    latest_total: rows.length,
  }));
  await page.goto("/");
  await expect(page.getByTestId("dashboard-work-row")).toHaveCount(20);
  await page.getByRole("button", { name: "Next page", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Ongoing session 21", exact: true }),
  ).toBeVisible();
  rows.splice(10);
  await expect(page.getByTestId("dashboard-work-row")).toHaveCount(10);
  await expect(
    page.getByRole("button", { name: "Ongoing session 21", exact: true }),
  ).toHaveCount(0);
  expect(api.requests.at(-1)?.query.page).toBe(0);
  expect(api.requests.at(-1)?.query.snapshot_id).toBe("snapshot-1");
});

test("retains the last successful page on a refresh error and retries", async ({
  page,
}) => {
  await mockDashboard(page);
  let fail = false;
  await page.route("**/api/dashboard/query", (route) =>
    fail
      ? route.fulfill({
          status: 503,
          json: { success: false, message: "Host offline" },
        })
      : route.fallback(),
  );
  await page.goto("/");
  await expect(
    page.getByRole("button", { name: "Ship release", exact: true }),
  ).toBeVisible();
  fail = true;
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(
    page.getByTestId("dashboard-page").getByRole("status"),
  ).toContainText("Refresh failed");
  await expect(
    page.getByRole("button", { name: "Ship release", exact: true }),
  ).toBeVisible();
  fail = false;
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByText("Refresh failed.", { exact: false })).toHaveCount(
    0,
  );
});

test("Host and user switches discard old requests, filters, selection and pagination", async ({
  page,
}) => {
  let release = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const api = await mockDashboard(page, async (_query, host) => {
    if (host === "host-a") await gate;
    return dashboardResponse([
      workRow(
        "session",
        host === "host-a"
          ? "Old host work"
          : host === "host-b"
            ? "New host work"
            : "Local work",
      ),
    ]);
  });
  await page.goto("/");
  await expect(
    page.getByRole("button", { name: "Local work", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Host A", exact: true }).click();
  await expect
    .poll(() => api.requests.some((entry) => entry.host === "host-a"))
    .toBe(true);
  await page.getByRole("button", { name: "Host B", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "New host work", exact: true }),
  ).toBeVisible();
  release();
  await expect(
    page.getByRole("button", { name: "Old host work", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Filters", exact: true }).click();
  await page
    .getByRole("combobox", { name: "Work mode" })
    .selectOption("session");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Other user", exact: true }).click();
  await page.getByRole("button", { name: "Filters", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "Work mode" })).toHaveValue(
    "",
  );
  await expect.poll(() => api.requests.at(-1)?.query.snapshot_id).toBeNull();
});

test("pauses the page poll while hidden and resumes when visible", async ({
  page,
}) => {
  const api = await mockDashboard(page);
  await page.goto("/");
  await expect(page.getByRole("table")).toBeVisible();
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "hidden",
    });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  const count = api.requests.length;
  await page.waitForTimeout(2300);
  expect(api.requests).toHaveLength(count);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "visible",
    });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await expect.poll(() => api.requests.length).toBeGreaterThan(count);
});

test("opens the original exact conversation, preserves drafts and restores focus without cancelling", async ({
  page,
}) => {
  const chat = await mockWorkflowConversations(page);
  await mockDashboard(page);
  const mutations: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && /cancel|stop|delete/i.test(request.url()))
      mutations.push(request.url());
  });
  await page.goto("/");
  const trigger = page.getByRole("button", {
    name: "Ship release",
    exact: true,
  });
  await trigger.click();
  const panel = page.getByTestId("dashboard-conversation-panel");
  await expect(panel).toHaveCSS("position", "absolute");
  await expect(panel).toHaveAttribute("data-session-id", "research-session");
  await expect(panel).toContainText("Original reply for research-session");
  await expect(panel.getByTestId("dashboard-selected-run")).toContainText(
    "agent-research-session",
  );
  const editor = panel.locator('[contenteditable="true"]').first();
  await editor.fill("Keep my original Session draft");
  await panel
    .getByRole("button", { name: "Close conversation", exact: true })
    .click();
  await expect(trigger).toBeFocused();
  await expect.poll(() => chat.writes.includes("research-session")).toBe(true);
  await page.getByRole("button", { name: "Explore idea", exact: true }).click();
  await expect(panel).toHaveAttribute("data-session-id", "analysis-session");
  await expect(editor).not.toContainText("Keep my original Session draft");
  await panel
    .getByRole("button", { name: "Close conversation", exact: true })
    .click();
  await trigger.click();
  await expect(editor).toHaveText("Keep my original Session draft");
  expect(mutations).toEqual([]);
});

test("work leaving the ongoing list keeps its already opened original conversation", async ({
  page,
}) => {
  await mockWorkflowConversations(page);
  const response = dashboardResponse([
    workRow("task", "Finishing work"),
    workRow("session", "Still working"),
  ]);
  await mockDashboard(page, () => response);
  await page.goto("/");
  await page
    .getByRole("button", { name: "Finishing work", exact: true })
    .click();
  const panel = page.getByTestId("dashboard-conversation-panel");
  await expect(panel).toHaveAttribute("data-session-id", "research-session");
  response.rows = response.rows.slice(1);
  response.total = response.latest_total = response.counts.work_count = 1;
  response.counts.active_session_count = response.counts.active_run_count = 1;
  await expect(page.getByTestId("dashboard-work-row")).toHaveCount(1);
  await expect(
    page.getByRole("button", { name: "Finishing work", exact: true }),
  ).toHaveCount(0);
  await expect(panel).toHaveAttribute("data-session-id", "research-session");
  await expect(panel).toContainText("Original reply for research-session");
});

test("workflow child uses the original delivery/file preview and keeps its exact Session", async ({
  page,
}) => {
  await mockWorkflowConversations(page);
  await mockWorkflowFileContext(page);
  await mockDashboard(page);
  await page.route("**/api/workflow-runs/visual-run/files/content?*", (route) =>
    route.fulfill({
      json: {
        success: true,
        data: {
          kind: "text",
          path: "deliverables/research report.md",
          name: "research report.md",
          mime_type: "text/markdown",
          language: "markdown",
          content: "# Research delivered\n\nCurrent file.",
          raw_url: null,
          size_bytes: 42,
          truncated: false,
        },
      },
    }),
  );
  await page.goto("/");
  await page.getByRole("button", { name: "Expand Launch workflow" }).click();
  await page.getByRole("button", { name: "Research", exact: true }).click();
  const panel = page.getByTestId("workflow-conversation-panel");
  await expect(panel).toHaveAttribute("data-session-id", "research-session");
  await expect(panel).toContainText("Original reply for research-session");
  const editor = panel.locator('[contenteditable="true"]').first();
  await editor.fill("Keep draft while reading output");
  await panel.getByTestId("workflow-deliverables-toggle").click();
  await panel
    .getByRole("button", {
      name: "deliverables/research report.md",
      exact: true,
    })
    .click();
  await expect(
    panel.getByRole("heading", { name: "Research delivered" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(editor).toHaveText("Keep draft while reading output");
  await expect(panel).toHaveAttribute("data-session-id", "research-session");
});

test("polling preserves the table scroll and an already opened Session and Agent Run", async ({
  page,
}) => {
  await mockWorkflowConversations(page);
  const rows = Array.from({ length: 20 }, (_, index) => ({
    ...workRow("task", `Working session ${index + 1}`),
    work: { kind: "task" as const, id: `scroll-${index}` },
  }));
  const response = dashboardResponse(rows);
  const api = await mockDashboard(page, () => response);
  await page.goto("/");
  const scroller = page.getByTestId("dashboard-scroller");
  await expect(page.getByTestId("dashboard-work-row")).toHaveCount(20);
  await scroller.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  const scrollTop = await scroller.evaluate((element) => element.scrollTop);
  expect(scrollTop).toBeGreaterThan(0);
  const trigger = page.getByRole("button", {
    name: "Working session 20",
    exact: true,
  });
  await trigger.click();
  const panel = page.getByTestId("dashboard-conversation-panel");
  await expect(panel).toContainText("Original reply for research-session");
  const requests = api.requests.length;
  response.rows[19].open_target = {
    ...sessionTarget("review-session"),
    agent_run_id: "replacement-run",
  };
  response.rows[19].preview!.text = "A replacement run has arrived";
  await expect.poll(() => api.requests.length).toBeGreaterThan(requests);
  await expect(panel).toHaveAttribute("data-session-id", "research-session");
  await expect(panel.getByTestId("dashboard-selected-run")).toContainText(
    "agent-research-session",
  );
  expect(await scroller.evaluate((element) => element.scrollTop)).toBe(
    scrollTop,
  );
  await panel
    .getByRole("button", { name: "Close conversation", exact: true })
    .click();
  await expect(trigger).toBeFocused();
  expect(await scroller.evaluate((element) => element.scrollTop)).toBe(
    scrollTop,
  );
  await trigger.click();
  await expect(panel).toHaveAttribute("data-session-id", "review-session");
  await expect(panel.getByTestId("dashboard-selected-run")).toContainText(
    "replacement-run",
  );
});

test("mobile uses the full exact conversation route and translated controls fit 375px", async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 812 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await mockDashboard(page);
  await page.goto("/?locale=zh-Hans");
  await expect(
    page.getByRole("table", { name: "智能体工作监控" }),
  ).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(
    375,
  );
  await expect(page.getByTestId("dashboard-page")).not.toContainText(
    "sessionDashboard.",
  );
  const filters = page.getByRole("button", { name: "筛选", exact: true });
  await filters.click();
  const panel = page.getByRole("dialog", { name: "筛选", exact: true });
  await expect(panel.getByRole("combobox")).toHaveCount(4);
  const filterBox = (await panel.boundingBox())!;
  expect(filterBox.x).toBeGreaterThanOrEqual(0);
  expect(filterBox.x + filterBox.width).toBeLessThanOrEqual(375);
  await page.keyboard.press("Escape");
  await expect(filters).toBeFocused();
  await page.getByRole("button", { name: "Ship release", exact: true }).click();
  await expect(page.getByTestId("destination")).toHaveText(
    "/workspaces/visual-workspace?session_id=research-session",
  );
  await expect(page.getByTestId("dashboard-conversation-panel")).toHaveCount(0);
  await expect(page.getByTestId("dashboard-work-row").first()).toHaveCSS(
    "animation-name",
    "none",
  );
});

test("light and dark themes preserve readable mode labels and table geometry", async ({
  page,
}) => {
  await mockDashboard(page);
  await page.goto("/");
  const table = page.getByRole("table");
  await expect(table).toBeVisible();
  const light = await page
    .getByTestId("dashboard-work-row")
    .first()
    .evaluate((row) => getComputedStyle(row).backgroundColor);
  const width = (await table.boundingBox())!.width;
  await page.getByRole("button", { name: "Dark", exact: true }).click();
  await expect
    .poll(() =>
      page
        .getByTestId("dashboard-work-row")
        .first()
        .evaluate((row) => getComputedStyle(row).backgroundColor),
    )
    .not.toBe(light);
  expect((await table.boundingBox())!.width).toBe(width);
  await expect(
    page.getByRole("button", { name: "Ship release", exact: true }),
  ).toBeVisible();
});

test("records a bounded first page and one refresh for 100 works / 300 sessions / 20 active sessions", async ({
  page,
}, testInfo) => {
  const all = Array.from({ length: 100 }, (_, index) => {
    const row = workRow("task", `Performance work ${index + 1}`);
    return {
      ...row,
      work: { ...row.work, id: `performance-${index}` },
      child_count: 3,
      active_session_count: index < 20 ? 1 : 0,
      active_run_count: index < 20 ? 1 : 0,
      raw_status: index < 20 ? "running" : "succeeded",
      status_group: index < 20 ? ("running" as const) : ("finished" as const),
    };
  });
  const response = dashboardResponse(all.slice(0, 20));
  response.total = 20;
  response.latest_total = 20;
  response.counts = {
    ...response.counts,
    work_count: 20,
    active_session_count: 20,
    active_run_count: 20,
    running: 20,
    current: 20,
    all: 100,
    finished: 80,
  };
  await page.addInitScript(() => {
    const timings: { firstRowsAt?: number; refreshAt?: number } = {};
    Object.assign(window, { dashboardTimings: timings });
    new MutationObserver(() => {
      if (
        !timings.firstRowsAt &&
        document.querySelectorAll('[data-testid="dashboard-work-row"]')
          .length === 20
      )
        timings.firstRowsAt = performance.now();
      if (
        !timings.refreshAt &&
        document
          .querySelector(".session-dashboard__reply-text")
          ?.textContent?.includes("Performance refresh")
      )
        timings.refreshAt = performance.now();
    }).observe(document, {
      subtree: true,
      childList: true,
      characterData: true,
    });
  });
  const api = await mockDashboard(page, () => response);
  await page.goto("/");
  await expect(page.getByTestId("dashboard-work-row")).toHaveCount(20);
  expect(api.requests).toHaveLength(1);
  response.rows[0].preview!.text = "Performance refresh reply";
  await expect(
    page.getByRole("button", { name: /Performance refresh reply/ }),
  ).toBeVisible();
  expect(api.requests).toHaveLength(2);
  const timings = await page.evaluate(() => {
    const metrics = (
      window as unknown as {
        dashboardTimings: { firstRowsAt: number; refreshAt: number };
      }
    ).dashboardTimings;
    const requests = performance
      .getEntriesByType("resource")
      .filter((entry) =>
        entry.name.endsWith("/api/dashboard/query"),
      ) as PerformanceResourceTiming[];
    return {
      ...metrics,
      firstResponseToDOMMs: metrics.firstRowsAt - requests[0].responseEnd,
      refreshResponseToDOMMs: metrics.refreshAt - requests[1].responseEnd,
    };
  });
  const report = {
    workload: { works: 100, sessions: 300, activeSessions: 20 },
    renderedRows: 20,
    firstPageRequests: 1,
    refreshRequests: 1,
    contextRequests: api.contexts.length,
    payloadBytes: Buffer.byteLength(JSON.stringify(response)),
    ...timings,
  };
  console.log("DASHBOARD_BROWSER_PERFORMANCE", JSON.stringify(report));
  await testInfo.attach("synthetic-dashboard-performance.json", {
    body: JSON.stringify(report, null, 2),
    contentType: "application/json",
  });
  await page.screenshot({ path: `${testInfo.outputDir}/dashboard-light.png` });
});
