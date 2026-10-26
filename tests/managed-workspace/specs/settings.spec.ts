import { expect, test } from '@playwright/test';
import type { settings } from '../fixture/settings-mocks';

declare global {
  interface Window {
    settingsFixture: typeof settings;
  }
}
test.beforeEach(async ({ page }) => {
  await page.goto('/?settings=1');
  await expect(page.getByTestId('ready')).toHaveText('fixture ready');
});

test('default null displays automatic placeholder; manual edit, clear and reset use nullable draft', async ({
  page,
}) => {
  const input = page.getByRole('textbox', {
    name: 'Default session directory',
  });
  await expect(input).toHaveValue('');
  await expect(input).toHaveAttribute(
    'placeholder',
    'Automatic (app data / workspaces)'
  );
  await expect(
    page.getByRole('button', { name: 'Reset', exact: true })
  ).toBeDisabled();
  await input.fill('/custom');
  await expect(page.getByTestId('value')).toHaveText('"/custom"');
  await input.fill('');
  await expect(page.getByTestId('value')).toHaveText('null');
  await input.fill('/other');
  await page.getByRole('button', { name: 'Reset', exact: true }).click();
  await expect(page.getByTestId('value')).toHaveText('null');
});

test('picker addresses selected Host and same-tick clicks open one picker', async ({
  page,
}) => {
  await page
    .getByRole('button', { name: 'Browse', exact: true })
    .evaluate((button: HTMLButtonElement) => {
      button.click();
      button.click();
    });
  expect(await page.evaluate(() => window.settingsFixture.calls)).toEqual([
    { value: '', hostId: 'host-a', title: 'Default session directory' },
  ]);
  await page.evaluate(() => window.settingsFixture.resolve('/selected'));
  await expect(page.getByTestId('value')).toHaveText('"/selected"');
});

for (const { label, rawPath, displayPath } of [
  {
    label: 'drive',
    rawPath: String.raw`\\?\D:\workspaces`,
    displayPath: String.raw`D:\workspaces`,
  },
  {
    label: 'UNC',
    rawPath: String.raw`\\?\UNC\server\share\workspaces`,
    displayPath: String.raw`\\server\share\workspaces`,
  },
]) {
  test(`${label} root displays a friendly path without changing the stored draft or picker prefill`, async ({
    page,
  }) => {
    await page.evaluate(
      (value) => window.settingsFixture.update({ value }),
      rawPath
    );
    const input = page.getByRole('textbox', {
      name: 'Default session directory',
    });
    await expect(input).toHaveValue(displayPath);
    await expect(input).toHaveAttribute('title', displayPath);
    expect(await page.evaluate(() => window.settingsFixture.changes)).toEqual(
      []
    );
    await page.getByRole('button', { name: 'Browse', exact: true }).click();
    expect(await page.evaluate(() => window.settingsFixture.calls)).toEqual([
      { value: rawPath, hostId: 'host-a', title: 'Default session directory' },
    ]);
    await page.evaluate(
      (value) => window.settingsFixture.resolve(value),
      rawPath
    );
    expect(await page.evaluate(() => window.settingsFixture.changes)).toEqual([
      rawPath,
    ]);
    await expect(input).toHaveValue(displayPath);
    await input.fill(String.raw`D:\edited-workspaces`);
    expect(await page.evaluate(() => window.settingsFixture.value)).toEqual(
      String.raw`D:\edited-workspaces`
    );
  });
}

for (const condition of [
  'host',
  'roundtrip',
  'degraded',
  'disabled',
  'missing',
] as const) {
  test(`picker result is rejected after ${condition} scope change`, async ({
    page,
  }) => {
    await page.getByRole('button', { name: 'Browse', exact: true }).click();
    await page.evaluate(
      (condition) =>
        window.settingsFixture.update(
          condition === 'host' || condition === 'roundtrip'
            ? { machine: 'b', value: '/host-b' }
            : condition === 'degraded'
              ? { canMutate: false }
              : condition === 'disabled'
                ? { disabled: true }
                : { machine: null }
        ),
      condition
    );
    if (condition === 'roundtrip')
      await page.evaluate(() =>
        window.settingsFixture.update({ machine: 'a', value: '/back-a' })
      );
    await page.evaluate(() => window.settingsFixture.resolve('/stale'));
    expect(await page.evaluate(() => window.settingsFixture.changes)).toEqual(
      []
    );
    await expect(page.getByTestId('value')).not.toHaveText('"/stale"');
  });
}

test('missing machine disables both manual edits and browse without fallback', async ({
  page,
}) => {
  await page.evaluate(() => window.settingsFixture.update({ machine: null }));
  await expect(
    page.getByRole('textbox', { name: 'Default session directory' })
  ).toBeDisabled();
  await expect(
    page.getByRole('button', { name: 'Browse', exact: true })
  ).toBeDisabled();
  expect(await page.evaluate(() => window.settingsFixture.calls)).toEqual([]);
});
