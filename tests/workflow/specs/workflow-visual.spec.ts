import { expect, test, type Locator, type Page } from "@playwright/test";
import {
  mockWorkflowConversations,
  mockWorkflowFileContext,
} from "../support/conversation-api";

async function expandDeliverables(page: Page) {
  const toggle = page.getByTestId("workflow-deliverables-toggle");
  if ((await toggle.getAttribute("aria-expanded")) === "false")
    await toggle.click();
}

function canvas(page: Page) {
  return page.getByTestId("visual-canvas");
}

async function viewportTransform(page: Page) {
  return canvas(page).locator(".react-flow__viewport").getAttribute("style");
}

async function styleOf(locator: Locator) {
  return locator.evaluate((element) => {
    const style = getComputedStyle(element);
    return {
      background: style.backgroundColor,
      color: style.color,
      radius: parseFloat(style.borderRadius),
      animation: style.animationName,
      beforeAnimation: getComputedStyle(element, "::before").animationName,
      afterAnimation: getComputedStyle(element, "::after").animationName,
    };
  });
}

function luminance(color: string) {
  const channels = color
    .match(/[\d.]+/g)!
    .slice(0, 3)
    .map(Number)
    .map((value) => {
      const normalized = value / 255;
      return normalized <= 0.04045
        ? normalized / 12.92
        : ((normalized + 0.055) / 1.055) ** 2.4;
    });
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrast(a: string, b: string) {
  const first = luminance(a);
  const second = luminance(b);
  return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  // The fixture is intentionally API-free. Fail closed if it starts requesting
  // a real backend; fonts are irrelevant to these deterministic style checks.
  await page.route("**/api/**", (route) => route.abort());
  await page.route("https://fonts.googleapis.com/**", (route) => route.abort());
  await mockWorkflowFileContext(page);
});

