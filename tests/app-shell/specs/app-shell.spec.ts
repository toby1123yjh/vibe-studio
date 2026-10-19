import { expect, test, type Page } from "@playwright/test";
import { mockEmptyDashboard } from "../support/dashboard-api";

test.beforeEach(async ({ page }) => mockEmptyDashboard(page));

const UNAVAILABLE_WORKFLOW_REASON =
  "Workflow service is unavailable in this environment.";

async function gotoFixture(page: Page) {
  await page.goto("/");
  await expect(page.getByTestId("current-route")).toHaveText("/dashboard");
}

test("desktop keeps fixed shell zones and automatically pages the middle object list", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);

  const sidebar = page.locator(".vk-product-sidebar");
  const identity = sidebar.locator(".vk-shell-identity");
  const primary = sidebar.getByRole("navigation", { name: "Primary" });
  const objectList = sidebar.locator(".vk-object-lists");
  const system = sidebar.locator(".vk-system-zone");

  await expect(sidebar).toBeVisible();
  await expect(identity).toContainText("Vibe Studio");
  await expect(identity).toContainText("Fixture / Local");
  await expect(primary.getByRole("button")).toHaveCount(6);
  const unavailableWorkflow = primary.getByRole("button", {
    name: "Workflows",
  });
  await expect(unavailableWorkflow).toHaveAttribute("aria-disabled", "true");
  await expect(unavailableWorkflow).toHaveJSProperty("disabled", false);
  await expect(
    unavailableWorkflow.getByText(UNAVAILABLE_WORKFLOW_REASON),
  ).toBeVisible();
  await unavailableWorkflow.focus();
  await expect(unavailableWorkflow).toBeFocused();
  await unavailableWorkflow.press("Enter");
  await expect(page.getByTestId("current-route")).toHaveText("/dashboard");
  await expect(system.getByRole("button", { name: "Settings" })).toBeVisible();
  await expect(system.getByRole("button", { name: "Settings" })).toHaveText(
    "Settings",
  );
  await expect(
    system.getByRole("button", { name: "Update 2.0.0 ready" }),
  ).toBeVisible();
  await expect(page.locator(".vk-app-shell__update-announcement")).toHaveText(
    "Update 2.0.0 ready",
  );
  await expect(system).toContainText("v0.1.0-contract");
  await expect(objectList).toHaveCSS("overflow-y", "auto");
  await expect
    .poll(() =>
      objectList.evaluate(
        (element) => element.scrollHeight > element.clientHeight,
      ),
    )
    .toBe(true);

  const identityBefore = await identity.boundingBox();
  const systemBefore = await system.boundingBox();
  await objectList.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  expect(await identity.boundingBox()).toEqual(identityBefore);
  expect(await system.boundingBox()).toEqual(systemBefore);

  await page
    .getByRole("button", { name: "Enable automatic project page" })
    .click();
  const sentinel = sidebar.locator(".vk-sidebar-load-more");
  await expect(sentinel).toBeAttached();
  await sentinel.scrollIntoViewIfNeeded();
  await expect(page.getByTestId("project-page-loads")).toHaveText("1");
  await expect(
    sidebar.getByRole("button", { name: "Project loaded automatically" }),
  ).toBeAttached();
  await expect(sidebar.getByText("Load more")).toHaveCount(0);
});

test("project selection and the Projects directory never highlight together", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);

  const sidebar = page.locator(".vk-product-sidebar");
  const projectsEntry = sidebar
    .getByRole("navigation", { name: "Primary" })
    .getByRole("button", { name: "Projects", exact: true });

  for (const [title, route] of [
    ["Project 01", "/projects/project-1"],
    ["Project 02", "/projects/project-2"],
  ]) {
    const project = sidebar.getByRole("button", { name: title, exact: true });
    await project.click();
    await expect(page.getByTestId("current-route")).toHaveText(route);
    await expect(project).toHaveAttribute("aria-current", "page");
    await expect(projectsEntry).not.toHaveAttribute("aria-current", "page");
    await expect(sidebar.locator('[data-active="true"]')).toHaveCount(1);
  }

  await projectsEntry.click();
  await expect(page.getByTestId("current-route")).toHaveText("/projects");
  await expect(projectsEntry).toHaveAttribute("aria-current", "page");
  await expect(sidebar.locator('[data-active="true"]')).toHaveCount(1);
});

