import { expect, test, type Page } from "@playwright/test";

const preview = (page: Page) => page.getByTestId("session-composer-preview");
const message = (page: Page) =>
  preview(page).getByRole("textbox", { name: "Message", exact: true });

test.beforeEach(({ page }) => {
  page.on("pageerror", (error) => {
    throw error;
  });
});

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

test("composer puts text before its compact controls and keeps the same draft across widths", async ({
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
    return input.bottom <= config.top;
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
  await page
    .getByRole("combobox", { name: "Runtime state", exact: true })
    .selectOption("sending");
  await expectContainedActions(page);
  await expect(
    preview(page).getByRole("button", { name: "Sending", exact: true }),
  ).toBeDisabled();
  await expect(message(page)).toBeEditable();
  for (const [state, action, result] of [
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

test("send feedback stays visible through request, run and stop without replacing the editor", async ({
  page,
}, testInfo) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await openComposer(page, "&clearOnSend=true&pendingSend=true");
  await message(page).fill("Show immediate feedback");
  const editor = await message(page).elementHandle();
  await message(page).press("Enter");
  const activity = preview(page).locator("[data-composer-activity]");
  await expect(activity).toBeVisible();
  await expect(activity).toHaveText("Sending");
  await expect(
    preview(page).getByRole("button", { name: "Sending", exact: true }),
  ).toBeDisabled();
  await expect(message(page)).toBeEmpty();
  await expect(message(page)).toBeFocused();
  await message(page).fill("Prepare the next message");

  const state = page.getByRole("combobox", {
    name: "Runtime state",
    exact: true,
  });
  for (const width of [480, 900]) {
    await page
      .getByRole("spinbutton", { name: "Composer width", exact: true })
      .fill(String(width));
    for (const mode of ["sending", "running", "queued"]) {
      await state.selectOption(mode);
      await expect(activity).toHaveCount(1);
      await expect(activity).toBeVisible();
      await expect(activity).toHaveAttribute(
        "data-composer-activity",
        mode === "sending" ? "sending" : "running",
      );
      await expect(activity).toHaveText(
        mode === "sending" ? "Sending" : "in progress",
      );
      await expectContainedActions(page);
      const indicator = activity.locator("svg");
      await expect
        .poll(() =>
          indicator.evaluate((element) =>
            element.getAnimations().some((animation) => {
              const timing = animation.effect!.getTiming();
              return (
                animation.playState === "running" &&
                timing.iterations === Infinity
              );
            }),
          ),
        )
        .toBe(true);
      const startedAt = await indicator.evaluate(
        (element) => element.getAnimations()[0].currentTime as number,
      );
      await expect
        .poll(() =>
          indicator.evaluate(
            (element) => element.getAnimations()[0].currentTime as number,
          ),
        )
        .toBeGreaterThan(startedAt);
      expect(
        await message(page).evaluate(
          (element, original) => element === original,
          editor,
        ),
      ).toBe(true);
      await expect(message(page)).toHaveText("Prepare the next message");
      await expect(
        preview(page).locator(
          "[data-composer-primary-actions] [role='status']",
        ),
      ).toHaveCount(0);
      if (mode === "sending" || mode === "running") {
        await preview(page).screenshot({
          path: testInfo.outputPath(`composer-${mode}-${width}.png`),
          animations: "disabled",
        });
      }
    }
  }
  await preview(page)
    .getByRole("button", { name: "Stop", exact: true })
    .click();
  await expect(page.getByTestId("composer-action")).toHaveText("stop");
  await state.selectOption("stopping");
  await expect(activity).toHaveAttribute("data-composer-activity", "stopping");
  await expect(activity).toHaveText("Stopping");
  await expect(
    preview(page).getByRole("button", { name: "Stopping", exact: true }),
  ).toBeDisabled();
  for (const mode of ["approval", "idle"]) {
    await state.selectOption(mode);
    await expect(activity).toHaveCount(0);
  }
  await page.emulateMedia({ reducedMotion: "reduce" });
  for (const mode of ["sending", "running"]) {
    await state.selectOption(mode);
    await expect(activity).toBeVisible();
    await expect(
      preview(page).getByRole("button", {
        name: mode === "sending" ? "Sending" : "Stop",
        exact: true,
      }),
    ).toBeVisible();
    expect(
      await activity
        .locator("svg")
        .evaluateAll((elements) =>
          elements.every(
            (element) => getComputedStyle(element).animationName === "none",
          ),
        ),
    ).toBe(true);
  }
});

test("new session creation uses the same ongoing send feedback", async ({
  page,
}) => {
  await page.goto("/?signedOut=true&selectors=true&composerPreview=true");
  const box = page.getByTestId("composer-preview");
  await box
    .getByRole("textbox", { name: "Message", exact: true })
    .fill("Start a session");
  await box.getByRole("button", { name: "Create", exact: true }).click();
  await expect(box.locator("[data-composer-activity='sending']")).toHaveText(
    "Sending",
  );
  await expect(
    box.getByRole("button", { name: "Creating...", exact: true }),
  ).toBeDisabled();
});

test("Enter and click send the latest editor snapshot without waiting for React draft state", async ({
  page,
}) => {
  await openComposer(page);
  await message(page).fill("Last character");
  await message(page).evaluate((element) => {
    element.focus();
    const selection = window.getSelection()!;
    selection.selectAllChildren(element);
    selection.collapseToEnd();
    document.execCommand("insertText", false, "!");
    element.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        keyCode: 13,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await expect(page.getByTestId("composer-submitted")).toHaveText(
    "Last character!",
  );
  await expect(page.getByTestId("composer-submit-count")).toHaveText("1");

  await message(page).evaluate((element) => {
    element.focus();
    const selection = window.getSelection()!;
    selection.selectAllChildren(element);
    selection.collapseToEnd();
    document.execCommand("insertText", false, "?");
    element
      .closest('[data-chatbox-container="true"]')!
      .querySelector<HTMLButtonElement>('button[aria-label="Send"]')!
      .click();
  });
  await expect(page.getByTestId("composer-submitted")).toHaveText(
    "Last character!?",
  );
  await expect(page.getByTestId("composer-submit-count")).toHaveText("2");
});

test("IME candidate confirmation, Shift Enter and repeated Enter do not submit", async ({
  page,
}) => {
  await openComposer(page);
  await message(page).fill("你好");
  await message(page).evaluate((element) => {
    element.focus();
    element.dispatchEvent(
      new CompositionEvent("compositionstart", { bubbles: true }),
    );
    element.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        keyCode: 229,
        isComposing: true,
        bubbles: true,
        cancelable: true,
      }),
    );
    element.dispatchEvent(
      new CompositionEvent("compositionend", { bubbles: true, data: "好" }),
    );
    element.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        keyCode: 13,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await expect(page.getByTestId("composer-submit-count")).toHaveText("0");
  // Allow the candidate-confirming Enter guard to expire before a new key press.
  await page.waitForTimeout(20);
  await message(page).press("Shift+Enter");
  await expect(page.getByTestId("composer-submit-count")).toHaveText("0");
  await message(page).evaluate((element) => {
    element.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        keyCode: 13,
        repeat: true,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await expect(page.getByTestId("composer-submit-count")).toHaveText("0");
  await message(page).press("Enter");
  await expect(page.getByTestId("composer-submit-count")).toHaveText("1");
  await expect(page.getByTestId("composer-submitted")).toContainText("你好");
});

test("submitting the final editor update clears the draft without replacing its editor", async ({
  page,
}) => {
  await openComposer(page, "&clearOnSend=true");
  await message(page).fill("Send this");
  const editor = await message(page).elementHandle();
  await message(page).evaluate((element) => {
    element.focus();
    const selection = window.getSelection()!;
    selection.selectAllChildren(element);
    selection.collapseToEnd();
    document.execCommand("insertText", false, " now");
    element.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        keyCode: 13,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await expect(page.getByTestId("composer-submitted")).toHaveText(
    "Send this now",
  );
  await expect(page.getByTestId("composer-draft")).toBeEmpty();
  await expect(message(page)).toBeEmpty();
  expect(
    await message(page).evaluate(
      (element, original) => element === original,
      editor,
    ),
  ).toBe(true);
  await expect(message(page)).toBeFocused();
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
