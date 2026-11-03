import { expect, test, type Page } from '@playwright/test';
import {
  THEME_STORAGE_KEY,
  type EffectiveTheme,
  type ThemeMode,
} from '../../../packages/ui/src/lib/theme';

async function readSemanticThemeTokens(page: Page) {
  return page.evaluate(() => {
    const styles = getComputedStyle(document.documentElement);
    return {
      surfaceCanvas: styles.getPropertyValue('--vk-surface-canvas').trim(),
      textHigh: styles.getPropertyValue('--vk-text-high').trim(),
    };
  });
}

async function readMotionDurationTokens(page: Page) {
  return page.evaluate(() => {
    const styles = getComputedStyle(document.documentElement);
    return {
      fast: styles.getPropertyValue('--vk-duration-fast').trim(),
      normal: styles.getPropertyValue('--vk-duration-normal').trim(),
      slow: styles.getPropertyValue('--vk-duration-slow').trim(),
    };
  });
}

async function expectRootTheme(
  page: Page,
  mode: ThemeMode,
  theme: EffectiveTheme
) {
  await expect
    .poll(() =>
      page.evaluate(() => ({
        mode: document.documentElement.dataset.themeMode,
        theme: document.documentElement.dataset.theme,
        className: document.documentElement.className,
        colorScheme: document.documentElement.style.colorScheme,
      }))
    )
    .toEqual({
      mode,
      theme,
      className: theme,
      colorScheme: theme,
    });
}

test('first launch resolves System to the OS theme before app code runs', async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.goto('/');

  await expect(page.getByTestId('bootstrap-snapshot')).toHaveText(
    JSON.stringify({
      mode: 'system',
      theme: 'dark',
      colorScheme: 'dark',
    })
  );
  await expect(page.getByTestId('controller-mode')).toHaveText('system');
  await expect(page.getByTestId('effective-theme')).toHaveText('dark');
  await expectRootTheme(page, 'system', 'dark');
  await expect
    .poll(() =>
      page.evaluate((key) => localStorage.getItem(key), THEME_STORAGE_KEY)
    )
    .toBeNull();
});

test('OS color-scheme changes affect System mode only', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'light' });
  await page.goto('/');
  await expectRootTheme(page, 'system', 'light');
  const lightTokens = await readSemanticThemeTokens(page);
  expect(lightTokens.surfaceCanvas).not.toBe('');
  expect(lightTokens.textHigh).not.toBe('');

  await page.emulateMedia({ colorScheme: 'dark' });
  await expectRootTheme(page, 'system', 'dark');
  const darkTokens = await readSemanticThemeTokens(page);
  expect(darkTokens.surfaceCanvas).not.toBe(lightTokens.surfaceCanvas);
  expect(darkTokens.textHigh).not.toBe(lightTokens.textHigh);

  await page.getByRole('button', { name: 'Light' }).click();
  await expectRootTheme(page, 'light', 'light');
  await page.emulateMedia({ colorScheme: 'light' });
  await expectRootTheme(page, 'light', 'light');
  await page.emulateMedia({ colorScheme: 'dark' });
  await expectRootTheme(page, 'light', 'light');
  expect(await readSemanticThemeTokens(page)).toEqual(lightTokens);

  await page.getByRole('button', { name: 'Dark' }).click();
  await expectRootTheme(page, 'dark', 'dark');
  await page.emulateMedia({ colorScheme: 'light' });
  await expectRootTheme(page, 'dark', 'dark');
  expect(await readSemanticThemeTokens(page)).toEqual(darkTokens);

  await page.getByRole('button', { name: 'System' }).click();
  await expectRootTheme(page, 'system', 'light');
  expect(await readSemanticThemeTokens(page)).toEqual(lightTokens);
});

test('explicit preference persists through refresh and pre-paint bootstrap', async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: 'light' });
  await page.goto('/');

  await page.getByRole('button', { name: 'Dark' }).click();
  await expect
    .poll(() =>
      page.evaluate((key) => localStorage.getItem(key), THEME_STORAGE_KEY)
    )
    .toBe('dark');

  await page.reload();

  await expect(page.getByTestId('bootstrap-snapshot')).toHaveText(
    JSON.stringify({
      mode: 'dark',
      theme: 'dark',
      colorScheme: 'dark',
    })
  );
  await expect(page.getByTestId('controller-mode')).toHaveText('dark');
  await expectRootTheme(page, 'dark', 'dark');
});

