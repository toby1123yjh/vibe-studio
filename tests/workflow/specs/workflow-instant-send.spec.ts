import { expect, test, type Locator, type Page } from "@playwright/test";
import {
  mockWorkflowConversations,
  mockWorkflowFileContext,
} from "../support/conversation-api";

declare global {
  interface Window {
    sendEchoObservation: {
      counts: number[];
      userRows: number[];
      frames: number[];
      samples: Array<{ count: number; rows: string[] }>;
      frameId: number;
      observer: MutationObserver;
    };
  }
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.route("**/api/**", (route) => route.abort());
  await page.route("https://fonts.googleapis.com/**", (route) => route.abort());
  await mockWorkflowFileContext(page);
});

async function openConversation(page: Page) {
  await page.goto("/visual.html?mode=file-run");
  await page.getByTestId("workflow-run-node-build").click();
  const panel = page.getByTestId("workflow-conversation-panel");
  const editor = panel.locator(
    '[data-chatbox-container] [data-lexical-editor="true"]',
  );
  await expect(editor).toBeVisible();
  await expect(editor).toBeEditable();
  return { panel, editor };
}

function historyRows(panel: Locator, text: string) {
  return panel.locator("[data-semantic-key]").filter({ hasText: text });
}

async function observeMessage(page: Page, text: string) {
  await page.evaluate((text) => {
    const panel = document.querySelector(
      '[data-testid="workflow-conversation-panel"]',
    )!;
    const counts: number[] = [];
    const userRows: number[] = [];
    const frames: number[] = [];
    const samples: Array<{ count: number; rows: string[] }> = [];
    const messageRows = () => [
      ...panel.querySelectorAll("[data-semantic-key]"),
    ];
    const count = () =>
      messageRows().filter((row) => row.textContent?.includes(text)).length;
    const sample = () => {
      const next = count();
      userRows.push(
        messageRows().filter((row) =>
          row.querySelector('[role="group"][aria-label="You"]'),
        ).length,
      );
      if (counts.at(-1) !== next)
        samples.push({
          count: next,
          rows: messageRows().map(
            (row) =>
              `${row.getAttribute("data-semantic-key")} | ${row.textContent}`,
          ),
        });
      counts.push(next);
    };
    const observer = new MutationObserver(sample);
    observer.observe(panel, {
      childList: true,
      characterData: true,
      subtree: true,
    });
    sample();
    const observation = {
      counts,
      userRows,
      frames,
      samples,
      frameId: 0,
      observer,
    };
    // Readonly Markdown initializes during mount. Sample visible content each
    // frame and retain DOM mutations to diagnose missing or duplicate rows.
    const frame = () => {
      frames.push(count());
      observation.frameId = requestAnimationFrame(frame);
    };
    observation.frameId = requestAnimationFrame(frame);
    window.sendEchoObservation = observation;
  }, text);
}

async function observedMessage(page: Page) {
  const observation = await page.evaluate(async () => {
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
    window.sendEchoObservation.observer.disconnect();
    cancelAnimationFrame(window.sendEchoObservation.frameId);
    const { counts, userRows, frames, samples } = window.sendEchoObservation;
    return { counts, userRows, frames, samples };
  });
  await test.info().attach("send-handoff-dom", {
    body: JSON.stringify(observation, null, 2),
    contentType: "application/json",
  });
  return observation;
}

function deferred() {
  let resolve = () => {};
  const promise = new Promise<void>((accept) => {
    resolve = accept;
  });
  return { promise, resolve: () => resolve() };
}