test("uses one quiet background and readable cards in both themes without resetting the graph", async ({
  page,
}) => {
  await page.goto("/visual.html");
  const node = page.getByTestId("workflow-node-plan");
  await expect(node).toBeVisible();
  await expect(canvas(page).locator(".react-flow__background")).toHaveCount(1);
  await expect(canvas(page).locator(".react-flow__pane")).toHaveCSS(
    "background-image",
    "none",
  );
  await expect(node).toHaveCSS("width", "248px");
  const light = await styleOf(node);
  expect(luminance(light.background)).toBeGreaterThan(0.8);
  expect(contrast(light.color, light.background)).toBeGreaterThanOrEqual(4.5);
  expect(light.radius).toBeGreaterThanOrEqual(4);
  expect(light.radius).toBeLessThanOrEqual(6);
  await expect(page.getByTestId("workflow-node-start")).toHaveText("Start");
  const transform = await viewportTransform(page);
  await page.getByRole("button", { name: "Dark", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect
    .poll(async () => luminance((await styleOf(node)).background))
    .toBeLessThan(0.08);
  const dark = await styleOf(node);
  expect(luminance(dark.background)).toBeLessThan(0.08);
  expect(contrast(dark.color, dark.background)).toBeGreaterThanOrEqual(4.5);
  expect(await viewportTransform(page)).toBe(transform);
  await expect(canvas(page).locator(".react-flow")).toHaveClass(/dark/);
  await page.getByRole("button", { name: "Light", exact: true }).click();
  await expect(node).toHaveCSS("background-color", light.background);
  expect(await viewportTransform(page)).toBe(transform);
});

test("selection stays static and configuration switches in the same nonmodal inset frame", async ({
  page,
}) => {
  await page.goto("/visual.html");
  const plan = page.getByTestId("workflow-node-plan");
  await expect(plan).toBeVisible();
  const transform = await viewportTransform(page);
  const canvasBox = await canvas(page).boundingBox();
  await plan.click();
  const frame = page.locator(".workflow-configuration-frame");
  await expect(frame).toBeVisible();
  await expect(frame).not.toHaveAttribute("aria-modal", "true");
  await expect(frame.getByRole("heading")).toHaveText("Plan the change");
  const frameElement = await frame.elementHandle();
  const box = await frame.boundingBox();
  expect(box!.x).toBeGreaterThan(900);
  expect(box!.x + box!.width).toBeLessThan(1600);
  const toolbarBox = await page.locator("header").boundingBox();
  expect(box!.y).toBeGreaterThan(toolbarBox!.y + toolbarBox!.height);
  expect(box!.y + box!.height).toBeLessThan(900);
  expect(await viewportTransform(page)).toBe(transform);
  expect(await canvas(page).boundingBox()).toEqual(canvasBox);
  const selectedStyle = await styleOf(plan);
  expect(selectedStyle.animation).toBe("none");
  expect(selectedStyle.beforeAnimation).toBe("none");
  expect(selectedStyle.afterAnimation).toBe("none");

  await frame.getByRole("textbox", { name: "Task name" }).fill("Keep my draft");
  await expect(plan).toContainText("Keep my draft");
  await expect(
    canvas(page).locator('.react-flow__node[data-id="plan"]'),
  ).toHaveClass(/selected/);
  const scroll = frame.locator("[data-object-content-key]");
  await scroll.evaluate((element) => {
    element.scrollTop = 240;
  });
  await page
    .getByTestId("workflow-node-build")
    .click({ position: { x: 30, y: 30 } });
  await expect(frame.getByRole("heading")).toHaveText("Implement the change");
  expect(
    await frameElement!.evaluate(
      (element) =>
        element === document.querySelector(".workflow-configuration-frame"),
    ),
  ).toBe(true);
  expect(await viewportTransform(page)).toBe(transform);
  await plan.click();
  await expect(frame.getByRole("textbox")).toHaveValue("Keep my draft");
  await expect
    .poll(() => scroll.evaluate((element) => element.scrollTop))
    .toBe(240);
  // Keyboard focus can leave the nonmodal panel; it is not a trapped drawer.
  await page.getByRole("button", { name: "Dark", exact: true }).focus();
  await expect(
    page.getByRole("button", { name: "Dark", exact: true }),
  ).toBeFocused();
  await page.getByRole("button", { name: "Dark", exact: true }).press("Enter");
  await expect(frame.getByRole("textbox")).toHaveValue("Keep my draft");
  expect(await viewportTransform(page)).toBe(transform);
});

test("only running edges animate and terminal updates retain viewport and selection", async ({
  page,
}) => {
  await page.goto("/visual.html?mode=run");
  const node = page.getByTestId("workflow-run-node-build");
  await expect(node).toBeVisible();
  await expect(canvas(page).locator(".react-flow__background")).toHaveCount(1);
  await expect(node).toHaveCSS("width", "248px");
  await expect
    .poll(async () =>
      canvas(page).locator(".workflow-edge-beam-running").count(),
    )
    .toBeGreaterThan(0);
  const runningBeam = canvas(page)
    .locator(".workflow-edge-beam-running")
    .first();
  await expect(runningBeam).not.toHaveCSS("animation-name", "none");
  expect((await styleOf(node)).beforeAnimation).toBe("none");
  await node.click();
  const transform = await viewportTransform(page);
  await page.getByRole("button", { name: "Complete run" }).click();
  await expect(canvas(page).locator(".workflow-edge-beam-running")).toHaveCount(
    0,
  );
  await expect(node).toContainText("Succeeded");
  await expect(
    canvas(page).locator('.react-flow__node[data-id="build"]'),
  ).toHaveClass(/selected/);
  expect(await viewportTransform(page)).toBe(transform);
  await page.getByRole("button", { name: "Dark", exact: true }).click();
  await expect
    .poll(async () => luminance((await styleOf(node)).background))
    .toBeLessThan(0.08);
  expect(
    contrast((await styleOf(node)).color, (await styleOf(node)).background),
  ).toBeGreaterThanOrEqual(4.5);
  expect(await viewportTransform(page)).toBe(transform);
});

test("System theme follows the OS and reduced motion removes canvas and frame effects", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "reduce", colorScheme: "light" });
  await page.goto("/visual.html");
  await page.getByRole("button", { name: "System", exact: true }).click();
  await page.emulateMedia({ colorScheme: "dark" });
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  const plan = page.getByTestId("workflow-node-plan");
  await plan.click();
  await expect(page.locator(".workflow-configuration-frame")).toHaveCSS(
    "animation-name",
    "none",
  );
  await expect(page.locator(".workflow-side-panel-content")).toHaveCSS(
    "animation-name",
    "none",
  );
  await page.goto("/visual.html?mode=run");
  await expect(page.getByTestId("workflow-run-node-build")).toBeVisible();
  await expect(
    canvas(page).locator(".workflow-edge-beam-running").first(),
  ).toHaveCSS("animation-name", "none");
});

