import { expect, test, type Page } from '@playwright/test';

const savedA = {
  executor: 'CODEX',
  variant: 'PLAN',
  model_id: 'saved-model',
  agent_id: 'saved-agent',
  reasoning_id: 'high',
  permission_policy: 'SUPERVISED',
  provider_options: null,
};
const savedB = {
  executor: 'CLAUDE_CODE',
  variant: 'CUSTOM',
  model_id: 'claude-model',
  agent_id: 'claude-agent',
  reasoning_id: null,
  permission_policy: null,
  provider_options: null,
};

async function fixtureBackend(page: Page) {
  const state = {
    records: new Map<string, Record<string, unknown> | null>([
      ['local:a', { ...savedA }],
      ['local:b', { ...savedB }],
      ['remote-host:a', { ...savedA, model_id: 'remote-model' }],
    ]),
    writes: [] as Record<string, unknown>[],
    failing: false,
    gate: undefined as Promise<void> | undefined,
    writeGate: undefined as Promise<void> | undefined,
    readKeys: [] as string[],
    workflowRecords: new Map<string, Record<string, unknown> | null>(),
    workflowReadKeys: [] as string[],
    workflowFailing: false,
  };
  // These are isolated fixtures: no request may reach real project APIs.
  await page.route('**/api/**', (route) => route.abort());
  await page.route(/\/api\/(?:host\/[^/]+\/)?workflow-management\/sessions\/[^/]+\/context/, async (route) => {
    const url = new URL(route.request().url());
    const match = url.pathname.match(/(?:\/host\/([^/]+))?\/workflow-management\/sessions\/([^/]+)\/context/)!;
    const key = `${match[1] ?? 'local'}:${match[2]}`;
    state.workflowReadKeys.push(key);
    await route.fulfill({
      status: state.workflowFailing ? 503 : 200,
      json: state.workflowFailing
        ? { success: false, message: 'Workflow binding unavailable' }
        : { success: true, data: state.workflowRecords.get(key) ?? null },
    });
  });
  await page.route('**/__fixture/session-config/**', async (route) => {
    const url = new URL(route.request().url());
    const key = `${url.searchParams.get('host')}:${url.pathname.split('/').at(-1)}`;
    if (route.request().method() === 'POST') {
      if (state.writeGate) await state.writeGate;
      const config = route.request().postDataJSON();
      state.records.set(key, config);
      state.writes.push(config);
      await route.fulfill({ json: config });
      return;
    }
    const record = state.records.get(key) ?? null;
    state.readKeys.push(key);
    if (state.gate) await state.gate;
    await route.fulfill({ status: state.failing ? 503 : 200, json: record });
  });
  return state;
}

async function config(page: Page) {
  return JSON.parse(await page.getByTestId('config').innerText());
}

test('prepared workflow main Session preserves its full snapshot before and after the first turn', async ({ page }) => {
  const backend = await fixtureBackend(page);
  backend.records.set('local:a', null);
  backend.workflowRecords.set('local:a', { main_agent_config: savedA });
  await page.goto('/?null-draft&preferred');
  await expect.poll(() => config(page)).toEqual(savedA);
  await expect(page.getByTestId('config-locked')).toHaveText('true');
  await page.getByRole('button', { name: 'Choose settings' }).click();
  await page.getByRole('button', { name: 'Follow CLI' }).click();
  await page.getByRole('button', { name: 'Choose Claude' }).click();
  await expect.poll(() => config(page)).toEqual(savedA);
  await page.getByRole('button', { name: 'Send fixture message' }).click();
  await expect.poll(() => backend.writes).toEqual([savedA]);
  // A later attempt/preset cannot silently replace the immutable main binding.
  backend.records.set('local:a', { ...savedA, model_id: 'new-template-model' });
  await page.getByRole('button', { name: 'Reopen', exact: true }).click();
  await expect.poll(() => config(page)).toEqual(savedA);
  expect(backend.workflowReadKeys.every((key) => key === 'local:a')).toBe(true);
});

test('workflow context read failure blocks sending and retry never falls back to a default', async ({ page }) => {
  const backend = await fixtureBackend(page);
  backend.workflowRecords.set('local:a', { main_agent_config: savedA });
  backend.workflowFailing = true;
  await page.goto('/');
  await expect(page.getByTestId('state')).toHaveText('error');
  await expect(page.getByRole('button', { name: 'Send fixture message' })).toBeDisabled();
  expect(backend.writes).toEqual([]);
  backend.workflowFailing = false;
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect.poll(() => config(page)).toEqual(savedA);
  await expect(page.getByTestId('config-locked')).toHaveText('true');
});

