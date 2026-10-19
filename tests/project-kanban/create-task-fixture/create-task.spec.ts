import { expect, test, type Page } from '@playwright/test';

async function events(page: Page) {
  return page.evaluate(() =>
    JSON.parse(document.documentElement.dataset.events ?? '[]')
  );
}

test.beforeEach(async ({ page }) => {
  await page.route('**/__fixture/task', async (route) => {
    await route.fulfill({
      json: {
        ...route.request().postDataJSON(),
        id: 'persisted-task',
        simple_id: 'VK-1',
        task_number: 1,
        creator_user_id: null,
      },
    });
  });
});

for (const legacy of [false, true]) {
  for (const shortcut of [false, true]) {
    test(`${legacy ? 'legacy draft' : 'new task'} opens details via ${shortcut ? 'Ctrl+Enter' : 'button'}, not session creation`, async ({
      page,
    }) => {
      await page.goto(legacy ? '/?legacy' : '/');
      const title = page.getByRole('textbox', {
        name: 'Task title',
        exact: true,
      });
      await expect(title).toHaveValue(legacy ? 'Restored task' : '');
      await expect(page.getByRole('switch')).toHaveCount(0);
      await title.fill('Choose execution later');
      if (shortcut) await title.press('Control+Enter');
      else
        await page
          .getByRole('button', { name: 'Create Task', exact: true })
          .click();

      await expect(page).toHaveURL(
        /\/projects\/project-1\/tasks\/persisted-task$/
      );
      await expect(page.getByTestId('composer-open')).toHaveText('false');
      const details = page.getByRole('complementary', { name: 'Task details' });
      await expect(
        details.getByRole('heading', { name: 'Choose execution later' })
      ).toBeVisible();
      for (const name of ['Single agent', 'Workflow', 'Arena']) {
        await expect(
          details.getByRole('button', { name, exact: true })
        ).toBeEnabled();
      }
      expect(await events(page)).toEqual([
        {
          event: 'insert',
          value: expect.objectContaining({
            title: 'Choose execution later',
            status_id: 'discuss',
            description: legacy ? 'Restored description' : null,
          }),
        },
        { event: 'expect-task', value: 'persisted-task' },
        {
          event: 'task-details',
          value: { projectId: 'project-1', taskId: 'persisted-task' },
        },
      ]);
      for (const [name, event] of [
        ['Single agent', 'choose-agent'],
        ['Workflow', 'choose-workflow'],
        ['Arena', 'choose-arena'],
      ]) {
        await details.getByRole('button', { name, exact: true }).click();
        expect((await events(page)).at(-1)).toEqual({ event });
      }
    });
  }
}

test('failed persistence keeps the form and draft; retry opens details', async ({
  page,
}) => {
  await page.route('**/__fixture/task', (route) =>
    route.fulfill({ status: 500 })
  );
  await page.goto('/?legacy');
  await page.getByRole('button', { name: 'Create Task', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText(
    'Task persistence failed'
  );
  await expect(page.getByTestId('composer-open')).toHaveText('true');
  await expect(
    page.getByRole('textbox', { name: 'Task title', exact: true })
  ).toHaveValue('Restored task');
  expect(
    (await events(page)).map((entry: { event: string }) => entry.event)
  ).toEqual(['insert']);

  await page.unroute('**/__fixture/task');
  await page.route('**/__fixture/task', (route) =>
    route.fulfill({
      json: {
        ...route.request().postDataJSON(),
        id: 'persisted-task',
        simple_id: 'VK-1',
      },
    })
  );
  await page.getByRole('button', { name: 'Create Task', exact: true }).click();
  await expect(
    page.getByRole('complementary', { name: 'Task details' })
  ).toBeVisible();
  await expect(page.getByTestId('composer-open')).toHaveText('false');
});

test('waits for persistence before closing the composer or opening details', async ({
  page,
}) => {
  let release!: () => void;
  const persisted = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route('**/__fixture/task', async (route) => {
    await persisted;
    await route.fulfill({
      json: {
        ...route.request().postDataJSON(),
        id: 'persisted-task',
        simple_id: 'VK-1',
      },
    });
  });
  await page.goto('/');
  await page
    .getByRole('textbox', { name: 'Task title', exact: true })
    .fill('Pending task');
  const create = page.getByRole('button', { name: 'Create Task', exact: true });
  await create.click();
  await expect(create).toBeDisabled();
  await expect(page.getByTestId('composer-open')).toHaveText('true');
  expect(
    (await events(page)).map((entry: { event: string }) => entry.event)
  ).toEqual(['insert']);
  release();
  await expect(page).toHaveURL(/\/projects\/project-1\/tasks\/persisted-task$/);
  await expect(
    page.getByRole('complementary', { name: 'Task details' })
  ).toBeVisible();
});