test("reused results stay read-only, identify every source iteration, and preserve the canvas", async ({
  page,
}) => {
  await page.goto("/visual.html?mode=run");
  const plan = page.getByTestId("workflow-run-node-plan");
  await expect(plan).toBeVisible();
  await plan.click();
  const transform = await viewportTransform(page);
  await page
    .getByRole("button", { name: "Rework with reused results" })
    .click();
  await expect(plan).toContainText("Reused");
  await plan.click();
  expect(await viewportTransform(page)).toBe(transform);
  await expect(
    canvas(page).locator('.react-flow__node[data-id="plan"]'),
  ).toHaveClass(/selected/);
  await expect(canvas(page).locator(".workflow-edge-beam-running")).toHaveCount(
    0,
  );

  const panel = page.getByTestId("workflow-run-node-details");
  await expect(panel).toContainText("This Node did not run again");
  const sources = panel.getByTestId("workflow-run-reused-result");
  await expect(sources).toHaveCount(2);
  await expect(sources.nth(0)).toContainText("Source iteration 0");
  await expect(sources.nth(0)).toContainText("Earlier plan, first iteration");
  await expect(sources.nth(1)).toContainText("Source iteration 1");
  await expect(sources.nth(1)).toContainText("Earlier plan, second iteration");
  await sources.nth(1).getByText("Result source", { exact: true }).click();
  await expect(sources.nth(1)).toContainText("earlier-run");
  await expect(sources.nth(1)).toContainText("plan-source-1");
  await expect(
    panel.getByRole("button", {
      name: /Approve|Reject|Retry|Open full session/,
    }),
  ).toHaveCount(0);
  await expect(panel.getByRole("link")).toHaveCount(0);
  await expect(
    panel.getByText("Technical details", { exact: true }),
  ).toHaveCount(0);
  const facts = JSON.parse(
    (await page.getByTestId("runtime-facts").textContent())!,
  );
  expect(facts).toMatchObject({
    completed: 1,
    freshSteps: 1,
    progressPercent: 100,
    reused: 2,
    skipped: 1,
    tokens: 42,
    actualExecutionIds: ["fresh-build-execution"],
    reusedExecution: null,
  });
  expect(
    Object.values(facts.reusedActions).every((allowed) => allowed === false),
  ).toBe(true);

  await page.getByTestId("workflow-run-node-end").click();
  await expect(panel).toContainText("Skipped");
  await expect(panel.getByTestId("workflow-run-reused-result")).toHaveCount(0);
  await expect(panel).toContainText("has not executed");
  await expect(
    panel.getByText("Technical details", { exact: true }),
  ).toHaveCount(0);
  expect(await viewportTransform(page)).toBe(transform);
});

test("explicit fit animates smoothly and reacts to a reduced-motion preference change", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.goto("/visual.html");
  await expect(page.getByTestId("workflow-node-plan")).toBeVisible();
  const original = await viewportTransform(page);

  async function fitFrames() {
    await canvas(page).locator(".react-flow__controls-zoomout").click();
    await expect.poll(() => viewportTransform(page)).not.toBe(original);
    return canvas(page).evaluate(async (element) => {
      const viewport = element.querySelector<HTMLElement>(
        ".react-flow__viewport",
      )!;
      const fit = element.querySelector<HTMLButtonElement>(
        ".react-flow__controls-fitview",
      )!;
      const positions = new Set<string>();
      fit.click();
      const started = performance.now();
      while (performance.now() - started < 360) {
        await new Promise(requestAnimationFrame);
        positions.add(viewport.style.transform);
      }
      return positions.size;
    });
  }

  expect(await fitFrames()).toBeGreaterThan(3);
  await page.emulateMedia({ reducedMotion: "reduce" });
  expect(await fitFrames()).toBeLessThanOrEqual(2);
});