test("history feedback follows canonical activity, respects reading position and supports reduced motion", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  const api = await mockWorkflowConversations(page, {
    initialRunStatus: "running",
    initialHistorySize: 100,
  });
  const { panel } = await openConversation(page);
  const activity = panel.locator("[data-conversation-activity]");
  await expect(activity).toHaveAttribute(
    "data-conversation-activity",
    "running",
  );
  await expect(activity).toBeInViewport();
  await expect(
    panel.locator("[data-chat-composer] [data-conversation-activity]"),
  ).toHaveCount(0);
  await expect
    .poll(() =>
      activity
        .locator("svg")
        .evaluate((element) =>
          element
            .getAnimations()
            .some(
              (animation) =>
                animation.effect!.getTiming().iterations === Infinity,
            ),
        ),
    )
    .toBe(true);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await expect(activity).toContainText("Agent is working");
  await expect
    .poll(() =>
      activity
        .locator("svg")
        .evaluate((element) => getComputedStyle(element).animationName),
    )
    .toBe("none");

  // The footer scrolls with the transcript; it cannot cover older messages.
  const scroll = panel.locator('[style*="overflow-anchor: none"]');
  await scroll.evaluate((element) => {
    element.dispatchEvent(
      new WheelEvent("wheel", { deltaY: -300, bubbles: true }),
    );
    element.scrollTop = Math.max(0, element.scrollTop - 300);
    element.dispatchEvent(new Event("scroll"));
  });
  const readingTop = await scroll.evaluate((element) => element.scrollTop);
  for (const status of ["awaiting_approval", "awaiting_input"] as const) {
    api.setRunStatus("analysis-session", status);
    await expect(activity).toHaveCount(0);
  }
  api.setRunStatus("analysis-session", "running");
  await expect(activity).toHaveAttribute(
    "data-conversation-activity",
    "running",
  );
  await expect
    .poll(() =>
      scroll.evaluate(
        (element, top) => Math.abs(element.scrollTop - top),
        readingTop,
      ),
    )
    .toBeLessThan(2);
  api.setRunStatus("analysis-session", "cancelling");
  await expect(activity).toHaveAttribute(
    "data-conversation-activity",
    "stopping",
  );
  await expect(activity).toHaveText("Stopping");
  api.setRunStatus("analysis-session", "cancelled");
  await expect(activity).toHaveCount(0);
});

test("an empty history echoes Enter immediately and keeps the message through HTTP acknowledgement until canonical history arrives", async ({
  page,
}, testInfo) => {
  const api = await mockWorkflowConversations(page, { initialHistory: false });
  const acknowledgement = deferred();
  const submissions: string[] = [];
  await page.route(
    "**/api/sessions/analysis-session/follow-up",
    async (route) => {
      submissions.push(route.request().postDataJSON().prompt);
      await acknowledgement.promise;
      await route.fulfill({
        json: {
          success: true,
          data: api.acknowledgement("analysis-session", "new-empty-turn"),
        },
      });
    },
  );
  const { panel, editor } = await openConversation(page);
  const text = "A message displayed before the server has answered";
  await editor.fill(text);
  await expect(
    panel.getByRole("button", { name: "Send", exact: true }),
  ).toBeEnabled();
  const editorHandle = await editor.elementHandle();
  await editor.press("Enter");

  // The request remains held: this must be real history, not the editor text.
  await expect(historyRows(panel, text)).toHaveCount(1);
  await expect(historyRows(panel, text)).toBeVisible();
  const activity = panel.locator("[data-conversation-activity]");
  await expect(activity).toHaveAttribute(
    "data-conversation-activity",
    "sending",
  );
  await expect(activity).toBeVisible();
  const activityElement = await activity.elementHandle();
  const composer = panel.locator("[data-chat-composer='true']");
  await expect(composer.locator("[data-conversation-activity]")).toHaveCount(0);
  const messageBounds = await historyRows(panel, text).boundingBox();
  const activityBounds = await activity.boundingBox();
  const composerBounds = await composer.boundingBox();
  expect(activityBounds!.y).toBeGreaterThanOrEqual(
    messageBounds!.y + messageBounds!.height,
  );
  expect(activityBounds!.y + activityBounds!.height).toBeLessThanOrEqual(
    composerBounds!.y,
  );
  await panel.screenshot({
    path: testInfo.outputPath("conversation-sending.png"),
  });
  const echoElement = await historyRows(panel, text).elementHandle();
  await expect(editor).toHaveText("");
  await expect(editor).toBeFocused();
  expect(await editorHandle!.evaluate((element) => element.isConnected)).toBe(
    true,
  );
  await expect.poll(() => submissions.length).toBe(1);
  await editor.press("Enter");
  expect(submissions).toEqual([text]);

  const newerDraft = "A new draft typed before acknowledgement";
  await editor.fill(newerDraft);
  await observeMessage(page, text);
  const response = page.waitForResponse((item) =>
    item.url().endsWith("/analysis-session/follow-up"),
  );
  acknowledgement.resolve();
  await (await response).finished();
  await expect(historyRows(panel, text)).toHaveCount(1);
  await expect(editor).toHaveText(newerDraft);
  await expect(editor).toBeFocused();
  // The response already carries the real Run state; a slower conversation
  // stream must not leave the composer idle between acknowledgement and history.
  await expect(
    panel.getByRole("button", { name: "Stop", exact: true }),
  ).toBeVisible();
  await expect(
    panel.locator("[data-conversation-activity='running']"),
  ).toBeVisible();
  expect(
    await activityElement!.evaluate((element) => element.isConnected),
  ).toBe(true);
  await panel.screenshot({
    path: testInfo.outputPath("conversation-running.png"),
  });
  await expect(
    panel.getByRole("button", { name: "Send", exact: true }),
  ).toHaveCount(0);

  api.publishUserMessage("analysis-session", text, "new-empty-turn");
  await expect(historyRows(panel, text)).toHaveCount(1);
  expect(
    await echoElement!.evaluate((element) => element.isConnected),
    "Acknowledgement should retain the same message seat",
  ).toBe(true);
  await expect(
    panel.getByRole("button", { name: "Stop", exact: true }),
  ).toBeVisible();
  const observation = await observedMessage(page);
  expect(new Set(observation.userRows), JSON.stringify(observation)).toEqual(
    new Set([1]),
  );
  expect(new Set(observation.frames), JSON.stringify(observation)).toEqual(
    new Set([1]),
  );
  expect(submissions).toEqual([text]);
});

