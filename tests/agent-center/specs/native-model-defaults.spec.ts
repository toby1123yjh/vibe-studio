import { expect, test, type Page } from '@playwright/test';

const session = (page: Page) =>
  page.getByRole('region', { name: 'Session agent selector', exact: true });

async function selectCodex(page: Page, query = '') {
  await page.goto(`/?signedOut=true&selectors=true${query}`);
  await session(page)
    .getByRole('button', { name: 'CODEX', exact: true })
    .click();
}

async function currentConfig(page: Page) {
  return JSON.parse(await page.getByTestId('session-selection').innerText());
}

test('native Codex model and Max replace discovery defaults without writing overrides on load or refresh', async ({
  page,
}) => {
  await selectCodex(page, '&refreshNative=true');
  await expect(
    session(page).getByRole('button', {
      name: 'gpt-6.1-sol · Max',
      exact: true,
    })
  ).toBeVisible();
  expect(await currentConfig(page)).toEqual({
    executor: 'CODEX',
    variant: null,
  });
  await expect(page.getByTestId('selector-overrides')).toHaveText('[]');

  await session(page)
    .getByRole('button', { name: 'Run settings', exact: true })
    .click();
  await page
    .getByRole('menuitem', { name: 'Diagnostics', exact: true })
    .focus();
  await page.keyboard.press('ArrowRight');
  await page
    .getByRole('menuitem', { name: 'Refresh native capabilities', exact: true })
    .click();
  await expect(
    session(page).getByRole('button', {
      name: 'gpt-6.1-sol · High',
      exact: true,
    })
  ).toBeVisible();
  expect(await currentConfig(page)).toEqual({
    executor: 'CODEX',
    variant: null,
  });
  await expect(page.getByTestId('selector-overrides')).toHaveText('[]');
});

test('native configured model absent from the catalog stays visible without invented capabilities', async ({
  page,
}) => {
  await selectCodex(page, '&nativeMissing=true');
  await session(page)
    .getByRole('button', { name: 'gpt-6.1-sol · Max', exact: true })
    .click();
  await expect(page.locator('[data-model-id="gpt-6.1-sol"]')).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Follow CLI · Max', exact: true })
  ).toHaveCount(0);
  await expect(page.getByTestId('selector-overrides')).toHaveText('[]');
});

test('reasoning inheritance shows its configured value and null bypasses preset reasoning', async ({
  page,
}) => {
  await selectCodex(page, '&preset=true');
  await session(page)
    .getByRole('button', { name: 'gpt-6.0 · High', exact: true })
    .click();
  await page.getByRole('button', { name: 'High', exact: true }).click();
  await page
    .getByRole('menuitem', { name: 'Follow CLI · Max', exact: true })
    .click();
  await page.keyboard.press('Escape');
  await expect(
    session(page).getByRole('button', { name: 'gpt-6.0 · Max', exact: true })
  ).toBeVisible();
  expect(await currentConfig(page)).toEqual({
    executor: 'CODEX',
    variant: null,
    reasoning_id: null,
  });
});