test("split progress retains failures, independent work, and the precise execution sessions", async ({
  page,
}) => {
  await mockWorkflowConversations(page);
  const mutations: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/")) {
      mutations.push(request.url());
    }
  });
  await page.goto("/visual.html?mode=file-run");
  const splitter = page.getByTestId("workflow-run-node-plan");
  await expect(splitter).toContainText("1 / 4 completed");
  await splitter.click();
  await expandDeliverables(page);
  const batch = page.getByTestId("workflow-split-batch");
  await expect(batch.getByTestId("workflow-split-progress")).toContainText(
    "1 / 4 completed",
  );
  await expect(batch.getByTestId("workflow-split-progress")).toContainText(
    "1 failed",
  );
  const stages = batch.getByTestId("workflow-split-stage");
  await expect(stages).toHaveCount(2);
  await expect(stages.nth(0).getByTestId("workflow-split-subtask")).toHaveCount(
    3,
  );
  await expect(stages.nth(1)).toContainText(
    "Depends on: Review the source material",
  );
  const tasks = batch.getByTestId("workflow-split-subtask");
  await expect(tasks.nth(1)).toContainText("Failed");
  await expect(tasks.nth(1)).toContainText("Source material was removed");
  await expect(tasks.nth(2)).toContainText("Running");
  await expect(tasks.nth(3)).toContainText("Dependency not met");
  for (const [index, session] of [
    "research-session",
    "review-session",
    "analysis-session",
  ].entries()) {
    await tasks
      .nth(index)
      .getByRole("button", { name: "Open execution session" })
      .click();
    await expect(
      page.getByTestId("workflow-conversation-panel"),
    ).toHaveAttribute("data-session-id", session);
    await page
      .getByRole("button", { name: "Back to work list", exact: true })
      .click();
    await expandDeliverables(page);
  }
  await tasks
    .nth(3)
    .getByRole("button", { name: "Open execution session" })
    .click();
  await expect(page.getByTestId("workflow-conversation-waiting")).toBeVisible();
  await expect(page.getByTestId("workflow-conversation-panel")).toHaveAttribute(
    "data-session-id",
    "",
  );
  await page
    .getByRole("button", { name: "Back to work list", exact: true })
    .click();
  await expandDeliverables(page);
  await expect(
    batch.getByRole("button", { name: "Open main session" }),
  ).toBeVisible();
  await batch.getByRole("button", { name: "Open main session" }).click();
  await expect(page.getByTestId("workflow-conversation-panel")).toHaveAttribute(
    "data-session-id",
    "main-session",
  );
  await expect(page.getByTestId("workflow-conversation-panel")).toContainText(
    "Original reply for main-session",
  );
  await page
    .getByRole("button", { name: "Back to work list", exact: true })
    .click();
  await expandDeliverables(page);
  expect(mutations).toEqual([]);

  await page.getByRole("button", { name: "Await dispatch" }).click();
  await expect(splitter).toContainText("Waiting for main Agent dispatch");
  await expect(batch).toContainText("Awaiting assignment");
  await expect(batch).toContainText("No executor assigned");
  await expect(
    batch.getByRole("button", { name: "Summarize completed work" }),
  ).toHaveCount(0);
  expect(mutations).toEqual([]);
});

test("workflow file previews fetch current content lazily and safely encode download paths", async ({
  page,
}) => {
  const paths: string[] = [];
  const filePath = "deliverables/research report.md";
  await page.route(
    "**/api/workflow-runs/visual-run/files/content?*",
    async (route) => {
      const path = new URL(route.request().url()).searchParams.get("path")!;
      paths.push(path);
      await route.fulfill({
        json: {
          success: true,
          data: {
            run_id: "visual-run",
            workspace_id: "visual-workspace",
            kind: "text",
            path,
            name: "research report.md",
            mime_type: "text/markdown",
            language: "markdown",
            content: "# Research delivered\n\nCurrent file contents.",
            raw_url: null,
            size_bytes: 51,
            truncated: false,
          },
        },
      });
    },
  );
  await page.goto("/visual.html?mode=file-run");
  await page.getByTestId("workflow-run-node-plan").click();
  await expandDeliverables(page);
  const task = page.getByTestId("workflow-split-subtask").first();
  await expect(task).toContainText(filePath);
  expect(paths).toEqual([]);
  await task.getByRole("button", { name: filePath, exact: true }).click();
  const preview = page.getByTestId("workflow-file-inline-preview");
  await expect(
    preview.getByRole("heading", { name: "Research delivered" }),
  ).toBeVisible();
  await expect(preview).toContainText("Preview and download the current file");
  expect(paths).toEqual([filePath]);
  const download = preview.getByRole("link", { name: "Download file" });
  const href = await download.getAttribute("href");
  const rawUrl = new URL(href!, page.url());
  expect(rawUrl.pathname).toBe("/api/workflow-runs/visual-run/files/raw");
  expect(rawUrl.searchParams.get("path")).toBe(filePath);
  expect(href).not.toContain(" ");
  await expect(download).toHaveAttribute("download", "");
  await preview.getByRole("button", { name: "Back", exact: true }).click();
  await expect(preview).toHaveCount(0);
  await expect(task).toContainText(filePath);
});

