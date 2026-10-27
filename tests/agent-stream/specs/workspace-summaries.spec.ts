import { expect, test } from '@playwright/test';

declare global {
  interface Window {
    workspaceSummaryFixture: {
      requests: Array<{ hostId: string | null; archived: boolean }>;
      initialize(hostId?: string): void;
      capture(): number;
      compare(): {
        activeList: boolean;
        archivedList: boolean;
        activeRow: boolean;
        archivedRow: boolean;
        listChanges: number;
      };
      change(hostId: string, archived: boolean, filesChanged: number): void;
      refetch(): Promise<void>;
    };
  }
}

test('identical summary refreshes retain both lists and rows; changed stats still update', async ({
  page,
}) => {
  await page.goto('/workspaces.html');
  await expect(page.getByTestId('state')).toHaveText('loading');
  await page.evaluate(() => window.workspaceSummaryFixture.initialize());
  await expect(page.getByTestId('state')).toHaveText('ready');
  await expect(page.getByTestId('files')).toHaveText('2');
  const initialChanges = await page.evaluate(() =>
    window.workspaceSummaryFixture.capture()
  );

  await page.evaluate(() => window.workspaceSummaryFixture.refetch());
  expect(
    await page.evaluate(() => window.workspaceSummaryFixture.compare())
  ).toEqual({
    activeList: true,
    archivedList: true,
    activeRow: true,
    archivedRow: true,
    listChanges: initialChanges,
  });

  await page.evaluate(async () => {
    window.workspaceSummaryFixture.change('host-a', false, 7);
    await window.workspaceSummaryFixture.refetch();
  });
  await expect(page.getByTestId('files')).toHaveText('7');
  expect(
    await page.evaluate(() => window.workspaceSummaryFixture.compare())
  ).toEqual({
    activeList: false,
    archivedList: true,
    activeRow: false,
    archivedRow: true,
    listChanges: initialChanges + 1,
  });
  expect(
    await page.evaluate(() => window.workspaceSummaryFixture.requests)
  ).toEqual([
    { hostId: 'host-a', archived: false },
    { hostId: 'host-a', archived: true },
    { hostId: 'host-a', archived: false },
    { hostId: 'host-a', archived: true },
    { hostId: 'host-a', archived: false },
    { hostId: 'host-a', archived: true },
  ]);
});

test('the same workspace IDs on another Host use that Host summary cache and transport', async ({
  page,
}) => {
  await page.goto('/workspaces.html');
  await expect(page.getByTestId('state')).toHaveText('loading');
  await page.evaluate(() => window.workspaceSummaryFixture.initialize());
  await expect(page.getByTestId('state')).toHaveText('ready');
  await page.evaluate(() =>
    window.workspaceSummaryFixture.change('host-b', false, 9)
  );
  await page.getByRole('button', { name: 'Switch Host' }).click();
  await expect(page.getByTestId('state')).toHaveText('loading');
  await page.evaluate(() =>
    window.workspaceSummaryFixture.initialize('host-b')
  );
  await expect(page.getByTestId('state')).toHaveText('ready');
  await expect(page.getByTestId('name')).toHaveText('host-b:workspace-active');
  await expect(page.getByTestId('files')).toHaveText('9');
  expect(
    (await page.evaluate(() => window.workspaceSummaryFixture.requests)).slice(
      -2
    )
  ).toEqual([
    { hostId: 'host-b', archived: false },
    { hostId: 'host-b', archived: true },
  ]);
});