test('theme switching preserves route, draft, scroll, and mounted app DOM', async ({
  page,
}) => {
  await page.goto('/');

  const initialUrl = page.url();
  const appMount = await page.getByTestId('app-mount').elementHandle();
  const draft = page.getByTestId('draft');
  const scrollRegion = page.getByTestId('scroll-region');

  await draft.fill('Unsaved representative draft');
  await scrollRegion.evaluate((element) => {
    element.scrollTop = 240;
  });

  await page.getByRole('button', { name: 'Dark' }).click();
  await expectRootTheme(page, 'dark', 'dark');
  await page.getByRole('button', { name: 'Light' }).click();
  await expectRootTheme(page, 'light', 'light');

  expect(page.url()).toBe(initialUrl);
  await expect(draft).toHaveValue('Unsaved representative draft');
  await expect
    .poll(() => scrollRegion.evaluate((element) => element.scrollTop))
    .toBe(240);
  expect(await appMount?.evaluate((element) => element.isConnected)).toBe(true);
  await expect(page.getByTestId('mount-count')).toHaveText('1');
});

test('reduced-motion preference collapses shared duration tokens', async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.goto('/');
  expect(await readMotionDurationTokens(page)).toEqual({
    fast: '120ms',
    normal: '180ms',
    slow: '220ms',
  });

  await page.emulateMedia({ reducedMotion: 'reduce' });
  await expect
    .poll(() => readMotionDurationTokens(page))
    .toEqual({
      fast: '0ms',
      normal: '0ms',
      slow: '0ms',
    });

  await expect
    .poll(() =>
      page.getByTestId('motion-probe').evaluate((element) => {
        const styles = getComputedStyle(element);
        return {
          animationDuration: styles.animationDuration,
          animationIterationCount: styles.animationIterationCount,
          transitionDuration: styles.transitionDuration,
        };
      })
    )
    .toEqual({
      animationDuration: '1e-05s',
      animationIterationCount: '1',
      transitionDuration: '1e-05s',
    });
});

test('disabled asChild Button blocks child handlers and navigation', async ({
  page,
}) => {
  await page.goto('/');

  const action = page.getByTestId('disabled-as-child');
  const initialUrl = page.url();
  await expect(action).toHaveAttribute('aria-disabled', 'true');

  await action.dispatchEvent('click');
  await action.focus();
  await page.keyboard.press('Enter');

  expect(page.url()).toBe(initialUrl);
  await expect(page.getByTestId('child-capture-count')).toHaveText('0');
  await expect(page.getByTestId('child-click-count')).toHaveText('0');
  await expect(page.getByTestId('button-capture-count')).toHaveText('0');
});

test('Button loadingLabel only contributes to the accessible name while loading', async ({
  page,
}) => {
  await page.goto('/');

  const idle = page.getByTestId('idle-loading-label');
  await expect(idle).toHaveAccessibleName('Save');
  await expect(idle).not.toHaveAttribute('aria-busy', 'true');
  await expect(idle.locator('.sr-only')).toHaveCount(0);

  const active = page.getByTestId('active-loading-label');
  await expect(active).toHaveAccessibleName('Saving');
  await expect(active).toHaveAttribute('aria-busy', 'true');
  await expect(active.locator('.sr-only')).toHaveText('Saving');
});

test('invalid Input exposes its description association', async ({ page }) => {
  await page.goto('/');

  const input = page.getByTestId('invalid-input');
  await expect(input).toHaveAttribute('aria-invalid', 'true');
  await expect(input).toHaveAttribute(
    'aria-describedby',
    'invalid-agent-endpoint-error'
  );
  await expect(page.locator('#invalid-agent-endpoint-error')).toHaveText(
    'Enter a valid endpoint.'
  );
});

test('CrashScreen uses the canonical error state and preserves recovery controls', async ({
  page,
}) => {
  await page.goto('/');

  const crashScreen = page
    .getByTestId('crash-screen-harness')
    .locator('[data-state="error"]');
  await expect(crashScreen).toBeVisible();
  await expect(crashScreen).toHaveAttribute('role', 'alert');

  const buttons = crashScreen.getByRole('button');
  await buttons.first().click();
  await expect(page.getByTestId('crash-reload-count')).toHaveText('1');

  await buttons.nth(1).click();
  await expect(crashScreen.getByText('Fixture crash')).toBeVisible();
  await expect(crashScreen.getByText('Component stack:')).toBeVisible();
});