test("partial summaries are explicit, wait for current work, and retry one idempotent submission", async ({
  page,
}) => {
  const bodies: Record<string, unknown>[] = [];
  await page.route(
    "**/api/workflow-management/instances/visual-instance/submit",
    async (route) => {
      bodies.push(route.request().postDataJSON());
      if (bodies.length === 1) {
        await route.fulfill({
          status: 503,
          json: { success: false, message: "Temporary submission failure" },
        });
      } else {
        await route.fulfill({
          status: 202,
          json: {
            success: true,
            data: {
              request_id: bodies[0].request_id,
              project_id: "visual-project",
              issue_id: "visual-issue",
              instance_id: "visual-instance",
              run_id: "accepted-partial-run",
              phase: "waiting_for_source",
            },
          },
        });
      }
    },
  );
  await page.goto("/visual.html?mode=file-run");
  await page.getByTestId("workflow-run-node-plan").click();
  await expandDeliverables(page);
  const action = page.getByTestId("workflow-partial-summary-action");
  const submit = action.getByRole("button", {
    name: "Summarize completed work",
  });
  await expect(submit).toBeEnabled();
  expect(bodies).toEqual([]);
  await submit.click();
  await expect(action).toContainText("Temporary submission failure");
  expect(bodies).toHaveLength(1);
  expect(bodies[0]).toMatchObject({
    action: "rework",
    source_run_id: "visual-run",
    source_batch_id: "visual-batch",
    delivery_intent: "partial",
    scope: { type: "from_nodes", node_ids: ["end"] },
    active_policy: "after_current",
    material_paths: [".vibe-kanban/workflows/visual-run/tasks.json"],
  });
  expect(bodies[0].request_id).toEqual(expect.any(String));
  await submit.click();
  await expect(action).toContainText("Partial summary accepted");
  await expect(
    action.getByRole("button", { name: "Open partial summary" }),
  ).toBeVisible();
  expect(bodies).toHaveLength(2);
  expect(bodies[1]).toEqual(bodies[0]);
  await expect(submit).toHaveCount(0);
  await expect(page.getByTestId("workflow-split-subtask").nth(1)).toContainText(
    "Failed",
  );
  await expect(page.getByTestId("workflow-split-subtask").nth(2)).toContainText(
    "Running",
  );
});

test("a missing workflow file keeps its reference and can be retried without resubmitting work", async ({
  page,
}) => {
  const filePath = "deliverables/research report.md";
  let reads = 0;
  const mutations: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/api/") && request.method() !== "GET") {
      mutations.push(request.url());
    }
  });
  await page.route(
    "**/api/workflow-runs/visual-run/files/content?*",
    async (route) => {
      reads += 1;
      if (reads === 1) {
        await route.fulfill({
          status: 404,
          json: { success: false, message: "Referenced file is missing" },
        });
      } else {
        await route.fulfill({
          json: {
            success: true,
            data: {
              run_id: "visual-run",
              workspace_id: "visual-workspace",
              kind: "text",
              path: filePath,
              name: "research report.md",
              mime_type: "text/markdown",
              language: "markdown",
              content: "# Restored current file",
              raw_url: null,
              size_bytes: 600000,
              truncated: true,
            },
          },
        });
      }
    },
  );
  await page.goto("/visual.html?mode=file-run");
  await page.getByTestId("workflow-run-node-plan").click();
  await expandDeliverables(page);
  await page
    .getByTestId("workflow-split-subtask")
    .first()
    .getByRole("button", { name: filePath, exact: true })
    .click();
  const preview = page.getByTestId("workflow-file-inline-preview");
  await expect(preview.getByRole("alert")).toContainText(
    "Could not read this workflow file",
  );
  await expect(preview.getByRole("alert")).toContainText(
    "Referenced file is missing",
  );
  expect(reads).toBe(1);
  await preview.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(
    preview.getByRole("heading", { name: "Restored current file" }),
  ).toBeVisible();
  await expect(preview).toContainText(
    "Preview truncated. Download the file to read the full content.",
  );
  expect(reads).toBe(2);
  expect(mutations).toEqual([]);
  await preview.getByRole("button", { name: "Back", exact: true }).click();
  const batch = page.getByTestId("workflow-split-batch");
  await expect(batch.getByTestId("workflow-split-progress")).toContainText(
    "1 / 4 completed",
  );
  await expect(
    batch.getByTestId("workflow-split-subtask").first(),
  ).toContainText(filePath);
});