test("canonical history arriving before HTTP acknowledgement replaces only its own echo and preserves an older identical message", async ({
  page,
}) => {
  const api = await mockWorkflowConversations(page);
  const text = "Repeat exactly this same user message";
  api.publishUserMessage(
    "analysis-session",
    text,
    "older-identical-turn",
    "succeeded",
  );
  const acknowledgement = deferred();
  await page.route(
    "**/api/sessions/analysis-session/follow-up",
    async (route) => {
      await acknowledgement.promise;
      await route.fulfill({
        json: {
          success: true,
          data: api.acknowledgement("analysis-session", "new-identical-turn"),
        },
      });
    },
  );
  const { panel, editor } = await openConversation(page);
  await expect(historyRows(panel, text)).toHaveCount(1);
  await editor.fill(text);
  await panel.getByRole("button", { name: "Send", exact: true }).click();
  await expect(historyRows(panel, text)).toHaveCount(2);
  await expect(editor).toHaveText("");
  await observeMessage(page, text);

  api.publishUserMessage("analysis-session", text, "new-identical-turn");
  await expect(
    panel.getByRole("button", { name: "Stop", exact: true }),
  ).toBeVisible();
  await expect(historyRows(panel, text)).toHaveCount(2);
  const response = page.waitForResponse((item) =>
    item.url().endsWith("/analysis-session/follow-up"),
  );
  acknowledgement.resolve();
  await (await response).finished();
  await expect(historyRows(panel, text)).toHaveCount(2);
  const observation = await observedMessage(page);
  expect(new Set(observation.userRows), JSON.stringify(observation)).toEqual(
    new Set([2]),
  );
  expect(new Set(observation.frames), JSON.stringify(observation)).toEqual(
    new Set([2]),
  );
  await expect(panel).toContainText("Original reply for analysis-session");
});

test("failed sends release the echo, preserve newer typing and restore an untouched retry draft", async ({
  page,
}) => {
  await mockWorkflowConversations(page);
  const firstFailure = deferred();
  const retryFailure = deferred();
  const submissions: string[] = [];
  await page.route(
    "**/api/sessions/analysis-session/follow-up",
    async (route) => {
      submissions.push(route.request().postDataJSON().prompt);
      await (submissions.length === 1
        ? firstFailure.promise
        : retryFailure.promise);
      await route.fulfill({
        status: 500,
        json: { success: false, message: "The fixture rejected this send" },
      });
    },
  );
  const { panel, editor } = await openConversation(page);
  const first = "Submitted before a newer draft";
  const newer = "The newer draft wins over failed-send restoration";
  await editor.fill(first);
  await expect(
    panel.getByRole("button", { name: "Send", exact: true }),
  ).toBeEnabled();
  await editor.press("Enter");
  await expect(historyRows(panel, first)).toHaveCount(1);
  await expect(editor).toHaveText("");
  await editor.fill(newer);
  firstFailure.resolve();
  await expect(panel).toContainText("The fixture rejected this send");
  await expect(historyRows(panel, first)).toHaveCount(0);
  await expect(panel.locator("[data-conversation-activity]")).toHaveCount(0);
  await expect(editor).toHaveText(newer);

  await panel.getByRole("button", { name: "Send", exact: true }).click();
  await expect(historyRows(panel, newer)).toHaveCount(1);
  await expect(editor).toHaveText("");
  await expect.poll(() => submissions.length).toBe(2);
  retryFailure.resolve();
  await expect(editor).toHaveText(newer);
  await expect(historyRows(panel, newer)).toHaveCount(0);
  expect(submissions).toEqual([first, newer]);
});

