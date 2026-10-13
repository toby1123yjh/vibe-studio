import { expect, test, type Locator, type Page } from "@playwright/test";

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
  const workspaceHref =
    "/projects/visual-project/tasks/visual-issue/workspaces/visual-workspace";
  for (const [index, session] of [
    "research-session",
    "review-session",
    "analysis-session",
  ].entries()) {
    await expect(
      tasks.nth(index).getByRole("link", { name: "Open execution session" }),
    ).toHaveAttribute("href", `${workspaceHref}?session_id=${session}`);
  }
  await expect(tasks.nth(3).getByRole("link")).toHaveCount(0);
  await expect(
    batch.getByRole("link", { name: "Open main session" }),
  ).toHaveAttribute("href", `${workspaceHref}?session_id=main-session`);
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
  await page.getByTestId("workflow-run-node-build").click();
  const task = page.getByTestId("workflow-split-subtask").first();
  await expect(task).toContainText(filePath);
  expect(paths).toEqual([]);
  await task.getByRole("button", { name: filePath, exact: true }).click();
  const preview = page.getByRole("dialog", { name: filePath, exact: true });
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
  await preview.getByRole("button", { name: "Close", exact: true }).click();
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
  await page
    .getByTestId("workflow-split-subtask")
    .first()
    .getByRole("button", { name: filePath, exact: true })
    .click();
  const preview = page.getByRole("dialog", { name: filePath, exact: true });
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
  await preview.getByRole("button", { name: "Close", exact: true }).click();
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
