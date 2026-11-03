import { expect, test, type Locator, type Page } from '@playwright/test';

async function bounds(locator: Locator) {
  const box = await locator.boundingBox();
  if (!box) throw new Error('Expected visible drag geometry');
  return box;
}

async function pickUp(page: Page, card: Locator) {
  const box = await bounds(card);
  const pointer = { x: box.x + 48, y: box.y + 24 };
  await page.mouse.move(pointer.x - 16, pointer.y);
  await page.mouse.down();
  await page.mouse.move(pointer.x, pointer.y, { steps: 2 });
  await expect(card).toHaveAttribute('data-dragging', 'true');
  await expect(page.locator('.vk-kanban-drag-preview')).toBeVisible();
  // The threshold-crossing event activates the sensor; its next move drives
  // the overlay transform. Check grip continuity after that first drag frame.
  pointer.x += 1;
  await page.mouse.move(pointer.x, pointer.y);
  return pointer;
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  // These tests use an in-memory board; never write to the user's project.
  await page.route('**/api/**', (route) => route.abort());
});

for (const id of ['issue-1', 'issue-2']) {
  test(`pickup keeps ${id} dimensions, content, grip and source space`, async ({
    page,
  }) => {
    await page.goto('/');
    const card = page.locator(`[data-task-id="${id}"]`);
    const next = page.locator('[data-task-id="issue-long-0"]');
    const original = await bounds(card);
    const nextOriginal = await bounds(next);
    const title = await card.locator('h3').innerText();
    const pointer = await pickUp(page, card);
    const preview = page.locator('.vk-kanban-drag-preview');
    const lifted = await bounds(preview);
    expect(lifted.width).toBeCloseTo(original.width, 0);
    expect(lifted.height).toBeCloseTo(original.height, 0);
    expect(pointer.x - lifted.x).toBeCloseTo(32, 0);
    expect(pointer.y - lifted.y).toBeCloseTo(24, 0);
    expect((await bounds(card)).height).toBeCloseTo(original.height, 0);
    expect(await bounds(next)).toEqual(nextOriginal);
    await expect(card).toHaveCSS('opacity', '0');
    await expect(preview.locator('h3')).toHaveText(title);
    await expect(preview.locator('[data-task-id]')).toHaveCount(0);
    await expect(
      preview.locator('button, a, input, [tabindex="0"]')
    ).toHaveCount(0);
    if (id === 'issue-1') {
      await expect(preview).toContainText('Planning label 1');
      await expect(preview).toContainText('Run the canonical agent task');
      await expect(preview).toContainText('high');
    }
    await page.mouse.move(pointer.x + 90, pointer.y + 40);
    await expect
      .poll(async () => (await bounds(preview)).x)
      .toBeCloseTo(lifted.x + 90, 0);
    expect((await bounds(preview)).width).toBeCloseTo(original.width, 0);
    expect((await bounds(preview)).height).toBeCloseTo(original.height, 0);
    await page.keyboard.press('Escape');
    await page.mouse.up();
    await expect(preview).toHaveCount(0);
    await expect(card).toHaveCSS('opacity', '1');
    expect(await bounds(next)).toEqual(nextOriginal);
    await expect(page.getByTestId('move-count')).toHaveText('0');
  });
}

test('only a genuinely tall card is clipped; its source and width stay unchanged', async ({
  page,
}) => {
  await page.goto('/?tall');
  const card = page.locator('[data-task-id="issue-1"]');
  const original = await bounds(card);
  expect(original.height).toBeGreaterThan(480);
  await pickUp(page, card);
  const preview = page.locator('.vk-kanban-drag-preview');
  await expect(preview).toHaveAttribute('data-clipped', 'true');
  const lifted = await bounds(preview);
  expect(lifted.width).toBeCloseTo(original.width, 0);
  expect(lifted.height).toBeLessThan(original.height);
  expect(lifted.height).toBeLessThanOrEqual(900 * 0.65);
  expect((await bounds(card)).height).toBeCloseTo(original.height, 0);
  await expect(preview.locator('h3')).toHaveText(
    'Keyboard and pointer interaction'
  );
  await expect(preview).toHaveCSS('overflow', 'hidden');
  const fade = await preview.evaluate(
    (element) => getComputedStyle(element, '::after').backgroundImage
  );
  expect(fade).toContain('linear-gradient');
  await page.keyboard.press('Escape');
  await page.mouse.up();
  await expect(preview).toHaveCount(0);
  expect((await bounds(card)).height).toBeCloseTo(original.height, 0);
  await expect(page.getByTestId('move-count')).toHaveText('0');
});