test('FloatingPanel keeps non-modal focus ownership explicit', async ({
  page,
}) => {
  await page.goto('/');

  const trigger = page.getByTestId('panel-trigger');
  const canvasTarget = page.getByTestId('canvas-target');
  const autoFocusTrigger = page.getByTestId('autofocus-trigger');

  await trigger.click();
  await expect(trigger).toBeFocused();
  const defaultPanel = page.getByRole('dialog', {
    name: 'Node configuration',
  });
  await expect(defaultPanel).toBeVisible();
  expect(await defaultPanel.getAttribute('aria-modal')).toBeNull();
  await page.getByRole('button', { name: 'Close panel', exact: true }).click();
  await expect(trigger).toBeFocused();

  await canvasTarget.click();
  await expect(canvasTarget).toBeFocused();
  await expect(defaultPanel).toBeVisible();
  await page.getByRole('button', { name: 'Close panel', exact: true }).click();
  await expect(canvasTarget).toBeFocused();

  await autoFocusTrigger.click();
  const closeButton = page.getByRole('button', {
    name: 'Close panel',
    exact: true,
  });
  await expect(closeButton).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(defaultPanel).toBeHidden();
  await expect(autoFocusTrigger).toBeFocused();

  await autoFocusTrigger.click();
  await expect(closeButton).toBeFocused();
  await closeButton.click();
  await expect(defaultPanel).toBeHidden();
  await expect(autoFocusTrigger).toBeFocused();

  await autoFocusTrigger.click();
  await page.getByTestId('panel-input').focus();
  const canvasClose = page.getByTestId('canvas-close-panel');
  await canvasClose.click();
  await expect(defaultPanel).toBeHidden();
  await expect(canvasClose).toBeFocused();
  await expect(autoFocusTrigger).not.toBeFocused();
});

test('SplitLayout keyboard and pointer input update controlled bounded size', async ({
  page,
}) => {
  await page.goto('/');

  const separator = page.getByRole('separator', {
    name: 'Resize test inspector',
  });
  const secondaryPane = page.locator('#fixture-secondary-pane');
  const secondarySize = page.getByTestId('secondary-size');

  await expect(separator).toHaveAttribute('aria-valuenow', '320');
  await separator.focus();

  await page.keyboard.press('Home');
  await expect(separator).toHaveAttribute('aria-valuenow', '240');
  await expect(secondarySize).toHaveText('240');
  await page.keyboard.press('ArrowRight');
  await expect(separator).toHaveAttribute('aria-valuenow', '240');

  await page.keyboard.press('End');
  await expect(separator).toHaveAttribute('aria-valuenow', '400');
  await expect(secondarySize).toHaveText('400');
  await page.keyboard.press('ArrowLeft');
  await expect(separator).toHaveAttribute('aria-valuenow', '400');

  await page.keyboard.press('ArrowRight');
  await expect(separator).toHaveAttribute('aria-valuenow', '360');
  await expect(secondarySize).toHaveText('360');

  const separatorBox = await separator.boundingBox();
  if (!separatorBox) {
    throw new Error('SplitLayout separator has no browser geometry');
  }
  const startX = separatorBox.x + separatorBox.width / 2;
  const startY = separatorBox.y + separatorBox.height / 2;
  await page.mouse.move(startX, startY);
  await page.mouse.down();
  await page.mouse.move(startX - 30, startY, { steps: 3 });
  await page.mouse.up();

  await expect(separator).toHaveAttribute('aria-valuenow', '390');
  await expect(secondarySize).toHaveText('390');
  await expect(secondaryPane).toHaveCSS('width', '390px');
  await expect(page.getByTestId('resize-end-size')).toHaveText('390');
});

test('SplitLayout pointer cancellation keeps the last accepted size', async ({
  page,
}) => {
  await page.goto('/');

  const separator = page.getByRole('separator', {
    name: 'Resize test inspector',
  });
  await separator.scrollIntoViewIfNeeded();
  const separatorBox = await separator.boundingBox();
  if (!separatorBox) {
    throw new Error('SplitLayout separator has no browser geometry');
  }

  await page.evaluate(() => {
    document.addEventListener(
      'pointerdown',
      (event) => {
        document.documentElement.dataset.fixturePointerId = String(
          event.pointerId
        );
      },
      { once: true }
    );
  });

  const startX = separatorBox.x + separatorBox.width / 2;
  const startY = separatorBox.y + separatorBox.height / 2;
  await page.mouse.move(startX, startY);
  await page.mouse.down();
  await page.mouse.move(startX - 30, startY, { steps: 3 });
  await expect(page.getByTestId('secondary-size')).toHaveText('350');

  const pointerId = await page.evaluate(() =>
    Number(document.documentElement.dataset.fixturePointerId)
  );
  await separator.dispatchEvent('pointercancel', {
    bubbles: true,
    clientX: 0,
    clientY: 0,
    pointerId,
  });
  await page.mouse.up();

  await expect(page.getByTestId('secondary-size')).toHaveText('350');
  await expect(page.getByTestId('resize-end-size')).toHaveText('350');
});