test("highlights only the selected session when a workspace has multiple sessions", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);

  const sidebar = page.locator(".vk-product-sidebar");
  const sessions = sidebar.getByRole("region", {
    name: "Sessions",
    exact: true,
  });
  const selectedSession = sessions.getByRole("button", {
    name: "Session 02, Claude Code",
  });

  await expect(
    sessions.getByRole("button", { name: /Session 0[1-3],/ }),
  ).toHaveCount(3);
  await selectedSession.click();

  await expect(page.getByTestId("current-route")).toHaveText(
    "/workspaces/workspace-shared?session_id=session-2",
  );
  await expect(sessions.locator('[data-active="true"]')).toHaveCount(1);
  await expect(selectedSession).toHaveAttribute("aria-current", "page");
  await expect(
    sessions.getByRole("button", { name: "Session 01, Codex" }),
  ).not.toHaveAttribute("aria-current", "page");
  await expect(
    sessions.getByRole("button", { name: "Session 03, Codex" }),
  ).not.toHaveAttribute("aria-current", "page");
});

test("session rows expose an explicit delete control", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);

  const sessions = page
    .locator(".vk-product-sidebar")
    .getByRole("region", { name: "Sessions", exact: true });

  await expect(
    sessions.getByRole("button", { name: "Delete session" }),
  ).toHaveCount(16);
});

test("session delete controls reveal on row interaction", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);

  const sessionRow = page
    .locator(".vk-product-sidebar .vk-object-link--session")
    .first();
  const deleteButton = sessionRow.getByRole("button", {
    name: "Delete session",
  });
  const chevron = sessionRow.locator(".vk-object-link__chevron");

  await expect(deleteButton).toHaveCSS("opacity", "0");
  await expect(chevron).toHaveCount(0);

  await sessionRow.hover();
  await expect(deleteButton).toHaveCSS("opacity", "1");

  await sessionRow.getByRole("button", { name: /Session 01,/ }).focus();
  await expect(deleteButton).toHaveCSS("opacity", "1");

  await deleteButton.focus();
  await expect(deleteButton).toBeFocused();
  await expect(deleteButton).toHaveCSS("opacity", "1");
});

test("search preserves the background route and provides grouped keyboard interaction", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);

  const searchTrigger = page
    .locator(".vk-product-sidebar")
    .getByRole("button", { name: "Search" });
  await searchTrigger.click();

  const dialog = page.getByRole("dialog", { name: "Global search" });
  const input = dialog.getByRole("combobox");
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveAttribute("aria-modal", "true");
  await expect(input).toBeFocused();
  await expect(page.getByTestId("current-route")).toHaveText("/dashboard");
  await expect(page.locator(".vk-app-shell__layout")).toHaveCSS(
    "filter",
    "blur(8px)",
  );
  await expect(page.locator(".vk-search-overlay")).not.toHaveCSS(
    "background-color",
    "rgba(0, 0, 0, 0)",
  );

  for (const group of [
    "Agents",
    "Configuration",
    "Tools",
    "Features and objects",
  ]) {
    await expect(dialog.getByRole("heading", { name: group })).toBeVisible();
  }
  await expect(dialog.getByRole("option", { name: /Workflows/ })).toHaveCount(
    0,
  );

  const selectedOption = dialog.locator(
    '[role="option"][aria-selected="true"]',
  );
  await expect(selectedOption).toContainText("Codex");
  await page.keyboard.press("ArrowDown");
  await expect(selectedOption).toContainText("Claude Code");
  await page.keyboard.press("Enter");
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId("current-route")).toHaveText(
    "/agents?provider=claude_code",
  );
  await expect(page.locator("#main-content")).toBeFocused();

  await searchTrigger.click();
  await input.fill("no destination has this phrase");
  expect(await dialog.getByText("Updating results...").count()).toBe(0);
  expect(await dialog.getByRole("option").count()).toBe(0);
  await expect(dialog.getByText("No matching destination")).toBeVisible();
  await expect(page.getByTestId("current-route")).toHaveText(
    "/agents?provider=claude_code",
  );
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(searchTrigger).toBeFocused();
});

