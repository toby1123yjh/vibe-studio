import { expect, test, type Page } from "@playwright/test";

const preview = (page: Page) => page.getByTestId("session-composer-preview");
const message = (page: Page) =>
  preview(page).getByRole("textbox", { name: "Message", exact: true });

async function openComposer(page: Page, query = "") {
  await page.goto(
    `/?signedOut=true&selectors=true&composerPreview=true&sessionComposer=true${query}`,
  );
  await page
    .getByRole("region", { name: "Session agent selector", exact: true })
    .getByRole("button", { name: "CODEX", exact: true })
    .click();
  await expect(
    preview(page).getByRole("button", {
      name: "gpt-6.1-sol · Max",
      exact: true,
    }),
  ).toBeVisible();
}

async function expectContainedActions(page: Page) {
  const measurements = await preview(page).evaluate((element) => {
    const shell = element.getBoundingClientRect();
    const box = element.querySelector('[data-chatbox-container="true"]')!;
    const boxBounds = box.getBoundingClientRect();
    const composer = box.firstElementChild!.getBoundingClientRect();
    const messageArea = element.querySelector(
      '[data-composer-message="true"]',
    )!;
    const actions = element.querySelector('[data-composer-actions="true"]')!;
    return {
      horizontalOverflow: box.scrollWidth - box.clientWidth,
      actionOverflow: actions.scrollWidth - actions.clientWidth,
      bottomOverflow: actions.getBoundingClientRect().bottom - shell.bottom,
      overlap:
        messageArea.getBoundingClientRect().bottom -
        actions.getBoundingClientRect().top,
      transcriptHeight: element
        .querySelector('[data-testid="preview-transcript"]')!
        .getBoundingClientRect().height,
      insets: {
        left: composer.left - boxBounds.left,
        right: boxBounds.right - composer.right,
        top: composer.top - boxBounds.top,
        bottom: shell.bottom - composer.bottom,
      },
      scrollButtonGap:
        composer.top -
        element
          .querySelector('button[aria-label="Scroll to bottom"]')!
          .getBoundingClientRect().bottom,
    };
  });
  expect(measurements.horizontalOverflow).toBeLessThanOrEqual(1);
  expect(measurements.actionOverflow).toBeLessThanOrEqual(1);
  expect(measurements.bottomOverflow).toBeLessThanOrEqual(1);
  expect(measurements.overlap).toBeLessThanOrEqual(1);
  expect(measurements.transcriptHeight).toBeGreaterThan(40);
  for (const inset of Object.values(measurements.insets)) {
    expect(inset).toBeGreaterThanOrEqual(7.5);
  }
  expect(measurements.scrollButtonGap).toBeGreaterThanOrEqual(7.5);
}

test("composer separates settings, text and actions and keeps the same draft across widths", async ({
  page,
}) => {
  await openComposer(page);
  const editor = await message(page).elementHandle();
  await message(page).fill("A draft that survives resizing");
  await expect(page.getByTestId("composer-draft")).toHaveText(
    "A draft that survives resizing",
  );
  for (const compact of [true, false]) {
    await page
      .getByRole("checkbox", { name: "Compact conversation", exact: true })
      .setChecked(compact);
    for (const width of [480, 600, 900]) {
      await page
        .getByRole("spinbutton", { name: "Composer width", exact: true })
        .fill(String(width));
      await expectContainedActions(page);
      await expect(message(page)).toHaveText("A draft that survives resizing");
      expect(
        await message(page).evaluate(
          (current, original) => current === original,
          editor,
        ),
      ).toBe(true);
    }
  }
  const order = await preview(page).evaluate((element) => {
    const config = element
      .querySelector('[data-composer-configuration="true"]')!
      .getBoundingClientRect();
    const input = element
      .querySelector('[data-composer-message="true"]')!
      .getBoundingClientRect();
    return config.bottom <= input.top;
  });
  expect(order).toBe(true);
  await expect(
    preview(page).getByRole("button", { name: "Tasks", exact: true }),
  ).toHaveCount(0);
  await expect(
    preview(page).getByRole("img", { name: "Context usage" }),
  ).toHaveCount(0);
});

test("long drafts scroll inside the message area and preserve visible actions in a short panel", async ({
  page,
}) => {
  await openComposer(page);
  const longDraft = Array.from(
    { length: 45 },
    (_, i) => `Line ${i}: ${"long-directory/".repeat(24)}`,
  ).join("\n");
  await message(page).fill(longDraft);
  await page
    .getByRole("spinbutton", { name: "Composer width", exact: true })
    .fill("480");
  await page
    .getByRole("spinbutton", { name: "Conversation height", exact: true })
    .fill("280");
  await expectContainedActions(page);
  const scroll = await preview(page)
    .locator('[data-composer-message="true"]')
    .evaluate((element) => ({
      height: element.clientHeight,
      scrollHeight: element.scrollHeight,
      horizontalOverflow: element.scrollWidth - element.clientWidth,
    }));
  expect(scroll.scrollHeight).toBeGreaterThan(scroll.height);
  expect(scroll.horizontalOverflow).toBeLessThanOrEqual(1);
  await preview(page)
    .getByRole("button", { name: "Send", exact: true })
    .click();
  await expect(page.getByTestId("composer-action")).toHaveText("send");
});