test("a late failed send stays with its original conversation and recovers when that conversation is reopened", async ({
  page,
}) => {
  await mockWorkflowConversations(page);
  const failure = deferred();
  await page.route(
    "**/api/sessions/analysis-session/follow-up",
    async (route) => {
      await failure.promise;
      await route.fulfill({
        status: 500,
        json: {
          success: false,
          message: "Failure on the original conversation",
        },
      });
    },
  );
  const { panel, editor } = await openConversation(page);
  const original = "Recover this original conversation draft";
  await editor.fill(original);
  await expect(
    panel.getByRole("button", { name: "Send", exact: true }),
  ).toBeEnabled();
  await editor.press("Enter");
  await expect(historyRows(panel, original)).toHaveCount(1);
  const selector = page.getByTestId("workflow-conversation-work-selector");
  await selector.selectOption("subtask:visual-batch:research");
  await expect(panel).toHaveAttribute("data-session-id", "research-session");
  await expect(historyRows(panel, original)).toHaveCount(0);
  const researchDraft = "Independent research draft";
  await editor.fill(researchDraft);
  failure.resolve();
  await expect(editor).toHaveText(researchDraft);
  await expect(panel).not.toContainText("Failure on the original conversation");

  await selector.selectOption("subtask:visual-batch:independent");
  await expect(panel).toHaveAttribute("data-session-id", "analysis-session");
  await expect(editor).toHaveText(original);
  await expect(historyRows(panel, original)).toHaveCount(0);
  await expect(panel).toContainText("Failure on the original conversation");
});

test("closing and reopening a pending conversation retains its single echo without sending twice", async ({
  page,
}) => {
  const api = await mockWorkflowConversations(page);
  const acknowledgement = deferred();
  let submissions = 0;
  await page.route(
    "**/api/sessions/analysis-session/follow-up",
    async (route) => {
      submissions += 1;
      await acknowledgement.promise;
      await route.fulfill({
        json: {
          success: true,
          data: api.acknowledgement(
            "analysis-session",
            "reopened-pending-turn",
          ),
        },
      });
    },
  );
  const { panel, editor } = await openConversation(page);
  const text = "Visible again when the pending conversation is reopened";
  await editor.fill(text);
  await expect(
    panel.getByRole("button", { name: "Send", exact: true }),
  ).toBeEnabled();
  await editor.press("Enter");
  await expect(historyRows(panel, text)).toHaveCount(1);
  await panel
    .getByRole("button", { name: "Close Node details", exact: true })
    .click();
  await expect(panel).toHaveCount(0);
  await page.getByTestId("workflow-run-node-build").click();
  await expect(historyRows(panel, text)).toHaveCount(1);
  await expect(editor).toHaveText("");
  await editor.press("Enter");
  expect(submissions).toBe(1);
  const newer = "A draft written in the replacement editor";
  await editor.fill(newer);
  acknowledgement.resolve();
  api.publishUserMessage("analysis-session", text, "reopened-pending-turn");
  await expect(historyRows(panel, text)).toHaveCount(1);
  await expect(editor).toHaveText(newer);
  expect(submissions).toBe(1);
});

