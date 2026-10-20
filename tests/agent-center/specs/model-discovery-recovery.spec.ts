import { expect, test, type Page } from '@playwright/test';

const session = (page: Page) =>
  page.getByRole('region', { name: 'Session agent selector', exact: true });
const picker = (page: Page) => page.locator('[data-model-selector-popover]');

async function selectAgent(page: Page, agent: 'CODEX' | 'GEMINI', query = '') {
  await page.goto(`/?signedOut=true&selectors=true${query}`);
  await session(page).getByRole('button', { name: agent, exact: true }).click();
}

async function currentConfig(page: Page) {
  // innerText collapses repeated spaces in these rendered JSON outputs.
  return JSON.parse(
    (await page.getByTestId('session-selection').textContent()) ?? ''
  );
}

async function openEmptyGemini(page: Page) {
  await session(page)
    .getByRole('button', { name: 'Model not specified', exact: true })
    .click();
}

async function openManualEntry(page: Page) {
  await picker(page)
    .getByRole('button', { name: 'Enter a model ID…', exact: true })
    .click();
  return picker(page).getByRole('textbox', { name: 'Model ID', exact: true });
}

test('Gemini cold empty catalog keeps manual entry and native inheritance accessible', async ({
  page,
}) => {
  await selectAgent(page, 'GEMINI');
  await openEmptyGemini(page);
  await expect(
    picker(page).getByRole('button', { name: 'Follow CLI config', exact: true })
  ).toBeVisible();
  await expect(await openManualEntry(page)).toBeFocused();
  await expect(page.getByTestId('selector-overrides')).toHaveText('[]');
  expect(await currentConfig(page)).toEqual({
    executor: 'GEMINI',
    variant: null,
  });
});

test('Gemini configured-only model does not pretend to be a full catalog and can be replaced', async ({
  page,
}) => {
  await selectAgent(page, 'GEMINI', '&geminiConfigured=true');
  await session(page)
    .getByRole('button', { name: 'gemini-configured', exact: true })
    .click();
  await expect(picker(page).getByRole('status')).toContainText(
    'Models from local settings, not a complete capability list.'
  );
  await expect(
    picker(page).locator('[data-model-id="gemini-configured"]')
  ).toBeVisible();
  const input = await openManualEntry(page);
  await input.fill('Native/Custom:model');
  await picker(page)
    .getByRole('button', { name: 'Use model', exact: true })
    .click();
  expect(await currentConfig(page)).toEqual({
    executor: 'GEMINI',
    variant: null,
    model_id: 'Native/Custom:model',
  });
  await expect(page.getByTestId('input-modalities')).toHaveText('null');
});

test('Gemini manual IDs preserve slash, colon, case and whitespace without inventing effort or submitting chat', async ({
  page,
}) => {
  await selectAgent(page, 'GEMINI');
  await openEmptyGemini(page);
  const input = await openManualEntry(page);
  const modelId = '  Vendor/Family/Exact:latest  ';
  await input.fill(modelId);
  await input.press('Enter');
  await expect(picker(page)).toHaveCount(0);
  expect(await currentConfig(page)).toEqual({
    executor: 'GEMINI',
    variant: null,
    model_id: modelId,
  });
  await expect(page.getByTestId('selector-overrides')).toHaveJSProperty(
    'textContent',
    JSON.stringify([{ model_id: modelId }])
  );
  await expect(page.getByTestId('session-submit-count')).toHaveText('0');
  await expect(page.getByTestId('input-modalities')).toHaveText('null');
  await session(page)
    .getByRole('button', { name: 'Vendor/Family/Exact:latest', exact: true })
    .click();
  await expect(picker(page).locator('[data-model-id]')).toHaveCount(1);
  await expect(
    picker(page).getByRole('button', { name: /Follow CLI.*config/ })
  ).toBeVisible();
  await expect(picker(page).getByTitle('Reasoning supported')).toHaveCount(0);
});

