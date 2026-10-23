import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('timeline')).toContainText('Initial response');
});

test('multiline notices show a compact summary and keep their full diagnostic available', async ({
  page,
}) => {
  await page
    .getByRole('textbox', { name: 'Follow-up draft' })
    .fill('Unchanged draft');
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.updateBlock(
      fixture.block('warning', 'notice', {
        text: 'Codex is ignoring a legacy setting.\n\nDetailed configuration warning remains available.',
      })
    );
  });
  const summary = page.getByRole('button', {
    name: 'Codex is ignoring a legacy setting.',
    exact: true,
  });
  await expect(summary).toHaveAttribute('aria-expanded', 'false');
  await expect(
    page.getByText('Detailed configuration warning remains available.', {
      exact: false,
    })
  ).toHaveCount(0);
  await summary.click();
  await expect(summary).toHaveAttribute('aria-expanded', 'true');
  await expect(
    page.getByText('Detailed configuration warning remains available.', {
      exact: false,
    })
  ).toBeVisible();
  await summary.click();
  await expect(summary).toHaveAttribute('aria-expanded', 'false');
  await expect(
    page.getByRole('textbox', { name: 'Follow-up draft' })
  ).toHaveValue('Unchanged draft');
});

test('multiple runs use one session stream and token updates preserve draft and focus', async ({
  page,
}) => {
  await page
    .getByRole('textbox', { name: 'Follow-up draft' })
    .fill('Keep my draft');
  const before = await page.evaluate(
    () => (window as any).conversationFixture.renders.draft
  );
  await page.evaluate(() => {
    for (let index = 0; index < 100; ++index)
      (window as any).conversationFixture.append(' streamed');
  });
  await expect(page.getByTestId('timeline')).toContainText('streamed');
  await expect(
    page.getByRole('textbox', { name: 'Follow-up draft' })
  ).toHaveValue('Keep my draft');
  await expect(
    page.getByRole('textbox', { name: 'Follow-up draft' })
  ).toBeFocused();
  expect(
    await page.evaluate(() => (window as any).conversationFixture.renders.draft)
  ).toBe(before);
  expect(
    await page.evaluate(
      () => (window as any).conversationFixture.sockets.length
    )
  ).toBe(1);
  await page.getByRole('button', { name: 'Switch host' }).click();
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).conversationFixture.sockets.length)
    )
    .toBe(2);
  expect(
    await page.evaluate(
      () => (window as any).conversationFixture.sockets[1].transport.hostId
    )
  ).toBe('host-b');
});

test('native option IDs and question answers are posted with one captured identity and host', async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).conversationFixture;
    f.updateBlock(
      f.block('request', 'interaction', {
        interaction: {
          request_id: 'opaque/request',
          kind: 'question',
          status: 'pending',
          actionable: true,
          title: 'Choose deployment',
          options: [],
          questions: [
            {
              id: 'target-id',
              prompt: 'Where?',
              multiple: true,
              required: true,
              allow_free_text: true,
              options: [
                { id: 'native:stage', label: 'Staging' },
                { id: 'native:prod', label: 'Production' },
              ],
            },
          ],
        },
      }),
      true
    );
  });
  await page.getByRole('checkbox', { name: 'Staging' }).check();
  await page.getByRole('textbox', { name: 'Where?' }).fill('also local');
  await page.getByRole('button', { name: 'Send response' }).click();
  await expect(
    page.getByText('Response accepted. Waiting for the agent.')
  ).toBeVisible();
  const request = await page.evaluate(() =>
    (window as any).conversationFixture.requests.find((item: any) =>
      item.path.endsWith('/interaction')
    )
  );
  const data = JSON.parse(request.options.body);
  expect(request.options.hostId).toBe('host-a');
  expect(data.run_attempt_id).toBe('attempt-a');
  expect(data.reply).toEqual({
    request_id: 'opaque/request',
    answers: [
      { question_id: 'target-id', values: ['native:stage', 'also local'] },
    ],
  });
  expect(data.command_id).toBe(data.reply_id);
  await page.getByRole('button', { name: 'Check status' }).click();
  expect(
    await page.evaluate(
      () =>
        (window as any).conversationFixture.requests.filter((item: any) =>
          item.path.endsWith('/interaction')
        ).length
    )
  ).toBe(1);
});

test('uncertain delivery cannot resend and does not replace the follow-up draft', async ({
  page,
}) => {
  await page.goto('/?uncertain=1&attempt-rollover=1');
  await expect(page.getByTestId('timeline')).toContainText('Initial response');
  await page
    .getByRole('textbox', { name: 'Follow-up draft' })
    .fill('Unrelated draft');
  await page.evaluate(() => {
    const f = (window as any).conversationFixture;
    f.updateBlock(
      f.block('permission', 'interaction', {
        interaction: {
          request_id: 'native-permission',
          kind: 'permission',
          status: 'pending',
          actionable: true,
          title: 'Run command?',
          options: [
            { id: 'allow_once:42', label: 'Allow once' },
            { id: 'deny:42', label: 'Deny' },
          ],
          questions: [],
        },
      }),
      true
    );
  });
  await page.getByRole('radio', { name: 'Allow once' }).check();
  await page.getByRole('button', { name: 'Send response' }).click();
  await expect(page.getByText(/Delivery could not be confirmed/)).toBeVisible();
  await page.getByRole('button', { name: 'Check status' }).click();
  await expect(page.getByRole('button', { name: 'Send response' })).toHaveCount(
    0
  );
  await expect(
    page.getByRole('textbox', { name: 'Follow-up draft' })
  ).toHaveValue('Unrelated draft');
  const requests = await page.evaluate(() =>
    (window as any).conversationFixture.requests.filter((item: any) =>
      item.path.endsWith('/interaction')
    )
  );
  expect(requests).toHaveLength(1);
  const decision = JSON.parse(requests[0].options.body);
  expect(decision.reply.option_id).toBe('allow_once:42');
  expect(decision.run_attempt_id).toBe('attempt-a');
  expect(
    await page.evaluate(
      () =>
        (window as any).conversationFixture.state.runs[0].state
          .last_run_attempt_id
    )
  ).toBe('attempt-restarted');
});

test('large live text follows a bounded tail, coalesces detail reads and can return to its beginning', async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).conversationFixture;
    const text = `BEGINNING ${'long line '.repeat(10000)}LATEST-TAIL`;
    f.bodies.set('answer:text', text);
    const item = f.block('answer', 'text', { text: text.slice(0, 16384) });
    item.bodies = [
      { field: 'text', byte_length: text.length, preview_offset: 0 },
    ];
    f.updateBlock(item);
    for (let index = 0; index < 100; ++index) f.append(` delta-${index}`);
  });
  await expect(page.getByTestId('timeline')).toContainText('delta-99');
  await expect(
    page.getByText('Showing a section of the full output.')
  ).toBeVisible();
  expect((await page.getByTestId('timeline').innerText()).length).toBeLessThan(
    50000
  );
  const reads = await page.evaluate(
    () =>
      (window as any).conversationFixture.requests.filter((item: any) =>
        item.path.includes('/blocks/')
      ).length
  );
  expect(reads).toBeLessThan(8);
  await page.getByRole('button', { name: 'Show beginning' }).click();
  await expect(page.getByTestId('timeline')).toContainText('BEGINNING');
  await expect(
    page.getByRole('button', { name: 'Load more content' })
  ).toBeVisible();
});