test('saved settings survive reopening and reload with no draft', async ({
  page,
}) => {
  await fixtureBackend(page);
  await page.goto('/');
  await expect.poll(() => config(page)).toEqual(savedA);
  await page.getByRole('button', { name: 'Reopen', exact: true }).click();
  await expect.poll(() => config(page)).toEqual(savedA);
  await page.reload();
  await expect.poll(() => config(page)).toEqual(savedA);
});

test('first message clears its draft but subsequent entry restores the selected settings', async ({
  page,
}) => {
  const backend = await fixtureBackend(page);
  backend.records.set('local:a', null);
  await page.goto('/?new');
  await page.getByRole('button', { name: 'Choose Codex' }).click();
  await page.getByRole('button', { name: 'Choose settings' }).click();
  await expect
    .poll(() => config(page))
    .toMatchObject({
      executor: 'CODEX',
      model_id: 'chosen-model',
      agent_id: 'chosen-agent',
    });
  await page.getByRole('button', { name: 'Send fixture message' }).click();
  await expect.poll(() => backend.writes.length).toBe(1);
  const sent = backend.writes[0];
  await page.getByRole('button', { name: 'Reopen', exact: true }).click();
  await expect.poll(() => config(page)).toEqual(sent);
  await page.goto('/');
  await expect.poll(() => config(page)).toEqual(sent);
});

test('same component session and host switches do not retain another selection', async ({
  page,
}) => {
  await fixtureBackend(page);
  await page.goto('/');
  await expect.poll(() => config(page)).toEqual(savedA);
  await page.getByRole('button', { name: 'Choose settings' }).click();
  await expect
    .poll(() => config(page))
    .toMatchObject({ model_id: 'chosen-model' });
  await page.getByRole('button', { name: 'Session B' }).click();
  await expect.poll(() => config(page)).toEqual(savedB);
  await page.getByRole('button', { name: 'Session A' }).click();
  await expect
    .poll(() => config(page))
    .toMatchObject({ executor: 'CODEX', variant: 'PLAN' });
  await page.getByRole('button', { name: 'Reopen', exact: true }).click();
  await expect.poll(() => config(page)).toEqual(savedA);
  await page.getByRole('button', { name: 'Switch host' }).click();
  await expect
    .poll(() => config(page))
    .toEqual({ ...savedA, model_id: 'remote-model' });
});

test('loading and read failure never expose a default send config; retry restores it', async ({
  page,
}) => {
  const backend = await fixtureBackend(page);
  let release!: () => void;
  backend.gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.goto('/');
  await expect(page.getByTestId('state')).toHaveText('loading');
  await expect(
    page.getByRole('button', { name: 'Send fixture message' })
  ).toBeDisabled();
  await expect(page.getByTestId('config')).toHaveText('null');
  backend.failing = true;
  release();
  await expect(page.getByTestId('state')).toHaveText('error');
  await expect(
    page.getByRole('button', { name: 'Send fixture message' })
  ).toBeDisabled();
  expect(backend.writes).toEqual([]);
  backend.failing = false;
  backend.gate = undefined;
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect.poll(() => config(page)).toEqual(savedA);
  await expect(
    page.getByRole('button', { name: 'Send fixture message' })
  ).toBeEnabled();
});

test('another-profile draft and preferred defaults cannot override the bound session', async ({
  page,
}) => {
  await fixtureBackend(page);
  await page.goto('/?wrong-draft&preferred');
  await expect.poll(() => config(page)).toEqual(savedA);
  await page.getByRole('button', { name: 'Choose Claude' }).click();
  await page.getByRole('button', { name: 'Change preset' }).click();
  await expect.poll(() => config(page)).toEqual(savedA);
});

test('explicit null draft overrides remain Follow CLI rather than preset values', async ({
  page,
}) => {
  await fixtureBackend(page);
  await page.goto('/?null-draft');
  await expect
    .poll(() => config(page))
    .toEqual({
      executor: 'CODEX',
      variant: 'PLAN',
      model_id: null,
      agent_id: null,
      reasoning_id: null,
      permission_policy: null,
      provider_options: null,
    });
});