test("streaming a long reply preserves typing, history DOM and the draft after reopening", async ({
  page,
}) => {
  const api = await mockWorkflowConversations(page, {
    initialHistorySize: 100,
    initialRunStatus: "running",
  });
  const { panel, editor } = await openConversation(page);
  await expect
    .poll(() => api.hasConversationConnection("analysis-session"))
    .toBe(true);
  const oldRow = historyRows(panel, "History message 100.");
  await expect(oldRow).toHaveCount(1);
  const oldElement = await oldRow.elementHandle();
  const editorElement = await editor.elementHandle();
  await editor.fill("Next request: ");
  api.startAssistantReply("analysis-session", "## Live response\n\n");
  // Query the real history rather than relying on the renderer's implementation.
  const liveRow = historyRows(panel, "Live response");
  await expect(liveRow).toHaveCount(1);
  const typed = "keep my draft while the agent replies";
  for (let index = 0; index < typed.length; index++) {
    api.appendAssistantReply(
      "analysis-session",
      `Streaming section ${index}: **retained text** and \`src/example.ts\`.\n\n`,
    );
    await editor.pressSequentially(typed[index]);
  }
  await expect(liveRow).toContainText(`Streaming section ${typed.length - 1}`);
  await expect(editor).toHaveText(`Next request: ${typed}`);
  await expect(editor).toBeFocused();
  expect(await editorElement!.evaluate((element) => element.isConnected)).toBe(
    true,
  );
  expect(await oldElement!.evaluate((element) => element.isConnected)).toBe(
    true,
  );
  await expect(
    panel.getByRole("button", { name: "Stop", exact: true }),
  ).toBeVisible();

  api.finishAssistantReply("analysis-session");
  await expect(
    panel.getByRole("button", { name: "Send", exact: true }),
  ).toBeEnabled();
  await expect(liveRow).toHaveCount(1);
  await expect(editor).toHaveText(`Next request: ${typed}`);
  await panel
    .getByRole("button", { name: "Close Node details", exact: true })
    .click();
  await expect(panel).toHaveCount(0);
  await page.getByTestId("workflow-run-node-build").click();
  await expect(editor).toHaveText(`Next request: ${typed}`);
  await expect(liveRow).toHaveCount(1);
  await expect(liveRow).toContainText(`Streaming section ${typed.length - 1}`);
  expect(
    api.requests.filter((request) => request.includes("/follow-up")),
  ).toEqual([]);
});

for (const withRepository of [false, true]) {
  test(`composer polls Git only for a confirmed repository (${withRepository})`, async ({
    page,
  }) => {
    await page.clock.install();
    await mockWorkflowConversations(page);
    if (withRepository) {
      await page.route("**/api/workspaces/visual-workspace/repos", (route) =>
        route.fulfill({
          json: {
            success: true,
            data: [
              {
                id: "fixture-repo",
                name: "fixture",
                path: "C:/fixture",
                target_branch: "main",
              },
            ],
          },
        }),
      );
    }
    let gitRequests = 0;
    await page.route(
      "**/api/workspaces/visual-workspace/git/status",
      (route) => {
        gitRequests += 1;
        return route.fulfill({ json: { success: true, data: [] } });
      },
    );
    const { editor } = await openConversation(page);
    if (withRepository) await expect.poll(() => gitRequests).toBeGreaterThan(0);
    await editor.fill("Keep the composer ready");
    await page.clock.fastForward(5_100);
    await expect(editor).toHaveText("Keep the composer ready");
    if (withRepository) await expect.poll(() => gitRequests).toBeGreaterThan(1);
    else expect(gitRequests).toBe(0);
  });
}