test("partial delivery keeps original missing work separate from referenced files and Agent changes", async ({
  page,
}) => {
  await page.route(
    "**/api/local/v1/workflow-runs/partial-run/file-changes",
    async (route) => {
      await route.fulfill({
        json: {
          files: [{ path: "deliverables/final.md", change_type: "modified" }],
          collection_status: "partial",
          reasons: [],
        },
      });
    },
  );
  await page.goto("/visual.html?mode=file-run");
  await page.getByRole("button", { name: "Show partial delivery" }).click();
  await page
    .getByRole("button", { name: "Execution result", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  const delivery = dialog.getByTestId("workflow-delivery-completeness");
  await expect(delivery).toContainText("Partial delivery");
  await expect(delivery.getByTestId("workflow-delivery-missing")).toContainText(
    "Review the source material",
  );
  await expect(delivery.getByTestId("workflow-delivery-missing")).toContainText(
    "Source material was removed",
  );
  await expect(
    delivery.getByRole("button", { name: "Open source run" }),
  ).toBeVisible();
  await expect(dialog.getByTestId("workflow-delivery-files")).toContainText(
    "deliverables/research report.md",
  );
  const evidence = dialog.getByTestId("workflow-agent-file-changes");
  await expect(evidence).toContainText("deliverables/final.md");
  await expect(evidence).not.toContainText("deliverables/research report.md");
  await expect(evidence).toContainText("This list may be incomplete");
});

test("the canvas float renders the original conversation and preserves drafts across rapid close and work switches", async ({
  page,
}) => {
  const api = await mockWorkflowConversations(page, {
    holdFirstDraftWrite: true,
  });
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/visual.html?mode=file-run");
  const node = page.getByTestId("workflow-run-node-build");
  await expect(node).toBeVisible();
  const transform = await viewportTransform(page);
  const canvasBox = await canvas(page).boundingBox();
  await node.click();
  const panel = page.getByTestId("workflow-conversation-panel");
  await expect(panel).toHaveAttribute("data-session-id", "analysis-session");
  await expect(panel).toContainText("Original reply for analysis-session");
  await expect(
    page.getByTestId("workflow-deliverables-toggle"),
  ).toHaveAttribute("aria-expanded", "false");
  const editor = panel.locator('[contenteditable="true"]').first();
  await expect(editor).toBeVisible();
  await editor.fill("Draft kept after closing immediately");
  await panel
    .getByRole("button", { name: "Close Node details", exact: true })
    .click();
  await expect(panel).toHaveCount(0);
  await expect.poll(() => api.writes.includes("analysis-session")).toBe(true);
  await node.click();
  await expect(editor).toHaveText("Draft kept after closing immediately");
  expect(api.drafts.has("analysis-session")).toBe(false);
  const latestDraft = "Draft edited while the previous save is pending";
  await editor.fill(latestDraft);
  api.releaseDraftWrite();
  await expect.poll(() => api.drafts.has("analysis-session")).toBe(true);
  await expect(editor).toHaveText(latestDraft);
  const selector = page.getByTestId("workflow-conversation-work-selector");
  await selector.selectOption("subtask:visual-batch:research");
  await expect(panel).toHaveAttribute("data-session-id", "research-session");
  await expect(panel).toContainText("Original reply for research-session");
  await expect(panel).not.toContainText("Original reply for analysis-session");
  await editor.fill("Independent research draft");
  await selector.selectOption("subtask:visual-batch:independent");
  await expect(editor).toHaveText(latestDraft);
  await selector.selectOption("subtask:visual-batch:research");
  await expect(editor).toHaveText("Independent research draft");
  expect(await viewportTransform(page)).toBe(transform);
  expect(await canvas(page).boundingBox()).toEqual(canvasBox);
  expect(api.requests.filter((request) => request.startsWith("POST "))).toEqual(
    [],
  );
  expect(errors).toEqual([]);
});

test("remote persisted drafts remain isolated when two Hosts have the same Session ID", async ({
  page,
}) => {
  const editor = page.getByRole("textbox", { name: "Session draft" });
  const openHost = async (hostId: string) => {
    await page.goto(
      `/visual.html?mode=scratch-scope&runtime=remote&host=${hostId}`,
    );
    await expect(editor).toBeVisible();
  };

  await openHost("first-host");
  await editor.fill("First host draft");
  await openHost("second-host");
  await expect(editor).toBeEmpty();
  await editor.fill("Second host draft");
  await openHost("first-host");
  await expect(editor).toHaveValue("First host draft");
  await page.getByRole("button", { name: "Delete draft" }).click();
  await expect(editor).toBeEmpty();
  await openHost("second-host");
  await expect(editor).toHaveValue("Second host draft");
});

test("a send acknowledged after close and reopen preserves the replacement session draft", async ({
  page,
}) => {
  const api = await mockWorkflowConversations(page);
  let submitted = false;
  let releaseSend = () => {};
  await page.route(
    "**/api/sessions/analysis-session/follow-up",
    async (route) => {
      submitted = true;
      await new Promise<void>((resolve) => {
        releaseSend = resolve;
      });
      await route.fulfill({ json: { success: true, data: null } });
    },
  );
  await page.goto("/visual.html?mode=file-run");
  const node = page.getByTestId("workflow-run-node-build");
  await node.click();
  const panel = page.getByTestId("workflow-conversation-panel");
  const editor = panel.locator('[contenteditable="true"]').first();
  await expect(editor).toBeVisible();
  await editor.fill("The submitted draft");
  await panel.getByRole("button", { name: "Send", exact: true }).click();
  await expect.poll(() => submitted).toBe(true);
  await panel
    .getByRole("button", { name: "Close Node details", exact: true })
    .click();
  await expect(panel).toHaveCount(0);
  await node.click();
  await expect(editor).toBeVisible();
  const replacement = "A new draft written after reopening the same Session";
  await editor.fill(replacement);
  await expect
    .poll(() => api.drafts.get("analysis-session"))
    .toMatchObject({
      payload: { data: { message: replacement } },
    });
  const response = page.waitForResponse((candidate) =>
    candidate.url().endsWith("/sessions/analysis-session/follow-up"),
  );
  releaseSend();
  await (await response).finished();
  // Deliver the response through the browser's event/render cycle before reopening.
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  await expect(editor).toHaveText(replacement);
  await panel
    .getByRole("button", { name: "Close Node details", exact: true })
    .click();
  await expect(panel).toHaveCount(0);
  await node.click();
  await expect(editor).toHaveText(replacement);
  expect(
    api.requests.filter((request) => request.startsWith("DELETE ")),
  ).toEqual([]);
});

test("a waiting conversation stays inset, does not start work and remains open on canvas interaction", async ({
  page,
}) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/api/") && request.method() !== "GET")
      writes.push(request.url());
  });
  await page.goto("/visual.html?mode=run");
  const node = page.getByTestId("workflow-run-node-plan");
  await expect(node).toBeVisible();
  const transform = await viewportTransform(page);
  const canvasBox = await canvas(page).boundingBox();
  await node.click();
  const panel = page.getByTestId("workflow-conversation-panel");
  await expect(page.getByTestId("workflow-conversation-waiting")).toBeVisible();
  await expect
    .poll(async () => {
      const bounds = (await panel.boundingBox())!;
      return Math.round(
        canvasBox!.x + canvasBox!.width - bounds.x - bounds.width,
      );
    })
    .toBe(24);
  const bounds = (await panel.boundingBox())!;
  expect(bounds.width).toBe(440);
  expect(canvasBox!.x + canvasBox!.width - bounds.x - bounds.width).toBeCloseTo(
    24,
    0,
  );
  expect(bounds.y - canvasBox!.y).toBeCloseTo(24, 0);
  await expect(panel).not.toHaveAttribute("aria-modal", "true");
  await canvas(page)
    .locator(".react-flow__pane")
    .click({ position: { x: 20, y: 20 } });
  await expect(panel).toBeVisible();
  expect(await viewportTransform(page)).toBe(transform);
  expect(await canvas(page).boundingBox()).toEqual(canvasBox);
  await panel
    .getByRole("button", { name: "Close Node details", exact: true })
    .focus();
  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  expect(writes).toEqual([]);
});

