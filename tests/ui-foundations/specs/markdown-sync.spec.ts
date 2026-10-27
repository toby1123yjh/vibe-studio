import { expect, test } from '@playwright/test';
import type {} from '../fixture/src/markdown-sync';

test('streamed read-only Markdown renders without exporting it back on every update', async ({
  page,
}) => {
  await page.goto('/markdown-sync.html');
  const editor = page.getByLabel('Markdown fixture');
  await expect(editor.locator('strong')).toHaveText('reply');
  await expect(editor).toHaveAttribute('contenteditable', 'false');
  const initialEditor = await editor.elementHandle();

  for (let index = 1; index <= 5; index += 1) {
    await page.evaluate((index) => {
      const paragraphs = Array.from(
        { length: 150 },
        (_, paragraph) => `Paragraph ${paragraph} **complete** with \`file.ts\``
      ).join('\n\n');
      window.markdownSyncFixture.replace(`${paragraphs}\n\nTail ${index}`);
    }, index);
    await expect(editor).toContainText(`Tail ${index}`);
  }

  const metrics = await page.evaluate(() => window.markdownSyncFixture.metrics);
  expect(metrics.observed).toBeGreaterThan(0);
  expect(metrics.exports).toBe(0);
  expect(metrics.changes).toEqual([]);
  expect(await initialEditor!.evaluate((element) => element.isConnected)).toBe(
    true
  );
});

test('selection and controlled value updates skip serialization while user edits still synchronize', async ({
  page,
}) => {
  await page.goto('/markdown-sync.html?editable');
  const editor = page.getByLabel('Markdown fixture');
  await expect(editor.locator('strong')).toHaveText('reply');
  await expect(editor).toHaveAttribute('contenteditable', 'true');
  await editor.click();
  await page.evaluate(() => window.markdownSyncFixture.reset());
  await page.evaluate(() => window.markdownSyncFixture.selectStart());
  await expect
    .poll(() =>
      page.evaluate(() => window.markdownSyncFixture.metrics.observed)
    )
    .toBeGreaterThan(0);
  expect(
    await page.evaluate(() => window.markdownSyncFixture.metrics.exports)
  ).toBe(0);

  await page.evaluate(() =>
    window.markdownSyncFixture.replace('Controlled **replacement**')
  );
  await expect(editor.locator('strong')).toHaveText('replacement');
  expect(
    await page.evaluate(() => window.markdownSyncFixture.metrics.changes)
  ).toEqual([]);

  await editor.press('End');
  await editor.pressSequentially(' edited');
  await expect
    .poll(() =>
      page.evaluate(() => window.markdownSyncFixture.metrics.changes.at(-1))
    )
    .toContain('edited');
  expect(
    await page.evaluate(() => window.markdownSyncFixture.metrics.exports)
  ).toBeGreaterThan(0);

  await page.evaluate(() => {
    window.markdownSyncFixture.setEditable(false);
  });
  await expect(editor).toHaveAttribute('contenteditable', 'false');
  await page.evaluate(() => {
    window.markdownSyncFixture.reset();
    window.markdownSyncFixture.replace('Final **answer**');
  });
  await expect(editor.locator('strong')).toHaveText('answer');
  expect(
    await page.evaluate(() => window.markdownSyncFixture.metrics.exports)
  ).toBe(0);
});
