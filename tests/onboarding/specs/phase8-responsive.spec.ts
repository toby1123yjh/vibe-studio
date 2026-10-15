import { expect, test } from '@playwright/test';

const viewports = [375, 768, 1024, 1440];

async function expectContained(page: import('@playwright/test').Page) {
  const overflow = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);
  expect(await page.locator('main').count()).toBe(1);
  expect(await page.locator('h1').count()).toBe(1);
}

for (const width of viewports) {
  test(`Landing remains contained and keyboard operable at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/?page=landing');

    await expect(page.getByRole('heading', { level: 1 })).toHaveText(
      'Set up Vibe Studio'
    );
    await expect(page.getByRole('heading', { level: 2 })).toHaveCount(3);
    await expectContained(page);

    const continueButton = page.getByRole('button', { name: 'Continue' });
    await continueButton.focus();
    await expect(continueButton).toBeFocused();
    if (width === 375) {
      expect(await continueButton.boundingBox()).toMatchObject({ height: 44 });
    }
  });
}

test('Landing exposes loading semantics', async ({ page }) => {
  await page.goto('/?page=landing&mode=loading');
  await expect(page.locator('[data-state="loading"]')).toHaveAttribute(
    'role',
    'status'
  );
  await expect(page.locator('main')).toHaveCount(1);
});

for (const width of viewports) {
  test(`Sign-in remains contained and keyboard operable at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/?page=signin');

    await expect(page.getByRole('heading', { level: 1 })).toHaveText(
      'Sign in to Vibe Studio'
    );
    await expectContained(page);

    const moreOptions = page.getByRole('button', { name: 'More options' });
    await moreOptions.focus();
    await expect(moreOptions).toBeFocused();
    if (width === 375) {
      expect(await moreOptions.boundingBox()).toMatchObject({ height: 44 });
    }
  });
}

test('Sign-in keeps an explicit empty state when no methods are configured', async ({
  page,
}) => {
  await page.goto('/?page=signin&mode=signin-empty');
  await expect(page.locator('[data-state="empty"]')).toContainText(
    'No sign-in methods configured'
  );
  await expect(
    page.getByRole('button', { name: 'More options' })
  ).toBeVisible();
});

test('Sign-in comparison stays locally scrollable on a narrow viewport', async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 640 });
  await page.goto('/?page=signin');

  const moreOptions = page.getByRole('button', { name: 'More options' });
  await moreOptions.focus();
  await moreOptions.press('Enter');

  await expect(page.getByRole('table')).toBeVisible();
  await expect(
    page.getByRole('button', {
      name: 'I understand, continue without signing in',
    })
  ).toBeVisible();
  await expectContained(page);
});

test('Sign-in exposes recovery for discovery errors', async ({ page }) => {
  await page.goto('/?page=signin&mode=signin-error');
  const error = page.locator('[data-state="error"]');
  await expect(error).toContainText('Sign-in methods unavailable');
  const retry = error.getByRole('button', { name: 'Retry' });
  await expect(retry).toBeVisible();
  await retry.focus();
  await expect(retry).toBeFocused();
});

test('Sign-in retains cached methods when refresh degrades', async ({
  page,
}) => {
  await page.goto('/?page=signin&mode=signin-degraded');
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
  await expect(page.locator('[data-state="degraded"]')).toContainText(
    'Sign-in methods may be out of date'
  );
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
});