test("inline file preview keeps the original editor mounted and Escape returns before closing the float", async ({
  page,
}, testInfo) => {
  await mockWorkflowConversations(page);
  const filePath = "deliverables/research report.md";
  const reads: string[] = [];
  await page.route(
    "**/api/workflow-runs/visual-run/files/content?*",
    async (route) => {
      reads.push(new URL(route.request().url()).searchParams.get("path")!);
      await route.fulfill({
        json: {
          success: true,
          data: {
            run_id: "visual-run",
            workspace_id: "visual-workspace",
            kind: "text",
            path: filePath,
            name: "research report.md",
            mime_type: "text/markdown",
            language: "markdown",
            content:
              "# Original file viewer\n\nThe real shared Markdown viewer.",
            raw_url: null,
            size_bytes: 67,
            truncated: false,
          },
        },
      });
    },
  );
  await page.goto("/visual.html?mode=file-run");
  await page.getByTestId("workflow-run-node-build").click();
  const panel = page.getByTestId("workflow-conversation-panel");
  const editor = panel.locator('[contenteditable="true"]').first();
  await expect(editor).toBeVisible();
  await editor.fill("Draft retained while reading the file");
  const editorHandle = await editor.elementHandle();
  await expandDeliverables(page);
  await expect(page.getByTestId("workflow-deliverables-toggle")).toContainText(
    "Deliverables 1",
  );
  expect(reads).toEqual([]);
  await page
    .getByTestId("workflow-deliverables")
    .getByRole("button", { name: filePath, exact: true })
    .first()
    .click();
  const preview = page.getByTestId("workflow-file-inline-preview");
  await expect(
    preview.getByRole("heading", { name: "Original file viewer" }),
  ).toBeVisible();
  await expect(editor).toBeHidden();
  expect(await editorHandle!.evaluate((element) => element.isConnected)).toBe(
    true,
  );
  await expect(
    preview.getByRole("button", { name: "Back", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(preview).toHaveCount(0);
  await expect(panel).toBeVisible();
  await expect(editor).toBeVisible();
  await expect(editor).toHaveText("Draft retained while reading the file");
  await expect(
    page
      .getByTestId("workflow-deliverables")
      .getByRole("button", { name: filePath, exact: true })
      .first(),
  ).toBeFocused();
  expect(await editorHandle!.evaluate((element) => element.isConnected)).toBe(
    true,
  );
  await expect(
    page.getByTestId("workflow-deliverables-toggle"),
  ).toHaveAttribute("aria-expanded", "true");
  expect(reads).toEqual([filePath]);

  await page.setViewportSize({ width: 620, height: 680 });
  await expect
    .poll(async () => {
      const bounds = (await panel.boundingBox())!;
      return bounds.x >= 0 && bounds.x + bounds.width <= 620;
    })
    .toBe(true);
  const editorBounds = (await editor.boundingBox())!;
  expect(editorBounds.y + editorBounds.height).toBeLessThanOrEqual(680);
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(620);
  await page.screenshot({
    path: testInfo.outputPath("workflow-conversation-narrow.png"),
  });
});

test("a cloned completed worker opens its exact original Session read-only even without file references", async ({
  page,
}) => {
  const api = await mockWorkflowConversations(page);
  await page.route(
    "**/api/workflow-runs/visual-run/file-context**",
    async (route) => {
      const id = new URL(route.request().url()).searchParams.get(
        "node_execution_id",
      );
      if (id !== "research-execution") return route.fallback();
      await route.fulfill({
        json: {
          success: true,
          data: {
            run_id: "visual-run",
            workspace_id: "visual-workspace",
            node_execution_id: id,
            execution: {
              run_id: "original-run",
              workspace_id: "visual-workspace",
              node_id: "build",
              node_execution_id: id,
              session_id: "research-session",
              status: "succeeded",
            },
            files: [],
            missing_subtasks: [],
          },
        },
      });
    },
  );
  await page.goto("/visual.html?mode=file-run&clone=1");
  await page.getByTestId("workflow-run-node-plan").click();
  await expandDeliverables(page);
  await page.getByTestId("workflow-open-subtask-research").click();
  const panel = page.getByTestId("workflow-conversation-panel");
  await expect(panel).toHaveAttribute("data-run-id", "original-run");
  await expect(panel).toHaveAttribute("data-session-id", "research-session");
  await expect(panel).toContainText("Original reply for research-session");
  await expect(panel.locator('[contenteditable="true"]')).toHaveCount(0);
  await expect(panel).toContainText("Source run original · read only");
  await expandDeliverables(page);
  await expect(page.getByTestId("workflow-split-batch")).toHaveCount(0);
  await page
    .getByRole("button", { name: "Back to work list", exact: true })
    .click();
  await expect(page.getByTestId("workflow-split-batch")).toBeVisible();
  expect(api.requests.filter((request) => !request.startsWith("GET "))).toEqual(
    [],
  );
});

test("tool output uses the existing log viewer inside the float and preserves the draft on return", async ({
  page,
}) => {
  const api = await mockWorkflowConversations(page, { withToolOutput: true });
  await page.goto("/visual.html?mode=file-run");
  await page.getByTestId("workflow-run-node-build").click();
  const panel = page.getByTestId("workflow-conversation-panel");
  const editor = panel.locator('[contenteditable="true"]').first();
  await expect(editor).toBeVisible();
  await editor.fill("Draft kept while reading tool output");
  await panel.getByRole("button", { name: /inspect_fixture/ }).click();
  const preview = panel.getByTestId("conversation-log-preview");
  await expect(preview).toContainText(
    "Exact tool output retained in the original viewer",
  );
  await expect(editor).toBeHidden();
  await expect(
    preview.getByRole("button", { name: "Back", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(preview).toHaveCount(0);
  await expect(panel).toBeVisible();
  await expect(editor).toHaveText("Draft kept while reading tool output");
  await expect(
    panel.getByRole("button", { name: /inspect_fixture/ }),
  ).toBeFocused();
  expect(api.requests.filter((request) => request.startsWith("POST "))).toEqual(
    [],
  );
});
