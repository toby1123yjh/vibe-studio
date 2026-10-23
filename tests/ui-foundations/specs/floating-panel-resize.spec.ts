import { expect, test, type Locator, type Page } from '@playwright/test';

const preferenceKey = 'fixture.conversation-panel-width';

async function expectWidth(panel: Locator, width: number) {
  await expect
    .poll(async () => Math.round((await panel.boundingBox())?.width ?? 0))
    .toBe(width);
}

async function startDrag(page: Page, separator: Locator) {
  const box = await separator.boundingBox();
  if (!box) throw new Error('Resize separator is missing');
  const origin = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  await page.mouse.move(origin.x, origin.y);
  await page.mouse.down();
  return origin;
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 1000 });
  await page.goto('/?panel-resize=1');
});

test('pointer and keyboard resizing preserve content and remember shared widths', async ({
  page,
}) => {
  const panel = page.getByTestId('resizable-panel');
  const separator = panel.getByRole('separator', {
    name: 'Resize conversation width',
  });
  const draft = page.getByTestId('resizable-panel-draft');
  await expectWidth(panel, 600);
  await draft.fill('A draft that must survive resizing');
  const editorNode = await draft.elementHandle();
  await panel.evaluate((node) => {
    (window as unknown as { originalPanel: Element }).originalPanel = node;
  });

  const origin = await startDrag(page, separator);
  await page.mouse.move(origin.x - 128, origin.y, { steps: 8 });
  await page.mouse.up();
  await expectWidth(panel, 728);
  await expect(draft).toHaveValue('A draft that must survive resizing');
  expect(
    await draft.evaluate((node, previous) => node === previous, editorNode)
  ).toBe(true);
  expect(
    await panel.evaluate(
      (node) =>
        node === (window as unknown as { originalPanel: Element }).originalPanel
    )
  ).toBe(true);
  await expect(separator).toHaveAttribute('aria-valuenow', '728');
  expect(
    await page.evaluate((key) => localStorage.getItem(key), preferenceKey)
  ).toBe('728');

  await separator.press('Home');
  await expectWidth(panel, 480);
  await separator.press('ArrowLeft');
  await expectWidth(panel, 496);
  await separator.press('End');
  await expectWidth(panel, 900);
  await separator.press('ArrowLeft');
  await expectWidth(panel, 900);
  await separator.press('Shift+ArrowRight');
  await expectWidth(panel, 836);

  await page.getByRole('button', { name: 'Toggle conversation' }).click();
  await expect(panel).toHaveCount(0);
  await page.getByRole('button', { name: 'Toggle conversation' }).click();
  await expectWidth(panel, 836);
  await page.getByRole('button', { name: 'Open another surface' }).click();
  await expect(panel).toHaveAttribute('aria-label', 'Workflow conversation');
  await expectWidth(panel, 836);
  await page.reload();
  await expectWidth(panel, 836);
});

test('temporary parent and mobile constraints do not overwrite the preferred width', async ({
  page,
}) => {
  const panel = page.getByTestId('resizable-panel');
  const separator = panel.getByRole('separator');
  await separator.press('End');
  await expectWidth(panel, 900);
  await page.getByRole('button', { name: 'Narrow parent' }).click();
  await expectWidth(panel, 472);
  await expect(separator).toHaveCount(0);
  const parentBounds = await page
    .getByTestId('resizable-panel-parent')
    .boundingBox();
  const panelBounds = await panel.boundingBox();
  expect(panelBounds!.x).toBeGreaterThanOrEqual(parentBounds!.x);
  expect(panelBounds!.x + panelBounds!.width).toBeLessThanOrEqual(
    parentBounds!.x + parentBounds!.width
  );
  expect(
    await page.evaluate((key) => localStorage.getItem(key), preferenceKey)
  ).toBe('900');

  await page.getByRole('button', { name: 'Wide parent' }).click();
  await expectWidth(panel, 900);
  await page.setViewportSize({ width: 375, height: 812 });
  await expectWidth(panel, 327);
  await expect(separator).toHaveCount(0);
  expect(
    await page.evaluate((key) => localStorage.getItem(key), preferenceKey)
  ).toBe('900');
  await page.setViewportSize({ width: 1280, height: 1000 });
  await expectWidth(panel, 900);
});

test('the mobile resize boundary agrees with the real CSS media query', async ({
  page,
}) => {
  const panel = page.getByTestId('resizable-panel');
  for (const width of [767, 768]) {
    await page.setViewportSize({ width, height: 1000 });
    const mobile = await page.evaluate(
      () => window.matchMedia('(width < 768px)').matches
    );
    await expect(panel.getByRole('separator')).toHaveCount(mobile ? 0 : 1);
    await expect(panel).toHaveCSS('right', mobile ? '0px' : '24px');
  }
});

for (const ending of [
  'pointercancel',
  'lostpointercapture',
  'unmount',
] as const) {
  test(`${ending} safely ends a drag at the accepted width`, async ({
    page,
  }) => {
    const panel = page.getByTestId('resizable-panel');
    const separator = panel.getByRole('separator');
    const origin = await startDrag(page, separator);
    await page.mouse.move(origin.x - 64, origin.y);
    await expectWidth(panel, 664);
    if (ending === 'unmount') {
      await page
        .getByRole('button', { name: 'Toggle conversation' })
        .evaluate((node) => (node as HTMLButtonElement).click());
      await expect(panel).toHaveCount(0);
    } else {
      await separator.evaluate((node, type) => {
        node.dispatchEvent(
          new PointerEvent(type, { pointerId: 1, bubbles: true, clientX: 0 })
        );
      }, ending);
      await expectWidth(panel, 664);
    }
    await page.mouse.up();
    expect(
      await page.evaluate(() => ({
        cursor: document.documentElement.style.cursor,
        userSelect: document.documentElement.style.userSelect,
      }))
    ).toEqual({ cursor: '', userSelect: '' });
    expect(
      await page.evaluate((key) => localStorage.getItem(key), preferenceKey)
    ).toBe('664');
    if (ending === 'unmount') {
      await page.getByRole('button', { name: 'Toggle conversation' }).click();
      await expectWidth(panel, 664);
    }
    await page.getByTestId('resizable-canvas-button').click();
    await expect(page.getByTestId('resizable-canvas-button')).toBeFocused();
  });
}

test('denied persistence keeps resizing usable', async ({ page }) => {
  await page.addInitScript((key) => {
    const getItem = Storage.prototype.getItem;
    const setItem = Storage.prototype.setItem;
    Object.defineProperty(Storage.prototype, 'getItem', {
      value(requestedKey: string) {
        if (requestedKey === key) {
          throw new DOMException('Storage is unavailable', 'SecurityError');
        }
        return getItem.call(this, requestedKey);
      },
    });
    Object.defineProperty(Storage.prototype, 'setItem', {
      value(requestedKey: string, value: string) {
        if (requestedKey === key) {
          throw new DOMException('Storage is unavailable', 'SecurityError');
        }
        return setItem.call(this, requestedKey, value);
      },
    });
  }, preferenceKey);
  await page.reload();
  const panel = page.getByTestId('resizable-panel');
  await expectWidth(panel, 600);
  await panel.getByRole('separator').press('ArrowLeft');
  await expectWidth(panel, 616);
});
