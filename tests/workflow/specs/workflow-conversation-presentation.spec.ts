import { expect, test } from '@playwright/test';
import {
  mockWorkflowConversations,
  mockWorkflowFileContext,
} from '../support/conversation-api';

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.route('**/api/**', (route) => route.abort());
  await page.route('https://fonts.googleapis.com/**', (route) => route.abort());
  await mockWorkflowFileContext(page);
});

test('shared conversation disclosures stay readable and keep their state when resized and completed', async ({
  page,
}, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const api = await mockWorkflowConversations(page, {
    withPresentationBlocks: true,
    initialRunStatus: 'running',
  });
  await page.goto('/visual.html?mode=file-run');
  await page.getByTestId('workflow-run-node-build').click();
  const panel = page.getByTestId('workflow-conversation-panel');
  const editor = panel.locator(
    '[data-chatbox-container] [data-lexical-editor="true"]'
  );
  await expect(editor).toBeVisible();
  await editor.fill('Continue after reviewing the details');
  const editorHandle = await editor.elementHandle();
  const reasoning = panel.getByRole('button', { name: /Thinking/ });
  const group = panel.getByRole('button', { name: /Tool activity/ });
  await expect(reasoning).toHaveAttribute('aria-expanded', 'false');
  await expect(reasoning).toContainText('Inspect project context');
  const activity = reasoning.locator('.vk-activity-text');
  await expect(activity).toHaveCSS('animation-name', 'activity-text-wave');
  const activityGeometry = await activity.evaluate((element) => {
    const text = document.createRange();
    text.selectNodeContents(element);
    return {
      width: element.getBoundingClientRect().width,
      textWidth: text.getBoundingClientRect().width,
    };
  });
  // The highlight must traverse the label, not an empty flex column.
  expect(activityGeometry.width).toBeLessThanOrEqual(
    activityGeometry.textWidth + 1
  );
  await expect(reasoning).toHaveCSS('transition-duration', '0.12s');
  await expect(group).toHaveAttribute('aria-expanded', 'false');
  await expect(group).toContainText('Search current directory');
  await expect(group).toContainText('Tool error');
  await expect(group).not.toContainText(/\d+ tool calls?|\d+\/\d+|completed/);
  await expect(
    panel.getByText('Workspace notes are retained in this tool result.')
  ).toHaveCount(0);
  await expect(
    panel.getByText('The full diagnostic is available when needed.')
  ).toHaveCount(0);
  expect(api.requests.some((request) => request.includes('/blocks/'))).toBe(
    false
  );

  const user = panel.locator('[data-chat-message="user"]');
  await expect(user).toContainText('Please review this workspace.');
  const initialPanelBox = await panel.boundingBox();
  const userBox = await user.boundingBox();
  expect(initialPanelBox!.width).toBeGreaterThanOrEqual(898);
  expect(userBox!.width).toBeLessThan(initialPanelBox!.width * 0.7);
  await page.screenshot({ path: testInfo.outputPath('conversation-900.png') });
  await reasoning.focus();
  await reasoning.press('Enter');
  await group.focus();
  await group.press('Space');
  const read = panel.getByRole('button', {
    name: 'Read workspace notes',
    exact: true,
  });
  const search = panel.getByRole('button', {
    name: 'Search current directory',
    exact: true,
  });
  await read.click();
  await search.click();
  await expect(
    panel.getByText('Parameters', { exact: true }).first()
  ).toBeVisible();
  await expect(
    panel.getByText('Workspace notes are retained in this tool result.')
  ).toBeVisible();
  await expect(
    panel.getByText('The directory search has produced a partial result.')
  ).toBeVisible();
  const notice = panel.getByRole('button', {
    name: 'An optional integration is unavailable.',
    exact: true,
  });
  await notice.click();
  await expect(
    panel.getByText('The full diagnostic is available when needed.', {
      exact: false,
    })
  ).toBeVisible();
  const metadata = panel.getByRole('button', { name: /Agent event details/ });
  await metadata.click();
  await expect(
    panel.getByText('"diagnostic": "Retained native metadata"', {
      exact: false,
    })
  ).toBeVisible();

  await panel.getByRole('separator').press('Home');
  await expect
    .poll(async () => Math.round((await panel.boundingBox())!.width))
    .toBe(480);
  await expect(reasoning).toHaveAttribute('aria-expanded', 'true');
  await expect(read).toHaveAttribute('aria-expanded', 'true');
  await expect(search).toHaveAttribute('aria-expanded', 'true');
  await expect
    .poll(() =>
      panel.evaluate((element) => element.scrollWidth <= element.clientWidth)
    )
    .toBe(true);
  const rows = await panel
    .locator('[data-chat-disclosure-row]')
    .evaluateAll((elements) =>
      elements.map((element) => {
        const row = element.getBoundingClientRect();
        const icon = element.firstElementChild!.getBoundingClientRect();
        return {
          rowHeight: row.height,
          iconWidth: icon.width,
          offset: Math.abs((icon.top + icon.bottom - row.top - row.bottom) / 2),
        };
      })
    );
  const expectedIconWidth = await page.evaluate(
    () => parseFloat(getComputedStyle(document.documentElement).fontSize) * 1.25
  );
  expect(rows.length).toBeGreaterThanOrEqual(7);
  for (const row of rows) {
    expect(row.rowHeight).toBeGreaterThanOrEqual(32);
    expect(row.iconWidth).toBe(expectedIconWidth);
    expect(row.offset).toBeLessThanOrEqual(1);
  }
  await page.screenshot({ path: testInfo.outputPath('conversation-480.png') });

  const request = api.requests.find((value) =>
    /GET \/api\/sessions\/[^/]+\/conversation$/.test(value)
  );
  expect(request).toBeTruthy();
  api.setRunStatus(request!.split('/')[3], 'cancelled');
  await expect(reasoning).not.toHaveAttribute('data-active', 'true');
  await expect(search).not.toHaveAttribute('data-active', 'true');
  await expect(search).toHaveAccessibleDescription('Result not confirmed');
  await expect(reasoning).toHaveAttribute('aria-expanded', 'true');
  await expect(search).toHaveAttribute('aria-expanded', 'true');
  await expect(editor).toHaveText('Continue after reviewing the details');
  expect(await editorHandle!.evaluate((element) => element.isConnected)).toBe(
    true
  );
  expect(errors).toEqual([]);
});

test('process disclosures honor reduced motion without hiding running status', async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await mockWorkflowConversations(page, {
    withPresentationBlocks: true,
    initialRunStatus: 'running',
  });
  await page.goto('/visual.html?mode=file-run');
  await page.getByTestId('workflow-run-node-build').click();
  const panel = page.getByTestId('workflow-conversation-panel');
  const group = panel.getByRole('button', { name: /Tool activity/ });
  await expect(group).toHaveAttribute('data-active', 'true');
  await expect(group).toContainText('Running');
  expect(
    await group.evaluate((element) =>
      parseFloat(getComputedStyle(element).transitionDuration)
    )
  ).toBeLessThanOrEqual(0.001);
  await expect(group.locator('.vk-activity-text')).toHaveCSS(
    'animation-name',
    'none'
  );
  const caret = group.locator(':scope > svg').last();
  expect(
    await caret.evaluate((element) =>
      parseFloat(getComputedStyle(element).transitionDuration)
    )
  ).toBeLessThanOrEqual(0.001);
  await group.press('Enter');
  await expect(group).toHaveAttribute('aria-expanded', 'true');
  await expect(
    panel.getByRole('button', { name: 'Search current directory', exact: true })
  ).toHaveAttribute('data-active', 'true');
});
