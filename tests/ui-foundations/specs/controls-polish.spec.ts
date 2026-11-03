import { expect, test } from '@playwright/test';

test('button wrappers share geometry and retain form, disabled and busy behavior', async ({
  page,
}) => {
  await page.goto('/?controls');
  const canonical = page.getByTestId('canonical-action');
  const primary = page.getByTestId('primary-action');
  await expect(primary).toBeVisible();
  await expect(primary).toHaveCSS('transition-duration', '0.12s');
  const idleCanonical = await canonical.boundingBox();
  const idlePrimary = await primary.boundingBox();
  expect(idleCanonical).toBeTruthy();
  expect(idlePrimary).toBeTruthy();
  expect(idlePrimary?.height).toBe(idleCanonical?.height);
  expect(idlePrimary?.width).toBe(idleCanonical?.width);
  expect(idleCanonical?.height).toBe(32);

  await page.getByRole('button', { name: 'Preview changes' }).click();
  await expect(page.getByTestId('preview-count')).toHaveText('1');
  await expect(page.getByTestId('submit-count')).toHaveText('0');
  await primary.click();
  await expect(page.getByTestId('submit-count')).toHaveText('1');

  await page.getByRole('button', { name: 'Toggle busy' }).click();
  for (const button of [canonical, primary]) {
    await expect(button).toBeDisabled();
    await expect(button).toHaveAttribute('aria-busy', 'true');
    const busy = await button.boundingBox();
    expect(busy?.width).toBe(idleCanonical?.width);
    expect(busy?.height).toBe(idleCanonical?.height);
  }
  await expect(
    page.getByRole('button', { name: 'Preview changes' })
  ).toBeDisabled();
  await primary.dispatchEvent('click');
  await expect(page.getByTestId('submit-count')).toHaveText('1');
});

test('coarse pointer controls expose touch targets without changing desktop density', async ({
  browser,
}) => {
  const context = await browser.newContext({
    hasTouch: true,
    viewport: { width: 375, height: 812 },
  });
  try {
    const page = await context.newPage();
    await page.goto('http://127.0.0.1:4176/?controls');
    for (const control of [
      page.getByTestId('primary-action'),
      page.getByRole('button', { name: 'Preview changes' }),
      page.getByLabel('Profile name'),
      page.getByRole('combobox', { name: 'Default agent' }),
    ]) {
      await expect(control).toHaveCSS('min-height', '44px');
    }
    const icon = await page
      .getByRole('button', { name: 'Preview changes' })
      .boundingBox();
    expect(icon?.width).toBeGreaterThanOrEqual(44);
    await page.getByRole('button', { name: 'Choose model' }).click();
    for (const item of await page.getByRole('menuitem').all()) {
      await expect(item).toHaveCSS('min-height', '44px');
    }
  } finally {
    await context.close();
  }
});

for (const colorScheme of ['light', 'dark'] as const) {
  test(`select shares input geometry and themed menu layers in ${colorScheme} mode`, async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme });
    await page.goto('/?controls');
    const select = page.getByRole('combobox', { name: 'Default agent' });
    const input = page.getByLabel('Profile name');
    await expect(select).toHaveCSS(
      'min-height',
      await input.evaluate((element) => getComputedStyle(element).minHeight)
    );
    await expect(select).toHaveCSS(
      'background-color',
      await input.evaluate(
        (element) => getComputedStyle(element).backgroundColor
      )
    );
    await select.focus();
    await select.press('Space');
    const menu = page.getByRole('listbox');
    await expect(menu).toBeVisible();
    const expected = await menu.evaluate((element) => {
      const probe = document.createElement('div');
      probe.style.backgroundColor = 'var(--vk-menu-surface)';
      element.appendChild(probe);
      const background = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return {
        background,
        layer: getComputedStyle(element)
          .getPropertyValue('--vk-z-popover')
          .trim(),
      };
    });
    await expect(menu).toHaveCSS('z-index', expected.layer);
    await expect(menu).toHaveCSS('background-color', expected.background);
    await expect(page.getByRole('option', { name: 'Gemini' })).toHaveAttribute(
      'aria-disabled',
      'true'
    );
    await expect(
      page.getByRole('option', { name: 'Codex', exact: true })
    ).toBeFocused();
    await page.keyboard.press('End');
    await expect(
      page.getByRole('option', { name: 'Claude Code' })
    ).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('selected-provider')).toHaveText('claude');
    await expect(select).toBeFocused();
  });
}

test('searchable dropdown retains search focus on hover and select behavior', async ({
  page,
}) => {
  await page.goto('/?controls');
  await page.getByRole('button', { name: 'Choose model' }).click();
  const search = page.getByRole('textbox', { name: 'Find model' });
  await expect(search).toBeFocused();
  await search.fill('model');
  await page.getByRole('menuitem', { name: 'Reasoning model' }).hover();
  await expect(search).toBeFocused();
  await page.getByRole('menuitem', { name: 'Reasoning model' }).click();
  await expect(page.getByTestId('selected-model')).toHaveText(
    'Reasoning model'
  );
  await expect(
    page.getByRole('button', { name: 'Choose model' })
  ).toBeFocused();
});

test('long popover stays in the viewport and its last action remains reachable', async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 640 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/?controls');
  await page.getByRole('button', { name: 'Open long menu' }).click();
  const popover = page.getByTestId('long-popover');
  await expect(popover).toBeVisible();
  const bounds = await popover.boundingBox();
  expect(bounds).toBeTruthy();
  expect(bounds!.y).toBeGreaterThanOrEqual(0);
  expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(640);
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(375);
  await expect(popover).toHaveCSS('overflow-y', 'auto');
  const last = popover.getByRole('button', { name: 'Last option' });
  await last.scrollIntoViewIfNeeded();
  await expect(last).toBeInViewport();
  const duration = await popover.evaluate((element) =>
    Number.parseFloat(getComputedStyle(element).animationDuration)
  );
  expect(duration).toBeLessThanOrEqual(0.001);
});
