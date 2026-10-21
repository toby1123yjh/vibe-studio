import { expect, test } from '@playwright/test';

test('the real list virtualizes, preserves the backward-page anchor, and returns to the new live tail', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/?list=1');
  const list = page.locator('.list-host');
  await expect(list).toContainText('Message 600');
  await page.evaluate(() => (window as any).conversationFixture.widenWindow());
  await expect(list.locator('[data-row-index="499"]')).toHaveCount(1);
  await expect
    .poll(() => list.locator('[data-row-index]').count())
    .toBeLessThan(100);
  await page.evaluate(() => {
    (window as any).conversationFixture.list.current.scrollToEntryByPatchKey(
      'conversation-block:00101'
    );
  });
  await expect(list.getByText('Message 101', { exact: true })).toBeVisible();
  await page.evaluate(() => {
    (
      window as any
    ).conversationFixture.list.current.getScrollElement().scrollTop = 0;
  });
  await expect(
    page.getByRole('button', { name: 'Load earlier messages' })
  ).toBeInViewport();
  const anchor = await list
    .locator('[data-semantic-key]')
    .first()
    .evaluate((element) => ({
      key: (element as HTMLElement).dataset.semanticKey,
      top: element.getBoundingClientRect().top,
    }));
  await page.getByRole('button', { name: 'Load earlier messages' }).click();
  await expect(
    page.getByRole('button', { name: 'Return to latest messages' })
  ).toBeVisible();
  const anchored = list.locator(`[data-semantic-key="${anchor.key}"]`);
  await expect(anchored).toBeVisible();
  await expect
    .poll(() =>
      anchored.evaluate((element) => element.getBoundingClientRect().top)
    )
    .toBeCloseTo(anchor.top, 0);
  await page.evaluate(() => {
    const f = (window as any).conversationFixture;
    f.updateBlock({
      ...f.block('00601', 'text', { text: 'NEW LIVE TAIL 601' }),
      finished: true,
    });
  });
  await expect(list).not.toContainText('NEW LIVE TAIL 601');
  await page.getByRole('button', { name: 'Return to latest messages' }).click();
  await expect(list).toContainText('NEW LIVE TAIL 601');
  await expect
    .poll(() =>
      page.evaluate(() => {
        const element = (
          window as any
        ).conversationFixture.list.current.getScrollElement();
        return element.scrollHeight - element.clientHeight - element.scrollTop;
      })
    )
    .toBeLessThan(2);
  expect(
    await page.evaluate(
      () => (window as any).conversationFixture.sockets.length
    )
  ).toBe(1);
  expect(errors).toEqual([]);
});