for (const historySize of [0, 100, 500]) {
  test(`submission feedback paints without waiting for HTTP with ${historySize} history blocks`, async ({
    page,
  }) => {
    const api = await mockWorkflowConversations(page, {
      initialHistory: historySize > 0,
      initialHistorySize: historySize,
    });
    const acknowledgement = deferred();
    await page.route(
      "**/api/sessions/analysis-session/follow-up",
      async (route) => {
        await acknowledgement.promise;
        await route.fulfill({
          json: {
            success: true,
            data: api.acknowledgement("analysis-session", "measured-send"),
          },
        });
      },
    );
    const { panel, editor } = await openConversation(page);
    const text = `Immediate submission with ${historySize} stable blocks`;
    await editor.fill(text);
    await expect(
      panel.getByRole("button", { name: "Send", exact: true }),
    ).toBeEnabled();
    await page.evaluate((text) => {
      const panel = document.querySelector(
        '[data-testid="workflow-conversation-panel"]',
      )!;
      const editor = panel.querySelector(
        '[data-chatbox-container] [data-lexical-editor="true"]',
      )!;
      const timing = {
        start: 0,
        committed: 0,
        frame: 0,
        events: [] as Array<{ name: string; duration: number }>,
      };
      const scope = window as Window & { sendFeedbackTiming?: typeof timing };
      scope.sendFeedbackTiming = timing;
      const hasEcho = () =>
        [...panel.querySelectorAll("[data-semantic-key]")].some((row) =>
          row.textContent?.includes(text),
        );
      const observer = new MutationObserver(() => {
        if (!timing.start || !hasEcho()) return;
        timing.committed ||= performance.now();
        observer.disconnect();
      });
      observer.observe(panel, {
        subtree: true,
        childList: true,
        characterData: true,
      });
      const eventObserver = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          if (entry.name === "keydown")
            timing.events.push({ name: entry.name, duration: entry.duration });
        }
      });
      eventObserver.observe({ type: "event", durationThreshold: 16 });
      editor.addEventListener(
        "keydown",
        () => {
          timing.start = performance.now();
          const sample = () => {
            if (hasEcho()) {
              timing.frame = performance.now();
              // Read Event Timing after the browser has presented the update.
              requestAnimationFrame(() => eventObserver.disconnect());
            } else requestAnimationFrame(sample);
          };
          requestAnimationFrame(sample);
        },
        { once: true, capture: true },
      );
    }, text);
    await editor.press("Enter");
    await expect(historyRows(panel, text)).toHaveCount(1);
    await expect(editor).toHaveText("");
    await expect
      .poll(() =>
        page.evaluate(() => {
          const scope = window as Window & {
            sendFeedbackTiming?: { frame: number };
          };
          return scope.sendFeedbackTiming?.frame ?? 0;
        }),
      )
      .toBeGreaterThan(0);
    const timing = await page.evaluate((historySize) => {
      const scope = window as Window & {
        sendFeedbackTiming?: {
          start: number;
          committed: number;
          frame: number;
          events: Array<{ name: string; duration: number }>;
        };
      };
      const timing = scope.sendFeedbackTiming!;
      return {
        historySize,
        domFeedbackMs: timing.committed - timing.start,
        nextFrameSampleMs: timing.frame - timing.start,
        eventTiming: timing.events,
      };
    }, historySize);
    await test.info().attach("submission-feedback-timing", {
      body: JSON.stringify(timing, null, 2),
      contentType: "application/json",
    });
    // Frame sampling is an observation, not an exact presentation timestamp.
    // Keep a generous regression bound for the development browser harness.
    expect(timing.domFeedbackMs).toBeGreaterThanOrEqual(0);
    expect(timing.domFeedbackMs).toBeLessThan(500);
    await expect(editor).toBeFocused();
    acknowledgement.resolve();
  });
}

test("reading upwards interrupts send follow and a late acknowledgement keeps that reading position", async ({
  page,
}) => {
  const api = await mockWorkflowConversations(page, {
    initialHistorySize: 100,
  });
  const acknowledgement = deferred();
  await page.route(
    "**/api/sessions/analysis-session/follow-up",
    async (route) => {
      await acknowledgement.promise;
      await route.fulfill({
        json: {
          success: true,
          data: api.acknowledgement("analysis-session", "reading-send"),
        },
      });
    },
  );
  const { panel, editor } = await openConversation(page);
  const text = "Keep reading history after this submission";
  await editor.fill(text);
  await editor.press("Enter");
  await expect(historyRows(panel, text)).toBeInViewport();
  const scroll = panel.locator('[style*="overflow-anchor: none"]');
  await scroll.hover();
  await page.mouse.wheel(0, -550);
  await expect
    .poll(() =>
      scroll.evaluate(
        (element) =>
          element.scrollHeight - element.clientHeight - element.scrollTop,
      ),
    )
    .toBeGreaterThan(200);
  const anchor = await scroll.evaluate(async (element) => {
    // Let newly visible virtual rows finish their first size measurements.
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
    const top = element.getBoundingClientRect().top;
    const row = [
      ...element.querySelectorAll<HTMLElement>("[data-semantic-key]"),
    ].find((row) => row.getBoundingClientRect().bottom > top)!;
    return {
      key: row.dataset.semanticKey!,
      top: row.getBoundingClientRect().top - top,
    };
  });
  const response = page.waitForResponse((item) =>
    item.url().endsWith("/analysis-session/follow-up"),
  );
  acknowledgement.resolve();
  await (await response).finished();
  api.publishUserMessage("analysis-session", text, "reading-send");
  await expect(
    panel.getByRole("button", { name: "Stop", exact: true }),
  ).toBeVisible();
  await expect
    .poll(async () =>
      scroll.evaluate((element, anchor) => {
        const row = [
          ...element.querySelectorAll<HTMLElement>("[data-semantic-key]"),
        ].find((row) => row.dataset.semanticKey === anchor.key);
        return row
          ? Math.abs(
              row.getBoundingClientRect().top -
                element.getBoundingClientRect().top -
                anchor.top,
            )
          : Infinity;
      }, anchor),
    )
    .toBeLessThan(4);
});