test('profile-only requests retain preset model and effort inheritance', async ({
  page,
}) => {
  const backend = await fixtureBackend(page);
  backend.records.set('local:a', { executor: 'CODEX', variant: 'PLAN' });
  await page.goto('/');
  await expect
    .poll(() => config(page))
    .toEqual({
      executor: 'CODEX',
      variant: 'PLAN',
      model_id: 'preset-model',
      agent_id: null,
      reasoning_id: 'preset-reasoning',
      permission_policy: null,
      provider_options: null,
    });
});

test('empty existing session uses its own provider while a new session uses defaults', async ({
  page,
}) => {
  const backend = await fixtureBackend(page);
  backend.records.set('local:a', null);
  await page.goto('/');
  await expect(page.getByTestId('state')).toHaveText('ready');
  await expect.poll(() => config(page)).toMatchObject({ executor: 'CODEX' });
  await page.getByRole('button', { name: 'Choose Claude' }).click();
  await expect.poll(() => config(page)).toMatchObject({ executor: 'CODEX' });
  await page.goto('/?new');
  await expect
    .poll(() => config(page))
    .toMatchObject({ executor: 'CLAUDE_CODE' });
});

test('omitted draft model and effort inherit the saved request', async ({
  page,
}) => {
  await fixtureBackend(page);
  await page.goto('/?omitted-draft');
  await expect
    .poll(() => config(page))
    .toEqual({
      executor: 'CODEX',
      variant: 'PLAN',
      model_id: 'saved-model',
      agent_id: null,
      reasoning_id: 'high',
      permission_policy: null,
      provider_options: null,
    });
});

test('late old-session response cannot replace the selected session config', async ({
  page,
}) => {
  const backend = await fixtureBackend(page);
  let release!: () => void;
  backend.gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.goto('/');
  await expect.poll(() => backend.readKeys).toContain('local:a');
  backend.gate = undefined;
  await page.getByRole('button', { name: 'Session B' }).click();
  await expect.poll(() => config(page)).toEqual(savedB);
  const lateResponse = page.waitForResponse(
    '**/__fixture/session-config/a?host=local'
  );
  release();
  await lateResponse;
  await expect.poll(() => config(page)).toEqual(savedB);
});

test('real create hook refreshes config read before the first run was saved', async ({
  page,
}) => {
  const backend = await fixtureBackend(page);
  backend.records.set('local:a', null);
  let release!: () => void;
  backend.writeGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.goto('/?new&create-race');
  await page.getByRole('button', { name: 'Choose Codex' }).click();
  await page.getByRole('button', { name: 'Choose settings' }).click();
  await expect
    .poll(() => config(page))
    .toMatchObject({ model_id: 'chosen-model' });
  await page.getByRole('button', { name: 'Send fixture message' }).click();
  await expect.poll(() => backend.readKeys).toContain('local:a');
  await expect(page.getByTestId('state')).toHaveText('ready');
  expect(backend.records.get('local:a')).toBeNull();
  release();
  await expect(page.getByTestId('create-state')).toHaveText('success');
  await expect.poll(() => config(page)).toEqual(backend.writes[0]);
  await expect.poll(() => backend.readKeys.length).toBeGreaterThan(1);
});

test('queued configuration is scoped by host even with the same session ID', async ({
  page,
}) => {
  await fixtureBackend(page);
  let release!: () => void;
  const remoteGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route('**/__fixture/queue/**', async (route) => {
    const remote =
      new URL(route.request().url()).searchParams.get('host') === 'remote-host';
    if (remote) await remoteGate;
    await route.fulfill({
      json: {
        status: 'queued',
        message: {
          session_id: 'a',
          queued_at: '2026-09-07T00:00:00Z',
          data: {
            message: 'fixture queue only',
            executor_config: {
              ...savedA,
              model_id: remote ? 'remote-queued' : 'local-queued',
            },
          },
        },
      },
    });
  });
  await page.goto('/?queued');
  await expect
    .poll(() => config(page))
    .toMatchObject({ model_id: 'local-queued' });
  await page.getByRole('button', { name: 'Switch host' }).click();
  await expect(page.getByTestId('queue-config')).toHaveText('null');
  await expect
    .poll(() => config(page))
    .toMatchObject({ model_id: 'remote-model' });
  release();
  await expect
    .poll(() => config(page))
    .toMatchObject({ model_id: 'remote-queued' });
});