test("search immediately selects the current query and reopens without prior query state", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);

  const searchTrigger = page
    .locator(".vk-product-sidebar")
    .getByRole("button", { name: "Search" });
  await searchTrigger.click();
  const dialog = page.getByRole("dialog", { name: "Global search" });
  const input = dialog.getByRole("combobox");

  await input.fill("Project 01");
  await expect(dialog.getByRole("option")).toHaveCount(1);
  await expect(dialog.getByRole("option")).toContainText("Project 01");
  await input.press("Enter");
  await expect(page.getByTestId("current-route")).toHaveText(/\/projects\//);
  await expect(dialog).toBeHidden();
  await searchTrigger.click();
  await expect(input).toHaveValue("");
  await expect(
    dialog.locator('[role="option"][aria-selected="true"]'),
  ).toContainText("Codex");
});

test("project and session discovery states stay isolated and retry their own source", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);

  const sidebar = page.locator(".vk-product-sidebar");
  const projectSection = sidebar
    .getByRole("heading", { name: "Projects" })
    .locator("..");
  const sessionSection = sidebar.getByRole("region", {
    name: "Sessions",
    exact: true,
  });

  await page.getByRole("button", { name: "Set projects loading" }).click();
  await expect(projectSection.locator('[data-state="loading"]')).toContainText(
    "Loading projects",
  );
  await expect(
    sessionSection.getByRole("button", { name: "Session 01, Codex" }),
  ).toBeAttached();

  await page.getByRole("button", { name: "Set projects empty" }).click();
  await expect(projectSection.locator('[data-state="empty"]')).toHaveCount(0);
  await expect(projectSection.getByText("No projects")).toHaveCount(0);

  await page
    .getByRole("button", { name: "Set projects initial error" })
    .click();
  const projectError = projectSection.locator('[data-state="error"]');
  await expect(projectError).toContainText("Projects unavailable");
  await projectError.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByTestId("project-retries")).toHaveText("1");
  await expect(
    projectSection.getByRole("button", { name: "Project 01" }),
  ).toBeAttached();

  await page.getByRole("button", { name: "Set sessions cached error" }).click();
  const sessionDegraded = sessionSection.locator('[data-state="degraded"]');
  await expect(sessionDegraded).toContainText("Sessions may be out of date");
  await expect(
    sessionSection.getByRole("button", { name: "Session 01, Codex" }),
  ).toBeAttached();
  await sessionDegraded.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByTestId("session-retries")).toHaveText("1");
});

test("search reports initial and cached source failures without hiding static results", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);

  await page
    .getByRole("button", { name: "Set projects initial error" })
    .click();
  await page.getByRole("button", { name: "Set sessions cached error" }).click();
  await page
    .locator(".vk-product-sidebar")
    .getByRole("button", { name: "Search" })
    .click();

  const dialog = page.getByRole("dialog", { name: "Global search" });
  const projectError = dialog.locator('[data-state="error"]');
  const sessionDegraded = dialog.locator('[data-state="degraded"]');
  await expect(projectError).toContainText("Projects results unavailable");
  await expect(sessionDegraded).toContainText(
    "Sessions results may be incomplete",
  );
  await expect(dialog.getByRole("option", { name: /Dashboard/ })).toBeVisible();

  await projectError.getByRole("button", { name: "Retry" }).click();
  await sessionDegraded.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByTestId("project-retries")).toHaveText("1");
  await expect(page.getByTestId("session-retries")).toHaveText("1");
  await expect(dialog.locator('[data-state="error"]')).toHaveCount(0);
  await expect(dialog.locator('[data-state="degraded"]')).toHaveCount(0);
});

