import { expect, test } from '@playwright/test';

const viewports = [375, 768, 1024, 1440];

async function expectNoOverflow(page: import('@playwright/test').Page) {
  const dimensions = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth);
}

for (const width of viewports) {
  test(`remote login is contained and keyboard-ready at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/?page=login');
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(
      'Sign in to Vibe Studio'
    );
    await expect(page.locator('main')).toHaveCount(1);
    await expectNoOverflow(page);
    const oauth = page.getByRole('button', { name: 'Continue with GitHub' });
    await oauth.focus();
    await expect(oauth).toBeFocused();
    if (width === 375) {
      expect(await oauth.boundingBox()).toMatchObject({ height: 44 });
    }
  });
}

test('remote login distinguishes empty, error, and degraded discovery', async ({
  page,
}) => {
  await page.goto('/?page=login&mode=login-empty');
  await expect(page.locator('[data-state="empty"]')).toContainText(
    'No sign-in methods configured'
  );

  await page.goto('/?page=login&mode=login-error');
  await expect(page.locator('[data-state="error"]')).toContainText(
    'Sign-in methods unavailable'
  );
  await expect(
    page
      .locator('[data-state="error"]')
      .getByRole('button', { name: 'Try again' })
  ).toBeVisible();

  await page.goto('/?page=login&mode=login-degraded');
  await expect(page.locator('[data-state="degraded"]')).toContainText(
    'Sign-in methods may be out of date'
  );
  await expect(
    page.getByRole('button', { name: 'Continue with GitHub' })
  ).toBeVisible();
});

test('invitation loading, error, and ready states keep one landmark', async ({
  page,
}) => {
  await page.goto('/?page=invite&mode=invite-loading');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(
    'Loading invitation'
  );
  await expect(page.locator('main')).toHaveCount(1);

  await page.goto('/?page=invite&mode=invite-error');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(
    'Could not load invitation'
  );
  await expect(page.getByRole('button', { name: 'Try again' })).toBeVisible();

  await page.goto('/?page=invite');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(
    "You're invited"
  );
  await expectNoOverflow(page);
});

for (const width of viewports) {
  test(`remote organizations remain usable at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/?page=home');
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(
      'Organizations'
    );
    await expect(page.locator('main')).toHaveCount(1);
    await expectNoOverflow(page);
    const project = page.getByRole('link', { name: /Open project/ });
    await project.focus();
    await expect(project).toBeFocused();
    if (width === 375) {
      expect(await project.boundingBox()).toMatchObject({ height: 61 });
    }
  });
}

test('remote organizations keep explicit loading, empty, error, and degraded states', async ({
  page,
}) => {
  await page.goto('/?page=home&mode=home-loading');
  await expect(page.locator('[data-state="loading"]')).toContainText(
    'Loading organizations'
  );

  await page.goto('/?page=home&mode=home-empty');
  await expect(page.locator('[data-state="empty"]')).toContainText(
    'No organizations found'
  );

  await page.goto('/?page=home&mode=home-error');
  await expect(page.locator('[data-state="error"]')).toContainText(
    'Unable to load organizations'
  );

  await page.goto('/?page=home&mode=home-degraded');
  await expect(page.locator('[data-state="degraded"]')).toContainText(
    'Organizations may be out of date'
  );
  await expect(page.getByRole('link', { name: /Open project/ })).toBeVisible();
});
