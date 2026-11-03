import { expect, test, type Locator } from '@playwright/test';
import {
  mockWorkflowConversations,
  mockWorkflowFileContext,
} from '../support/conversation-api';

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.route('**/api/**', (route) => route.abort());
  await page.route('https://fonts.googleapis.com/**', (route) => route.abort());
  await mockWorkflowFileContext(page);
});

async function readingAnchor(panel: Locator) {
  return panel
    .locator('[data-semantic-key]')
    .first()
    .evaluate((row) => {
      const scroller = row.closest('.overflow-y-auto') as HTMLElement;
      const top = scroller.getBoundingClientRect().top;
      const visible = [
        ...scroller.querySelectorAll<HTMLElement>('[data-semantic-key]'),
      ].find((item) => item.getBoundingClientRect().bottom > top);
      if (!visible) throw new Error('No visible conversation row');
      return {
        key: visible.dataset.semanticKey!,
        offset: visible.getBoundingClientRect().top - top,
        scrollTop: scroller.scrollTop,
      };
    });
}

test('a reading gesture immediately takes over from disclosure anchoring', async ({
  page,
}) => {
  await mockWorkflowConversations(page, {
    initialHistorySize: 70,
    withPresentationBlocks: true,
  });
  await page.goto('/visual.html?mode=file-run');
  await page.getByTestId('workflow-run-node-build').click();
  const panel = page.getByTestId('workflow-conversation-panel');
  const group = panel.getByRole('button', { name: /Tool activity/ });
  await expect(group).toBeVisible();
  await group.scrollIntoViewIfNeeded();
  // Keep the gesture inside the disclosure's anchoring window. The wheel
  // expresses the user's intent; scrolling models its browser default action.
  const target = await group.evaluate(async (button) => {
    const scroller = button.closest('.overflow-y-auto') as HTMLElement;
    (button as HTMLButtonElement).click();
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => resolve())
    );
    scroller.dispatchEvent(
      new WheelEvent('wheel', { deltaY: -160, bubbles: true })
    );
    scroller.scrollTop -= 160;
    const top = scroller.scrollTop;
    await new Promise((resolve) => setTimeout(resolve, 320));
    return { before: top, after: scroller.scrollTop };
  });
  expect(target.before).toBeGreaterThan(0);
  expect(Math.abs(target.after - target.before)).toBeLessThan(3);
});

test('reopening at a narrower width restores the visible message and keeps the draft', async ({
  page,
}) => {
  await mockWorkflowConversations(page, { initialHistorySize: 120 });
  await page.goto('/visual.html?mode=file-run');
  await page.getByTestId('workflow-run-node-build').click();
  const panel = page.getByTestId('workflow-conversation-panel');
  const editor = panel.locator(
    '[data-chatbox-container] [data-lexical-editor="true"]'
  );
  await expect(editor).toBeEditable();
  await editor.fill('Keep this draft while I read history');
  await panel
    .locator('[data-semantic-key]')
    .first()
    .evaluate((row) => {
      const scroller = row.closest('.overflow-y-auto') as HTMLElement;
      scroller.dispatchEvent(
        new WheelEvent('wheel', { deltaY: -800, bubbles: true })
      );
      scroller.scrollTop = scroller.scrollHeight * 0.45;
    });
  // Wait for the virtual window and its measured rows to settle before capture.
  await expect
    .poll(async () => (await readingAnchor(panel)).scrollTop)
    .toBeGreaterThan(500);
  const anchor = await readingAnchor(panel);
  await panel
    .getByRole('button', { name: 'Close Node details', exact: true })
    .click();
  await expect(panel).toHaveCount(0);
  await page.setViewportSize({ width: 760, height: 900 });
  await page.getByTestId('workflow-run-node-build').click();
  await expect(editor).toHaveText('Keep this draft while I read history');
  await expect
    .poll(async () => (await readingAnchor(panel)).key)
    .toBe(anchor.key);
  await expect
    .poll(async () =>
      Math.abs((await readingAnchor(panel)).offset - anchor.offset)
    )
    .toBeLessThan(3);
});
