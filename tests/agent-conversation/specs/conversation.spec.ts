import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('timeline')).toContainText('Initial response');
});

test('tool groups show useful activity without counters and reveal details on demand', async ({
  page,
}) => {
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.updateBlock({
      ...fixture.block('tool-a', 'tool', {
        tool_name: 'Read file',
        tool_kind: 'file_read',
        status: 'succeeded',
      }),
      finished: true,
    });
    const output = `BEGINNING ${'full tool output '.repeat(5000)}END`;
    fixture.bodies.set('tool-b:output_text', output);
    fixture.updateBlock({
      ...fixture.block('tool-b', 'tool', {
        tool_name: 'Search source',
        status: 'succeeded',
        locations: [{ path: String.raw`\\?\D:\project\source.ts` }],
        arguments: { query: 'retained argument' },
        output_text: output.slice(0, 16384),
      }),
      finished: true,
      bodies: [{ field: 'output_text', byte_length: output.length }],
    });
  });
  const group = page.getByRole('button', { name: /Tool activity/ });
  await expect(group).toContainText(
    String.raw`Search source · D:\project\source.ts`
  );
  await expect(group).not.toContainText(/\d+ tool calls?|\d+\/\d+|finished/);
  await expect(group).toHaveAttribute('aria-expanded', 'false');
  await expect(
    page.getByRole('button', { name: 'Search source', exact: true })
  ).toHaveCount(0);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).conversationFixture.requests.filter((request: any) =>
            request.path.includes('/blocks/')
          ).length
      )
    )
    .toBe(0);
  await group.focus();
  await group.press('Enter');
  await expect(group).toHaveAttribute('aria-expanded', 'true');
  await page
    .getByRole('button', { name: 'Search source', exact: true })
    .click();
  await expect(
    page.getByText('retained argument', { exact: false })
  ).toBeVisible();
  await page.getByRole('button', { name: 'Load more content' }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).conversationFixture.requests.filter((request: any) =>
            request.path.includes('/blocks/')
          ).length
      )
    )
    .toBeGreaterThan(0);
  await group.focus();
  await group.press('Space');
  await expect(group).toHaveAttribute('aria-expanded', 'false');
});

test('collapsed tool groups follow canonical running, failure and terminal state', async ({
  page,
}) => {
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.updateBlock(
      fixture.block('tool-a', 'tool', {
        tool_name: 'Read file',
        status: 'running',
      })
    );
    fixture.updateBlock(
      fixture.block('tool-b', 'tool', {
        tool_name: 'Run command',
        status: 'failed',
      })
    );
  });
  const group = page.getByRole('button', { name: /Tool activity/ });
  await expect(group).toContainText('Read file');
  await expect(group).toContainText('Tool error');
  await expect(group.locator('.vk-activity-text')).toHaveCount(1);
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.state.runs[0].state.status = 'succeeded';
    fixture.updateBlock(
      fixture.block('unrelated', 'notice', { text: 'Run finished' })
    );
  });
  await expect(group.locator('.vk-activity-text')).toHaveCount(0);
  await expect(group).toContainText('Tool error');
  await expect(group).toContainText('Run command');
});

test('collapsed approval cue requires the matching unresolved canonical request', async ({
  page,
}) => {
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.state.runs[0].state.status = 'awaiting_approval';
    fixture.updateBlock(
      fixture.block('tool-a', 'tool', {
        tool_name: 'Run command',
        tool_call_id: 'call-a',
        status: 'waiting_approval',
      })
    );
    fixture.updateBlock(
      fixture.block('request', 'interaction', {
        interaction: {
          request_id: 'permission-a',
          tool_call_id: 'call-a',
          kind: 'permission',
          status: 'pending',
          actionable: false,
          title: 'Command permission',
          options: [],
          questions: [],
        },
      }),
      true
    );
  });
  const group = page.getByRole('button', { name: /Tool activity/ });
  await expect(group).toContainText('Permission required');
  await expect(group.locator('.vk-activity-text')).toHaveCount(1);
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.state.pending_interactions = [];
    fixture.updateBlock(
      fixture.block('unrelated', 'notice', { text: 'Request resolved' })
    );
  });
  await expect(group).not.toContainText('Permission required');
  await expect(group.locator('.vk-activity-text')).toHaveCount(0);
  await expect(group).toContainText('Result not confirmed');
});

test('tool locations display ordinary paths while native arguments stay intact', async ({
  page,
}) => {
  const rawPath = String.raw`\\?\D:\目录 with spaces\file.txt`;
  await page.evaluate((path) => {
    const fixture = (window as any).conversationFixture;
    fixture.updateBlock(
      fixture.block('file-tool', 'tool', {
        tool_name: 'Read file',
        tool_kind: 'file_read',
        locations: [{ path, line: 42 }],
        raw_arguments: path,
        arguments: { path },
      })
    );
  }, rawPath);
  await page.getByRole('button', { name: 'Read file', exact: true }).click();
  await expect(
    page.getByText(String.raw`D:\目录 with spaces\file.txt:42`, { exact: true })
  ).toBeVisible();
  await expect(page.getByText(rawPath, { exact: true })).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).conversationFixture.state.blocks.find(
          (block: { block_id: string }) => block.block_id === 'file-tool'
        ).fields.arguments.path
    )
  ).toBe(rawPath);
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