test('Gemini rejects blank manual IDs inline and cancelling never modifies the request', async ({
  page,
}) => {
  await selectAgent(page, 'GEMINI');
  await openEmptyGemini(page);
  const input = await openManualEntry(page);
  await input.fill('   ');
  await input.press('Enter');
  await expect(input).toHaveAttribute('aria-invalid', 'true');
  await expect(picker(page).getByRole('alert')).toHaveText('Enter a model ID.');
  await expect(input).toBeFocused();
  await picker(page)
    .getByRole('button', { name: 'Cancel', exact: true })
    .click();
  await expect(picker(page).getByRole('textbox')).toHaveCount(0);
  await expect(
    picker(page).getByRole('button', { name: 'Enter a model ID…', exact: true })
  ).toBeFocused();
  await expect(page.getByTestId('selector-overrides')).toHaveText('[]');
  await expect(page.getByTestId('session-submit-count')).toHaveText('0');
});

test('Gemini Escape cancels only manual entry before closing the picker, not the surrounding chat', async ({
  page,
}) => {
  await selectAgent(page, 'GEMINI');
  await openEmptyGemini(page);
  const input = await openManualEntry(page);
  await input.fill('draft-not-selected');
  await input.press('Escape');
  await expect(picker(page)).toBeVisible();
  await expect(picker(page).getByRole('textbox')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(picker(page)).toHaveCount(0);
  await expect(page.getByTestId('outer-escape-count')).toHaveText('0');
  await expect(page.getByTestId('selector-overrides')).toHaveText('[]');
});

test('Gemini catalog selection is retained but manual IDs never borrow its image or reasoning capabilities', async ({
  page,
}) => {
  await selectAgent(page, 'GEMINI', '&geminiCatalog=true');
  await openEmptyGemini(page);
  await picker(page)
    .locator('[data-model-id="gemini-native-choice"]')
    .getByRole('button', { name: 'gemini-native-choice', exact: true })
    .click();
  await expect(page.getByTestId('input-modalities')).toHaveText(
    '["text","image"]'
  );
  const input = await openManualEntry(page);
  await input.fill('unlisted/manual:ID');
  await input.press('Enter');
  await expect(page.getByTestId('input-modalities')).toHaveText('null');
  expect(await currentConfig(page)).toEqual({
    executor: 'GEMINI',
    variant: null,
    model_id: 'unlisted/manual:ID',
  });
});

test('Gemini refresh changes inherited display but never replaces an explicit manual model', async ({
  page,
}) => {
  await selectAgent(page, 'GEMINI', '&geminiConfigured=true');
  await session(page)
    .getByRole('button', { name: 'gemini-configured', exact: true })
    .click();
  const input = await openManualEntry(page);
  await input.fill('requested/model:exact');
  await input.press('Enter');
  await session(page)
    .getByRole('button', { name: 'Custom', exact: true })
    .click();
  await page
    .getByRole('menuitem', { name: 'Refresh native capabilities', exact: true })
    .click();
  await expect(
    session(page).getByRole('button', {
      name: 'requested/model:exact',
      exact: true,
    })
  ).toBeVisible();
  expect(await currentConfig(page)).toEqual({
    executor: 'GEMINI',
    variant: null,
    model_id: 'requested/model:exact',
  });
  await session(page)
    .getByRole('button', { name: 'requested/model:exact', exact: true })
    .click();
  await expect(
    picker(page).getByRole('button', { name: /Follow CLI.*gemini-refreshed/ })
  ).toBeVisible();
});

test('Gemini native inheritance explicitly clears a preset model with null', async ({
  page,
}) => {
  await selectAgent(page, 'GEMINI', '&geminiPreset=true&geminiConfigured=true');
  await session(page)
    .getByRole('button', { name: 'gemini-preset', exact: true })
    .click();
  await picker(page)
    .getByRole('button', { name: /Follow CLI.*gemini-configured/ })
    .click();
  await page.keyboard.press('Escape');
  expect(await currentConfig(page)).toEqual({
    executor: 'GEMINI',
    variant: null,
    model_id: null,
  });
  await expect(
    session(page).getByRole('button', {
      name: 'gemini-configured',
      exact: true,
    })
  ).toBeVisible();
});

for (const scope of ['host', 'profile', 'workspace', 'session']) {
  test(`Gemini uncommitted model drafts do not leak across ${scope} changes`, async ({
    page,
  }) => {
    await selectAgent(page, 'GEMINI', '&scopeControls=true');
    await openEmptyGemini(page);
    const input = await openManualEntry(page);
    await input.fill('uncommitted-old-scope');
    // Route/Host changes can arrive while a Radix menu is open.
    await page
      .locator(`[data-fixture-scope="${scope}"]`)
      .evaluate((element: HTMLButtonElement) => element.click());
    await expect(picker(page)).toHaveCount(0);
    await openEmptyGemini(page);
    const freshInput = await openManualEntry(page);
    await expect(freshInput).toHaveValue('');
    await expect(page.getByTestId('selector-overrides')).toHaveText('[]');
    expect((await currentConfig(page)).model_id).toBeUndefined();
  });
}

test('Gemini discovery failure with no config still permits manual selection and retry', async ({
  page,
}) => {
  await selectAgent(
    page,
    'GEMINI',
    '&geminiError=true&noDiscoveryConfig=true&discoveryLoadingError=true'
  );
  await openEmptyGemini(page);
  await expect(picker(page).getByRole('alert')).toHaveText(
    'Gemini model discovery failed: host connection unavailable.'
  );
  const input = await openManualEntry(page);
  await input.fill('requested-before-discovery');
  await input.press('Enter');
  expect((await currentConfig(page)).model_id).toBe(
    'requested-before-discovery'
  );
  await session(page)
    .getByRole('button', { name: 'requested-before-discovery', exact: true })
    .click();
  await picker(page)
    .getByRole('button', { name: 'Refresh native capabilities', exact: true })
    .click();
  await expect(picker(page).getByRole('alert')).toHaveCount(0);
  expect((await currentConfig(page)).model_id).toBe(
    'requested-before-discovery'
  );
});

test('readonly Gemini exposes observations but no manual entry or model mutation', async ({
  page,
}) => {
  await selectAgent(page, 'GEMINI', '&readonly=true&geminiConfigured=true');
  await session(page)
    .getByRole('button', { name: 'gemini-configured', exact: true })
    .click();
  await expect(
    page.getByRole('button', { name: 'Enter a model ID…', exact: true })
  ).toHaveCount(0);
  await expect(
    page.getByText('Latest attempt · native report', { exact: true })
  ).toBeVisible();
  await expect(page.getByTestId('selector-overrides')).toHaveText('[]');
});

test('Codex discovery failure shows actual details and retry without losing explicit model or effort', async ({
  page,
}) => {
  await selectAgent(
    page,
    'CODEX',
    '&codexError=true&codexExplicitEffort=true&noDiscoveryConfig=true&discoveryLoadingError=true'
  );
  await session(page)
    .getByRole('button', { name: /gpt-6\.1-sol.*Xhigh/, exact: true })
    .click();
  await expect(picker(page).getByRole('alert')).toHaveText(
    'Codex model discovery failed: native request timed out.'
  );
  await expect(
    picker(page).getByRole('button', { name: 'Enter a model ID…', exact: true })
  ).toHaveCount(0);
  const expectedConfig = {
    executor: 'CODEX',
    variant: null,
    model_id: 'gpt-6.1-sol',
    reasoning_id: 'xhigh',
  };
  expect(await currentConfig(page)).toEqual(expectedConfig);
  await picker(page)
    .getByRole('button', { name: 'Refresh native capabilities', exact: true })
    .click();
  await expect(picker(page).getByRole('alert')).toHaveCount(0);
  await expect(picker(page).locator('[data-model-id="gpt-6.0"]')).toBeVisible();
  expect(await currentConfig(page)).toEqual(expectedConfig);
  await expect(page.getByTestId('selector-overrides')).toHaveText('[]');
});