test("sending, running, queued and approval controls remain reachable without remounting the normal editor", async ({
  page,
}) => {
  await openComposer(page);
  await message(page).fill("The next draft");
  const editor = await message(page).elementHandle();
  await page
    .getByRole("spinbutton", { name: "Composer width", exact: true })
    .fill("480");
  for (const [state, action, result] of [
    ["sending", "Stop", "stop"],
    ["running", "Queue", "queue"],
    ["queued", "Cancel Queue", "cancel-queue"],
  ] as const) {
    await page
      .getByRole("combobox", { name: "Runtime state", exact: true })
      .selectOption(state);
    await expectContainedActions(page);
    expect(
      await message(page).evaluate(
        (current, original) => current === original,
        editor,
      ),
    ).toBe(true);
    await expect(message(page)).toHaveText("The next draft");
    await preview(page).getByRole("button", { name: action }).click();
    await expect(page.getByTestId("composer-action")).toHaveText(result);
  }
  await page
    .getByRole("combobox", { name: "Runtime state", exact: true })
    .selectOption("approval");
  await expectContainedActions(page);
  await preview(page)
    .getByRole("button", { name: "Request Changes", exact: true })
    .click();
  await expect(page.getByTestId("composer-action")).toHaveText(
    "request-changes",
  );
});

test("secondary session actions are discoverable in the more menu", async ({
  page,
}) => {
  await openComposer(page);
  await expect(
    preview(page).getByRole("button", { name: "Example session", exact: true }),
  ).toHaveCount(0);
  const more = preview(page).getByRole("button", {
    name: "More actions",
    exact: true,
  });
  await more.focus();
  await page.keyboard.press("Enter");
  await page.getByRole("menuitem", { name: "Rename", exact: true }).click();
  await expect(page.getByTestId("composer-action")).toHaveText("rename");
  await expect(more).toBeFocused();
});

test("small screens keep settings and action buttons inside the composer", async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 667 });
  await page.emulateMedia({ reducedMotion: "reduce", colorScheme: "dark" });
  await openComposer(page, "&theme=dark");
  await message(page).fill("Mobile draft");
  await expectContainedActions(page);
  await expect(
    preview(page).getByRole("button", {
      name: "gpt-6.1-sol · Max",
      exact: true,
    }),
  ).toBeVisible();
  await preview(page)
    .getByRole("button", { name: "Send", exact: true })
    .click();
  await expect(page.getByTestId("composer-action")).toHaveText("send");
});

test("run settings retain native agent and permission controls without crowding the input", async ({
  page,
}) => {
  await openComposer(page, "&composerSettings=true");
  const settings = preview(page).getByRole("button", {
    name: "Run settings",
    exact: true,
  });
  await settings.click();
  const permissions = page.getByRole("menuitem", { name: /Permissions/ });
  await permissions.focus();
  await page.keyboard.press("ArrowRight");
  await page.getByRole("menuitem", { name: "Ask", exact: true }).click();
  await expect(page.getByTestId("session-selection")).toContainText(
    '"permission_policy":"SUPERVISED"',
  );
  await expect(settings).toBeFocused();
  await settings.click();
  const agent = page.getByRole("menuitem", { name: /^Agent/ });
  await agent.focus();
  await page.keyboard.press("ArrowRight");
  await page
    .getByRole("menuitem", { name: "Review agent", exact: true })
    .click();
  await expect(page.getByTestId("session-selection")).toContainText(
    '"agent_id":"reviewer"',
  );
  await expect(settings).toBeFocused();
  await expectContainedActions(page);

  for (const viewport of [
    { width: 375, height: 700 },
    { width: 740, height: 375 },
  ]) {
    await page.setViewportSize(viewport);
    for (const label of ["Service tier", "Diagnostics"]) {
      await settings.click();
      const menu = page.getByRole("menu", {
        name: "Run settings",
        exact: true,
      });
      const trigger = menu.getByRole("menuitem", {
        name: new RegExp(`^${label}`),
      });
      await trigger.focus();
      await page.keyboard.press("ArrowRight");
      await expect(page.getByRole("menu").last()).toContainText(
        label === "Diagnostics" ? "Refresh native capabilities" : "Standard",
      );
      await expect
        .poll(() =>
          page.getByRole("menu").evaluateAll((elements) =>
            elements.every((element) => {
              const bounds = element.getBoundingClientRect();
              return (
                bounds.left >= -1 &&
                bounds.top >= -1 &&
                bounds.right <= window.innerWidth + 1 &&
                bounds.bottom <= window.innerHeight + 1 &&
                element.scrollWidth <= element.clientWidth + 1
              );
            }),
          ),
        )
        .toBe(true);
      await page.keyboard.press("ArrowLeft");
      await expect(trigger).toBeFocused();
      await page.keyboard.press("Escape");
      await expect(settings).toBeFocused();
    }
  }
});

test("create mode displays project and task context apart from the send actions", async ({
  page,
}) => {
  await page.goto("/?signedOut=true&selectors=true&composerPreview=true");
  const box = page.getByTestId("composer-preview");
  const context = box.locator('[data-composer-context="true"]');
  await expect(
    context.getByRole("combobox", { name: "Project", exact: true }),
  ).toBeVisible();
  await expect(context).toContainText("TASK-2");
  await expect(box.locator('[data-composer-actions="true"]')).not.toContainText(
    "TASK-2",
  );
  await expect(
    box
      .locator('[data-composer-actions="true"]')
      .getByRole("button", { name: "Attach file", exact: true }),
  ).toBeVisible();
});