test('coarse-pointer layout keeps title and task positions inside the lifted card', async ({
  browser,
}) => {
  const page = await browser.newPage({
    viewport: { width: 375, height: 812 },
    hasTouch: true,
  });
  try {
    await page.route('**/api/**', (route) => route.abort());
    await page.goto('/');
    const card = page.locator('[data-task-id="issue-1"]');
    const original = await bounds(card);
    const title = await bounds(card.locator('h3'));
    const task = await bounds(card.locator('.vk-kanban-task-preview'));
    await pickUp(page, card);
    const preview = page.locator('.vk-kanban-drag-preview');
    const lifted = await bounds(preview);
    const liftedTitle = await bounds(preview.locator('h3'));
    const liftedTask = await bounds(preview.locator('.vk-kanban-task-preview'));
    expect(lifted.width).toBeCloseTo(original.width, 0);
    expect(lifted.height).toBeCloseTo(original.height, 0);
    expect(liftedTitle.y - lifted.y).toBeCloseTo(title.y - original.y, 0);
    expect(liftedTask.y - lifted.y).toBeCloseTo(task.y - original.y, 0);
    await expect(
      preview.locator('button, a, input, [tabindex="0"]')
    ).toHaveCount(0);
    await page.keyboard.press('Escape');
    await page.mouse.up();
    await expect(page.getByTestId('move-count')).toHaveText('0');
  } finally {
    await page.close();
  }
});

test('picking up near the bottom of a tall, scrolled card keeps the preview under the pointer', async ({
  page,
}) => {
  await page.goto('/?tall');
  const card = page.locator('[data-task-id="issue-1"]');
  const original = await bounds(card);
  await page.locator('.vk-kanban-scroll').evaluate((element) => {
    element.scrollTop = 400;
  });
  const scrolled = await bounds(card);
  // Pick the outer padding, not a tag/task/button excluded from drag.
  const pointer = {
    x: scrolled.x + 5,
    y: scrolled.y + Math.min(original.height - 10, 700),
  };
  await page.mouse.move(pointer.x, pointer.y);
  await page.mouse.down();
  await page.mouse.move(pointer.x + 20, pointer.y, { steps: 2 });
  const preview = page.locator('.vk-kanban-drag-preview');
  await expect(preview).toBeVisible();
  const box = await bounds(preview);
  expect(pointer.y).toBeGreaterThanOrEqual(box.y);
  expect(pointer.y).toBeLessThanOrEqual(box.y + box.height);
  expect(box.width).toBeCloseTo(original.width, 0);
  await page.keyboard.press('Escape');
  await page.mouse.up();
  await expect(preview).toHaveCount(0);
});

async function observeDrop(page: Page) {
  // Install the listener before releasing the pointer. Returning the Promise
  // directly races page.evaluate against mouse.up and can miss the event.
  const observation = await page.evaluateHandle(() => ({
    animations: new Promise<
      Array<{ duration: number | string; frames: ComputedKeyframe[] }>
    >((resolve) => {
      document.addEventListener(
        'pointerup',
        () => {
          const observed = new Map<
            Animation,
            { duration: number | string; frames: ComputedKeyframe[] }
          >();
          const started = performance.now();
          function sample() {
            const preview = document.querySelector('.vk-kanban-drag-preview');
            if (preview) {
              for (const animation of document.getAnimations()) {
                const effect = animation.effect;
                if (
                  effect instanceof KeyframeEffect &&
                  effect.target?.contains(preview)
                ) {
                  observed.set(animation, {
                    duration: effect.getTiming().duration,
                    frames: effect.getKeyframes(),
                  });
                }
              }
            }
            if (performance.now() - started < 300)
              requestAnimationFrame(sample);
            else resolve([...observed.values()]);
          }
          requestAnimationFrame(sample);
        },
        { once: true, capture: true }
      );
    }),
  }));
  return async () => {
    try {
      return await observation.evaluate((sample) => sample.animations);
    } finally {
      await observation.dispose();
    }
  };
}