test("Dashboard displays its live table filters and an honest empty result", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await gotoFixture(page);
  await page
    .getByRole("button", { name: "Show dashboard state contract" })
    .click();

  const dashboard = page.getByTestId("dashboard-page");
  await expect(
    dashboard.getByRole("table", { name: "Agent work monitoring" }),
  ).toBeVisible();
  await expect(dashboard.locator(".session-dashboard__tabs")).toHaveCount(0);
  await expect(
    dashboard.getByRole("heading", { name: "No work matches this view" }),
  ).toBeVisible();
  await expect(dashboard.locator('[data-state="unavailable"]')).toHaveCount(0);
});

test("tablet object drawer reaches every object and restores focus on Escape", async ({
  page,
}) => {
  await page.setViewportSize({ width: 900, height: 800 });
  await gotoFixture(page);

  const unavailableWorkflow = page
    .locator(".vk-product-rail")
    .getByRole("button", { name: "Workflows" });
  await expect(unavailableWorkflow).toHaveAttribute("aria-disabled", "true");
  await expect(unavailableWorkflow).toHaveJSProperty("disabled", false);
  await expect(unavailableWorkflow).toHaveAttribute(
    "title",
    `Workflows — unavailable: ${UNAVAILABLE_WORKFLOW_REASON}`,
  );
  await expect(
    unavailableWorkflow.locator(".vk-module-unavailable-reason"),
  ).toContainText(UNAVAILABLE_WORKFLOW_REASON);
  await unavailableWorkflow.focus();
  await expect(unavailableWorkflow).toBeFocused();
  await unavailableWorkflow.press("Enter");
  await expect(page.getByTestId("current-route")).toHaveText("/dashboard");

  const updateTrigger = page
    .locator(".vk-product-rail")
    .getByRole("button", { name: "Update 2.0.0 ready" });
  await expect(updateTrigger).toBeVisible();
  await expect(updateTrigger).toHaveCSS("width", "44px");
  await expect(updateTrigger).toHaveCSS("height", "44px");

  const browseTrigger = page
    .locator(".vk-product-rail")
    .getByRole("button", { name: "Browse projects and sessions" });
  await browseTrigger.click();
  const drawer = page.getByRole("dialog", {
    name: "Browse projects and sessions",
  });
  await expect(drawer).toBeVisible();
  await expect(
    drawer.getByRole("button", { name: "Close projects and sessions" }),
  ).toBeFocused();

  const lastProject = drawer.getByRole("button", { name: "Project 24" });
  const lastSession = drawer.getByRole("button", {
    name: "Session 16, Claude Code",
  });
  await expect(lastProject).toBeAttached();
  await lastProject.scrollIntoViewIfNeeded();
  await expect(lastProject).toBeVisible();
  await lastSession.scrollIntoViewIfNeeded();
  await expect(lastSession).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(drawer).toBeHidden();
  await expect(browseTrigger).toBeFocused();
});

