import { expect, test } from '@playwright/test';
import type { draftTransport } from '../fixture/mocks';

declare global {
  interface Window {
    draftTransport: typeof draftTransport;
  }
}

test.beforeEach(({ page }) => {
  page.on('pageerror', (error) => {
    throw error;
  });
});

test('closing flushes the latest draft and reopening before PUT completion recovers it', async ({
  page,
}) => {
  await page.goto('/?draftPersistence');
  const editor = page.getByRole('textbox', { name: 'Draft', exact: true });
  await expect(editor).toBeVisible();
  await editor.fill('First draft');
  await page.getByRole('button', { name: 'Close draft', exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.draftTransport.writes))
    .toEqual([
      {
        scope: 'host-a:persistence-test',
        method: 'PUT',
        message: 'First draft',
      },
    ]);
  await page.getByRole('button', { name: 'Open draft', exact: true }).click();
  await expect(editor).toHaveValue('First draft');
  await editor.fill('Newer draft');
  await page.getByRole('button', { name: 'Close draft', exact: true }).click();
  expect(await page.evaluate(() => window.draftTransport.writes)).toHaveLength(
    1
  );
  await page.evaluate(() => window.draftTransport.release());
  await expect
    .poll(() =>
      page.evaluate(() => window.draftTransport.writes.at(-1)?.message)
    )
    .toBe('Newer draft');
  await page.evaluate(() => window.draftTransport.release());
  await page.getByRole('button', { name: 'Open draft', exact: true }).click();
  await expect(editor).toHaveValue('Newer draft');
});

test('clear, newer input and close in one batch save the newer draft after the old PUT and DELETE', async ({
  page,
}) => {
  await page.goto('/?draftPersistence');
  const editor = page.getByRole('textbox', { name: 'Draft', exact: true });
  await editor.fill('Submitted draft');
  await page.getByRole('button', { name: 'Close draft', exact: true }).click();
  await page.getByRole('button', { name: 'Open draft', exact: true }).click();
  await expect(editor).toHaveValue('Submitted draft');
  await page
    .getByRole('button', {
      name: 'Clear and close with newer input',
      exact: true,
    })
    .click();
  await page.evaluate(() => window.draftTransport.release());
  await expect
    .poll(() => page.evaluate(() => window.draftTransport.writes))
    .toEqual([
      {
        scope: 'host-a:persistence-test',
        method: 'PUT',
        message: 'Submitted draft',
      },
      { scope: 'host-a:persistence-test', method: 'DELETE' },
      {
        scope: 'host-a:persistence-test',
        method: 'PUT',
        message: 'New draft after clear',
      },
    ]);
  await page.evaluate(() => window.draftTransport.release());
  await page.getByRole('button', { name: 'Open draft', exact: true }).click();
  await expect(editor).toHaveValue('New draft after clear');
});