test('drop settles with a short translation and respects live reduced-motion changes', async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.goto('/');
  const card = page.locator('[data-task-id="issue-1"]');
  await pickUp(page, card);
  const destination = await bounds(page.locator('.vk-kanban-column').nth(1));
  await page.mouse.move(destination.x + 90, destination.y + 120, { steps: 5 });
  const sample = await observeDrop(page);
  await page.mouse.up();
  const animations = await sample();
  expect(animations.length).toBeGreaterThan(0);
  for (const animation of animations) {
    expect(Number(animation.duration)).toBeGreaterThan(0);
    expect(Number(animation.duration)).toBeLessThanOrEqual(200);
    for (const frame of animation.frames) {
      expect(frame.transform).toContain('scaleX(1) scaleY(1)');
    }
  }
  await expect(page.getByTestId('move-count')).toHaveText('1');
  await expect(page.locator('.vk-kanban-drag-preview')).toHaveCount(0);
  await expect(card).toHaveCSS('opacity', '1');

  await page.emulateMedia({ reducedMotion: 'reduce' });
  await pickUp(page, card);
  await page.mouse.move(4, 4, { steps: 3 });
  const reducedSample = await observeDrop(page);
  await page.mouse.up();
  expect(await reducedSample()).toEqual([]);
  await expect(page.locator('.vk-kanban-drag-preview')).toHaveCount(0);
  await expect(card).toHaveCSS('opacity', '1');
  await expect(page.getByTestId('move-count')).toHaveText('1');
});

test('reduced motion disables keyboard overlay movement transitions', async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/');
  const card = page.locator('[data-task-id="issue-1"]');
  await card.focus();
  await page.keyboard.press('Space');
  const preview = page.locator('.vk-kanban-drag-preview');
  await expect(preview).toBeVisible();
  await expect(preview.locator('..')).toHaveAttribute(
    'style',
    /transition: none/
  );
  await page.keyboard.press('ArrowRight');
  await expect(
    page.getByText(
      'Draggable item issue-1 was moved over droppable area issue-doing'
    )
  ).toBeAttached();
  const motion = await preview.locator('..').evaluate((element) => {
    const style = getComputedStyle(element);
    return {
      property: style.transitionProperty,
      duration: style.transitionDuration,
    };
  });
  expect(motion.property).not.toContain('transform');
  expect(parseFloat(motion.duration)).toBeLessThan(0.001);
  await page.keyboard.press('Space');
  await expect(preview).toHaveCount(0);
  await expect(card).toHaveCSS('opacity', '1');
  await expect(page.getByTestId('move-count')).toHaveText('1');
});

test('a second pickup during settlement cannot leave the source transparent', async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.goto('/');
  // Hold settlement open deterministically so a slow CI machine still tests
  // overlapping cleanup, rather than accidentally waiting out the first drop.
  await page.evaluate(() => {
    const animate = Element.prototype.animate;
    Element.prototype.animate = function (...args) {
      const animation = animate.apply(this, args);
      if (this.querySelector('.vk-kanban-drag-preview')) animation.pause();
      return animation;
    };
  });
  const card = page.locator('[data-task-id="issue-1"]');
  await pickUp(page, card);
  await page.mouse.move(4, 4);
  await page.mouse.up();
  await expect(card).toHaveAttribute('data-dragging', 'false');
  await expect(card).toHaveCSS('opacity', '0');

  const box = await bounds(card);
  // Use lower outer padding, outside the first overlay's held return path.
  await page.mouse.move(box.x + 5, box.y + box.height - 8);
  await page.mouse.down();
  await page.mouse.move(box.x + 25, box.y + box.height - 8);
  await expect(card).toHaveAttribute('data-dragging', 'true');
  await page.mouse.move(4, 4);
  await page.mouse.up();
  await expect(card).toHaveAttribute('data-dragging', 'false');
  // dnd-kit retains one settling clone; the new drag must already have
  // restored its source even when that previous clone has not finished.
  await expect(card).toHaveCSS('opacity', '1');
  const heldAnimations = await page.evaluate(() => {
    const animations = document.getAnimations().filter((animation) => {
      const effect = animation.effect;
      return (
        effect instanceof KeyframeEffect &&
        effect.target?.querySelector('.vk-kanban-drag-preview') &&
        animation.playState === 'paused'
      );
    });
    for (const animation of animations) animation.finish();
    return animations.length;
  });
  expect(heldAnimations).toBeGreaterThanOrEqual(1);
  await expect(page.locator('.vk-kanban-drag-preview')).toHaveCount(0);
  await expect(card).toHaveCSS('opacity', '1');
  await expect(page.getByTestId('move-count')).toHaveText('0');
});