test("mobile exposes five modules, object browsing, system actions, and no horizontal overflow", async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 812 });
  await gotoFixture(page);

  const bottomNavigation = page.getByRole("navigation", {
    name: "Primary mobile navigation",
  });
  await expect(bottomNavigation).toBeVisible();
  await expect(bottomNavigation.getByRole("button")).toHaveCount(5);
  for (const name of [
    "Dashboard",
    "Search",
    "Projects",
    "Workflows",
    "Agents",
  ]) {
    await expect(bottomNavigation.getByRole("button", { name })).toBeVisible();
  }

  const unavailableWorkflow = bottomNavigation.getByRole("button", {
    name: "Workflows",
  });
  await expect(unavailableWorkflow).toHaveAttribute("aria-disabled", "true");
  await expect(unavailableWorkflow).toHaveJSProperty("disabled", false);
  await expect(unavailableWorkflow).toHaveAttribute(
    "title",
    `Workflows — unavailable: ${UNAVAILABLE_WORKFLOW_REASON}`,
  );
  await expect(
    unavailableWorkflow.locator(".vk-module-unavailable-reason"),
  ).toContainText(UNAVAILABLE_WORKFLOW_REASON);
  await unavailableWorkflow.focus();
  await expect(unavailableWorkflow).toBeFocused();
  await unavailableWorkflow.press("Enter");
  await expect(page.getByTestId("current-route")).toHaveText("/dashboard");

  const productMenuTrigger = page.getByRole("button", {
    name: "Open product menu",
  });
  await productMenuTrigger.click();
  const productMenu = page.getByRole("menu");
  await expect(
    productMenu.getByRole("menuitem", { name: "Settings" }),
  ).toBeVisible();
  await expect(
    productMenu.getByRole("menuitem", { name: "Fixture User" }),
  ).toBeVisible();
  await expect(
    productMenu.getByRole("menuitem", { name: "Update 2.0.0 ready" }),
  ).toBeVisible();
  await expect(productMenu).toContainText("Version 0.1.0-contract");
  await productMenu.getByRole("menuitem", { name: "Settings" }).click();
  await expect(page.getByTestId("system-action")).toHaveText("settings");

  const browseTrigger = page.getByRole("button", { name: "Browse" });
  await browseTrigger.click();
  const drawer = page.getByRole("dialog", {
    name: "Browse projects and sessions",
  });
  const lastProject = drawer.getByRole("button", { name: "Project 24" });
  const lastSession = drawer.getByRole("button", {
    name: "Session 16, Claude Code",
  });
  await lastProject.scrollIntoViewIfNeeded();
  await expect(lastProject).toBeVisible();
  await lastSession.scrollIntoViewIfNeeded();
  await expect(lastSession).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(drawer).toBeHidden();
  await expect(browseTrigger).toBeFocused();

  await expect
    .poll(() =>
      page.evaluate(() => ({
        viewport: window.innerWidth,
        document: document.documentElement.scrollWidth,
        body: document.body.scrollWidth,
      })),
    )
    .toEqual({ viewport: 375, document: 375, body: 375 });
});

test("PageCanvas switches contained/full-bleed geometry and tokens render in Light/Dark", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await gotoFixture(page);

  const canvas = page.locator("#main-content");
  const content = canvas.locator(".vk-page-canvas__content");
  await expect(canvas).toHaveAttribute("data-mode", "contained");
  const containedCanvasBox = await canvas.boundingBox();
  const containedContentBox = await content.boundingBox();
  if (!containedCanvasBox || !containedContentBox) {
    throw new Error("PageCanvas fixture did not produce geometry");
  }
  expect(containedContentBox.width).toBeLessThan(containedCanvasBox.width);
  expect(containedContentBox.width).toBeCloseTo(1120, 0);
  expect(
    containedContentBox.x -
      containedCanvasBox.x -
      (containedCanvasBox.width - containedContentBox.width) / 2,
  ).toBeCloseTo(0, 0);

  await page.getByRole("button", { name: "Full-bleed canvas" }).click();
  await expect(canvas).toHaveAttribute("data-mode", "full-bleed");
  const fullCanvasBox = await canvas.boundingBox();
  const fullContentBox = await content.boundingBox();
  if (!fullCanvasBox || !fullContentBox) {
    throw new Error("Full-bleed PageCanvas fixture did not produce geometry");
  }
  expect(fullContentBox.width).toBeCloseTo(fullCanvasBox.width, 0);

  const lightSurface = await page.evaluate(() =>
    getComputedStyle(document.documentElement)
      .getPropertyValue("--vk-surface-canvas")
      .trim(),
  );
  const lightBackground = await page
    .getByTestId("app-shell")
    .evaluate((element) => getComputedStyle(element).backgroundColor);
  await page.getByRole("button", { name: "Dark theme" }).click();
  const darkSurface = await page.evaluate(() =>
    getComputedStyle(document.documentElement)
      .getPropertyValue("--vk-surface-canvas")
      .trim(),
  );
  expect(lightSurface).not.toBe("");
  expect(darkSurface).not.toBe("");
  expect(darkSurface).not.toBe(lightSurface);
  await expect
    .poll(() =>
      page
        .getByTestId("app-shell")
        .evaluate((element) => getComputedStyle(element).backgroundColor),
    )
    .not.toBe(lightBackground);
  await expect(page.getByTestId("app-shell")).not.toHaveCSS(
    "background-color",
    "rgba(0, 0, 0, 0)",
  );
});