test('explicit effort survives model switches and explicit null model bypasses preset model', async ({
  page,
}) => {
  await selectCodex(page, '&preset=true');
  await session(page)
    .getByRole('button', { name: 'gpt-6.0 · High', exact: true })
    .click();
  await page.getByRole('button', { name: 'High', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Max', exact: true }).click();
  await page
    .locator('[data-model-id="gpt-6.1-sol"]')
    .getByRole('button', { name: 'gpt-6.1-sol', exact: true })
    .click();
  expect(await currentConfig(page)).toMatchObject({
    model_id: 'gpt-6.1-sol',
    reasoning_id: 'max',
  });
  await page
    .locator('[data-model-id="gpt-6.0"]')
    .getByRole('button', { name: 'gpt-6.0', exact: true })
    .click();
  expect(await currentConfig(page)).toMatchObject({
    model_id: 'gpt-6.0',
    reasoning_id: 'max',
  });
  await page
    .getByRole('button', { name: 'Follow CLI · gpt-6.1-sol', exact: true })
    .click();
  await page.keyboard.press('Escape');
  await expect(
    session(page).getByRole('button', {
      name: 'gpt-6.1-sol · Max',
      exact: true,
    })
  ).toBeVisible();
  expect(await currentConfig(page)).toMatchObject({
    model_id: null,
    reasoning_id: 'max',
  });
});

test('Claude advertised alias uses matching model-specific effort and never carries it into Sonnet', async ({
  page,
}) => {
  await page.goto('/?signedOut=true&selectors=true');
  await session(page)
    .getByRole('button', { name: 'CLAUDE_CODE', exact: true })
    .click();
  await session(page)
    .getByRole('button', { name: 'claude-opus-4-6 · Extra High', exact: true })
    .click();
  await expect(
    page.getByRole('button', { name: 'Follow CLI · Extra High', exact: true })
  ).toBeVisible();
  await page
    .locator('[data-model-id="opus"]')
    .getByRole('button', { name: /claude-opus-4-6/ })
    .click();
  expect(await currentConfig(page)).toEqual({
    executor: 'CLAUDE_CODE',
    variant: null,
    model_id: 'opus',
  });
  await page
    .locator('[data-model-id="sonnet"]')
    .getByRole('button', { name: /Sonnet \(alias\)/ })
    .click();
  await page.keyboard.press('Escape');
  await expect(
    session(page).getByRole('button', { name: 'Sonnet (alias)', exact: true })
  ).toBeVisible();
  expect(await currentConfig(page)).toEqual({
    executor: 'CLAUDE_CODE',
    variant: null,
    model_id: 'sonnet',
  });
});

test('Claude alias-only discovery is honest and preserves the native executable alias', async ({
  page,
}) => {
  await page.goto('/?signedOut=true&selectors=true&claudeAliasOnly=true');
  await session(page)
    .getByRole('button', { name: 'CLAUDE_CODE', exact: true })
    .click();
  await session(page)
    .getByRole('button', {
      name: 'Opus (alias) · Follow CLI config',
      exact: true,
    })
    .click();
  const row = page.locator('[data-model-id="opus"]');
  await expect(row).toContainText(
    'Native alias: opus. Exact model unconfirmed.'
  );
  await expect(row).not.toContainText('claude-opus-4-6');
  await row.getByRole('button', { name: /Opus \(alias\)/ }).click();
  await page.keyboard.press('Escape');
  expect(await currentConfig(page)).toEqual({
    executor: 'CLAUDE_CODE',
    variant: null,
    model_id: 'opus',
  });
});

test('Claude native versioned display names are retained without pretending to resolve the alias', async ({
  page,
}) => {
  await page.goto('/?signedOut=true&selectors=true&claudeNativeName=true');
  await session(page)
    .getByRole('button', { name: 'CLAUDE_CODE', exact: true })
    .click();
  await session(page)
    .getByRole('button', {
      name: 'Opus 4.6 (alias) · Follow CLI config',
      exact: true,
    })
    .click();
  const row = page.locator('[data-model-id="opus"]');
  await expect(row).toContainText('Opus 4.6 (alias)');
  await expect(row).toContainText('Exact model unconfirmed.');
  await row.getByRole('button', { name: /Opus 4.6 \(alias\)/ }).click();
  expect(await currentConfig(page)).toMatchObject({ model_id: 'opus' });
});

test('concrete model search keeps the owning provider and executable ID intact', async ({
  page,
}) => {
  await page.goto('/?signedOut=true&selectors=true&claudeSearch=true');
  await session(page)
    .getByRole('button', { name: 'CLAUDE_CODE', exact: true })
    .click();
  await session(page)
    .getByRole('button', {
      name: 'claude-opus-4-6 · Follow CLI config',
      exact: true,
    })
    .click();
  await page
    .getByPlaceholder('Filter by name or ID...')
    .fill('CLAUDE-OPUS-4-6');
  const row = page.locator('[data-model-key="native/opus"]');
  await expect(row).toBeVisible();
  await expect(page.locator('[data-model-id="sonnet"]')).toHaveCount(0);
  await row.getByRole('button', { name: /claude-opus-4-6/ }).click();
  expect(await currentConfig(page)).toMatchObject({ model_id: 'native/opus' });
});

test('task priority arrows have explanatory labels and do not consume the task click', async ({
  page,
}) => {
  await page.goto('/?signedOut=true&selectors=true&priorities=true');
  const priorities = page.getByRole('region', {
    name: 'Task priorities',
    exact: true,
  });
  await expect(priorities.getByRole('img')).toHaveCount(4);
  const icon = priorities.getByRole('img', {
    name: 'Low priority',
    exact: true,
  });
  await icon.hover();
  await expect(page.getByRole('tooltip')).toHaveText('Low priority');
  await icon.click();
  await expect(page.getByTestId('priority-clicks')).toHaveText('1');
  for (const label of ['Urgent priority', 'High priority', 'Medium priority']) {
    await expect(
      priorities.getByRole('img', { name: label, exact: true })
    ).toBeVisible();
  }
});

test('builtin list is labelled and does not pretend its default is native configuration', async ({
  page,
}) => {
  await selectCodex(page, '&builtin=true');
  await session(page)
    .getByRole('button', { name: 'Model not specified', exact: true })
    .click();
  await expect(page.getByRole('status')).toContainText(
    'Fallback list. Native capabilities are unconfirmed.'
  );
  await expect(page.getByTestId('selector-overrides')).toHaveText('[]');
  await page
    .locator('[data-model-id="gpt-6.0"]')
    .getByRole('button', { name: 'gpt-6.0', exact: true })
    .click();
  await page.keyboard.press('Escape');
  await expect(
    session(page).getByRole('button', {
      name: 'gpt-6.0 · Follow CLI config',
      exact: true,
    })
  ).toBeVisible();
  expect(await currentConfig(page)).toEqual({
    executor: 'CODEX',
    variant: null,
    model_id: 'gpt-6.0',
  });
});

test('native resolved default alias can show the concrete model even without a model setting in files', async ({
  page,
}) => {
  await page.goto('/?signedOut=true&selectors=true&nativeAlias=true');
  await session(page)
    .getByRole('button', { name: 'CLAUDE_CODE', exact: true })
    .click();
  await expect(
    session(page).getByRole('button', {
      name: 'claude-opus-4-6 · Extra High',
      exact: true,
    })
  ).toBeVisible();
  expect(await currentConfig(page)).toEqual({
    executor: 'CLAUDE_CODE',
    variant: null,
  });
  await expect(page.getByTestId('selector-overrides')).toHaveText('[]');
});

test('workflow uses the same native model and effort without freezing them into node overrides', async ({
  page,
}) => {
  await page.goto('/?signedOut=true&selectors=true');
  const workflow = page.getByRole('region', {
    name: 'Workflow agent selector',
    exact: true,
  });
  await workflow
    .getByRole('button', { name: 'Select executor', exact: true })
    .click();
  await page.getByRole('menuitem', { name: 'Codex', exact: true }).click();
  await expect(
    workflow.getByRole('button', { name: 'gpt-6.1-sol · Max', exact: true })
  ).toBeVisible();
  const config = JSON.parse(
    await page.getByTestId('workflow-selection').innerText()
  );
  expect(config.executor).toBe('CODEX');
  expect(config.model_id).toBeUndefined();
  expect(config.reasoning_id).toBeUndefined();
});
