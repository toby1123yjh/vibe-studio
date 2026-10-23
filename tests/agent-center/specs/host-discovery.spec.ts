import { expect, test } from '@playwright/test';

declare global {
  interface Window {
    agentCenterCalls: { relay: number; paired: number; garage: number };
  }
}

test('signed-out local users do not query cloud discovery', async ({
  page,
}) => {
  await page.goto('/?signedOut=true&cloudError=true');
  await expect(page.locator('#agent-center-title')).toBeVisible();
  await page.getByRole('button', { name: 'Rescan host', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Rescan host', exact: true })
  ).toBeEnabled();
  expect(await page.evaluate(() => window.agentCenterCalls.relay)).toBe(0);
  await expect(page.getByText(/Could not load remote hosts/)).toHaveCount(0);
});

test('local-only configuration skips both remote lists and keeps local scanning usable', async ({
  page,
}) => {
  await page.goto('/?remoteConfigured=false&cloudError=true');
  await expect(page.locator('#agent-center-title')).toBeVisible();
  const discovery = page.getByTestId('host-discovery-probe');
  await expect(discovery).toHaveAttribute('data-canonical', 'true');
  await expect(discovery).toHaveAttribute('data-remote-error', 'false');
  await expect(discovery).toHaveAttribute('data-remote-loading', 'false');
  await expect(discovery).toHaveAttribute('data-remote-can-retry', 'false');
  await expect(page.getByText(/Could not load remote hosts/)).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Retry', exact: true })
  ).toHaveCount(0);
  await page.getByRole('button', { name: 'Rescan host', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Rescan host', exact: true })
  ).toBeEnabled();
  expect(await page.evaluate(() => window.agentCenterCalls)).toMatchObject({
    relay: 0,
    paired: 0,
  });
});

test('remote discovery starts when service configuration becomes available', async ({
  page,
}) => {
  await page.goto('/?remoteConfigured=false&remoteConfigControls=true');
  const discovery = page.getByTestId('host-discovery-probe');
  await expect(discovery).toHaveAttribute('data-host-count', '1');
  expect(await page.evaluate(() => window.agentCenterCalls.relay)).toBe(0);
  await page.getByRole('button', { name: 'Toggle remote service' }).click();
  await expect(discovery).toHaveAttribute('data-remote-canonical', 'true');
  await expect(discovery).toHaveAttribute('data-host-count', '2');
  expect(await page.evaluate(() => window.agentCenterCalls)).toMatchObject({
    relay: 1,
    paired: 1,
  });
});

for (const cloudError of [false, true]) {
  test(`removing service configuration hides cached remote ${cloudError ? 'errors' : 'hosts'}`, async ({
    page,
  }) => {
    await page.goto(
      `/?remoteConfigControls=true${cloudError ? '&cloudError=true' : ''}`
    );
    const discovery = page.getByTestId('host-discovery-probe');
    if (cloudError) {
      await expect(discovery).toHaveAttribute('data-remote-error', 'true');
    } else {
      await expect(discovery).toHaveAttribute('data-host-count', '2');
    }
    const calls = await page.evaluate(() => window.agentCenterCalls);
    await page.getByRole('button', { name: 'Toggle remote service' }).click();
    await expect(discovery).toHaveAttribute('data-host-count', '1');
    await expect(discovery).toHaveAttribute('data-remote-canonical', 'false');
    await expect(discovery).toHaveAttribute('data-remote-error', 'false');
    await expect(discovery).toHaveAttribute('data-remote-loading', 'false');
    await expect(discovery).toHaveAttribute('data-remote-retrying', 'false');
    await expect(discovery).toHaveAttribute('data-remote-can-retry', 'false');
    await expect(page.getByText(/Could not load remote hosts/)).toHaveCount(0);
    expect(await page.evaluate(() => window.agentCenterCalls)).toMatchObject({
      relay: calls.relay,
      paired: calls.paired,
    });
  });
}

test('remote web keeps same-origin remote discovery enabled', async ({
  page,
}) => {
  await page.goto('/?runtime=remote&remoteConfigured=false');
  await expect(page.locator('#agent-center-title')).toBeVisible();
  await expect(page.getByTestId('host-discovery-probe')).toHaveAttribute(
    'data-remote-canonical',
    'true'
  );
  expect(await page.evaluate(() => window.agentCenterCalls)).toMatchObject({
    relay: 1,
    paired: 0,
  });
});

test('cloud failure has separate retry and does not degrade local scan', async ({
  page,
}) => {
  await page.goto('/?cloudError=true');
  await expect(page.locator('#agent-center-title')).toBeVisible();
  await expect(page.getByText(/Could not load remote hosts/)).toBeVisible();
  const discovery = page.getByTestId('host-discovery-probe');
  await expect(discovery).toHaveAttribute('data-canonical', 'true');
  await expect(discovery).toHaveAttribute('data-host-error', 'false');
  await expect(discovery).toHaveAttribute('data-remote-error', 'true');
  await expect(page.getByText('Could not rescan this host.')).toHaveCount(0);
  await page.getByRole('button', { name: 'Rescan host', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Rescan host', exact: true })
  ).toBeEnabled();
  expect(await page.evaluate(() => window.agentCenterCalls.relay)).toBe(1);
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.agentCenterCalls.relay))
    .toBe(2);
  await expect(page.getByText('PRIVATE_CLOUD_ERROR')).toHaveCount(0);
});

for (const query of [
  'cloudError=true',
  'signedOut=true',
  'remoteConfigured=false',
]) {
  test(`explicit remote target never falls back to local: ${query}`, async ({
    page,
  }) => {
    await page.goto(`/?host=remote-fixture&${query}`);
    await expect(
      page.getByRole('status').or(page.getByRole('alert'))
    ).toBeVisible();
    await expect(page.locator('#agent-center-title')).toHaveCount(0);
    expect(await page.evaluate(() => window.agentCenterCalls.garage)).toBe(0);
  });
}

test('real local scan failure remains visible', async ({ page }) => {
  await page.goto('/?signedOut=true&scanError=true');
  await expect(
    page.getByRole('heading', { name: 'Codex', exact: true })
  ).toBeVisible();
  await page.getByRole('button', { name: 'Rescan host', exact: true }).click();
  await expect(
    page.locator('.vk-agent-center__refresh-diagnostics')
  ).toBeVisible();
  await expect(page.getByText('PRIVATE_SCAN_ERROR')).toHaveCount(0);
});