test('reasoning previews stay compact and manual expansion survives streaming and completion', async ({
  page,
}) => {
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.updateBlock(
      fixture.block('reasoning', 'reasoning', {
        text: '**Inspect current state**\n\nStill considering',
      })
    );
  });
  const reasoning = page.locator('[data-conversation-block="reasoning"]');
  const header = reasoning.getByRole('button', { name: /Thinking/ });
  await expect(header).toHaveAttribute('aria-expanded', 'false');
  await expect(header).toContainText('Inspect current state');
  await expect(header).not.toContainText('**');
  await expect(reasoning.getByTestId('markdown')).toHaveCount(0);
  const preview = await header.textContent();
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.updateBlock(
      fixture.block('reasoning', 'reasoning', {
        text: '**Inspect current state**\n\nStill considering another option',
      })
    );
  });
  await expect(header).toHaveText(preview!);
  await header.focus();
  await header.press('Space');
  await expect(header).toHaveAttribute('aria-expanded', 'true');
  const bodyId = await header.getAttribute('aria-controls');
  expect(bodyId).toBeTruthy();
  await expect(page.locator(`[id="${bodyId}"]`)).toContainText(
    'Still considering another option'
  );
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.updateBlock(
      fixture.block('reasoning', 'reasoning', {
        text: '**Inspect current state**\n\nThe chosen option is now clear.',
      })
    );
    fixture.state.runs[0].state.status = 'succeeded';
    fixture.updateBlock(
      fixture.block('finished', 'notice', { text: 'Run finished' })
    );
  });
  await expect(header).toHaveAttribute('aria-expanded', 'true');
  await expect(header).not.toHaveAttribute('data-active', 'true');
  await expect(reasoning).toContainText('The chosen option is now clear.');
  await expect(reasoning.getByRole('button', { name: /Thinking/ })).toHaveCount(
    1
  );
  await page.setViewportSize({ width: 520, height: 720 });
  await expect(header).toHaveAttribute('aria-expanded', 'true');
  await header.press('Enter');
  await expect(header).toHaveAttribute('aria-expanded', 'false');
  await expect(reasoning.getByTestId('markdown')).toHaveCount(0);
});

test('individual tool disclosures preserve independent expansion and stop stale activity from run metadata', async ({
  page,
}) => {
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.updateBlock(
      fixture.block('tool-a', 'tool', {
        tool_name: 'Read current file',
        status: 'running',
        output_text: 'First tool output',
      })
    );
    fixture.updateBlock(
      fixture.block('tool-b', 'tool', {
        tool_name: 'Search related file',
        status: 'running',
        output_text: 'Second tool output',
      })
    );
  });
  const group = page.getByRole('button', { name: /Tool activity/ });
  await group.click();
  const first = page.getByRole('button', {
    name: 'Read current file',
    exact: true,
  });
  const second = page.getByRole('button', {
    name: 'Search related file',
    exact: true,
  });
  await first.click();
  await second.click();
  await expect(first).toHaveAttribute('aria-expanded', 'true');
  await expect(second).toHaveAttribute('aria-expanded', 'true');
  const firstBodyId = await first.getAttribute('aria-controls');
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.updateBlock(
      fixture.block('tool-a', 'tool', {
        tool_name: 'Read current file',
        status: 'running',
        output_text: 'First tool output has streamed more text',
      })
    );
    fixture.state.runs[0].state.status = 'cancelled';
    fixture.updateBlock(
      fixture.block('cancelled', 'notice', { text: 'Run was cancelled' })
    );
  });
  await expect(first).toHaveAttribute('aria-controls', firstBodyId!);
  await expect(first).toHaveAttribute('aria-expanded', 'true');
  await expect(second).toHaveAttribute('aria-expanded', 'true');
  await expect(group).not.toHaveAttribute('data-active', 'true');
  await expect(first).not.toHaveAttribute('data-active', 'true');
  await expect(second).not.toHaveAttribute('data-active', 'true');
  await expect(first).toHaveAccessibleDescription('Result not confirmed');
  await expect(second).toHaveAccessibleDescription('Result not confirmed');
  await expect(page.locator(`[id="${firstBodyId}"]`)).toContainText(
    'First tool output has streamed more text'
  );
  await group.click();
  await group.click();
  await expect(first).toHaveAttribute('aria-expanded', 'true');
  await expect(second).toHaveAttribute('aria-expanded', 'true');
});

test('large reasoning bodies load only after expansion and old attempts never animate', async ({
  page,
}) => {
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    const text = `**Reasoning preview**\n\n${'Retained detailed reasoning. '.repeat(3000)}`;
    fixture.bodies.set('large-reasoning:text', text);
    fixture.updateBlock({
      ...fixture.block('large-reasoning', 'reasoning', {
        text: text.slice(0, 16384),
      }),
      bodies: [{ field: 'text', byte_length: text.length }],
    });
  });
  const reasoning = page.locator('[data-conversation-block="large-reasoning"]');
  const header = reasoning.getByRole('button', { name: /Thinking/ });
  await expect(header).toHaveAttribute('aria-expanded', 'false');
  await expect(reasoning.getByTestId('markdown')).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        (window as any).conversationFixture.requests.filter((request: any) =>
          request.path.includes('/blocks/')
        ).length
    )
  ).toBe(0);
  await header.click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).conversationFixture.requests.filter((request: any) =>
            request.path.includes('/blocks/')
          ).length
      )
    )
    .toBeGreaterThan(0);
  await expect(reasoning).toContainText('Retained detailed reasoning.');
  await page.evaluate(() => {
    const fixture = (window as any).conversationFixture;
    fixture.state.runs[0].state.last_run_attempt_id = 'new-attempt';
    fixture.updateBlock(
      fixture.block('restarted', 'notice', { text: 'Another attempt started' })
    );
  });
  await expect(header).not.toHaveAttribute('data-active', 'true');
  await expect(header).toHaveAttribute('aria-expanded', 'true');
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
