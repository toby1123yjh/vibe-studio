import { test, expect } from '@playwright/test';

for (const { name, rawPath, displayPath } of [
  {
    name: 'extended drive',
    rawPath: String.raw`\\?\D:\dev_space\chat`,
    displayPath: String.raw`D:\dev_space\chat`,
  },
  {
    name: 'extended UNC',
    rawPath: String.raw`\\?\unc\server\share\chat`,
    displayPath: String.raw`\\server\share\chat`,
  },
]) {
  test(`${name} paths display normally while inspection, browse and workflow payloads retain the original`, async ({
    page,
  }) => {
    const inspectedPaths: string[] = [];
    const pickerRequests: unknown[] = [];
    const defaultWrites: unknown[] = [];
    const workflowPayloads: unknown[] = [];
    await page.route('**/api/**', async (route) => {
      const request = route.request();
      const url = request.url();
      let data: unknown = {};
      if (url.includes('/repos')) data = [];
      else if (url.includes('/scratch/')) {
        if (request.method() !== 'GET')
          defaultWrites.push(request.postDataJSON());
        data = {
          payload: {
            type: 'PROJECT_REPO_DEFAULTS',
            data: { repos: [], directory_path: rawPath },
          },
        };
      } else if (url.includes('/inspect-directory')) {
        inspectedPaths.push(request.postDataJSON().path);
        data = { path: rawPath, is_git_repo: false, repo: null };
      } else if (url.includes('/pick-folder')) {
        pickerRequests.push(request.postDataJSON());
        data = rawPath;
      } else if (url.includes('/workflow-attempts')) {
        workflowPayloads.push(request.postDataJSON());
        data = { id: 'attempt-a' };
      }
      await route.fulfill({
        json: url.includes('/local/v1/')
          ? { data, txid: 1 }
          : { success: true, data, message: null },
      });
    });
    await page.goto('/');
    await page
      .getByRole('button', { name: 'Choose workflow location' })
      .click();
    const input = page.getByRole('textbox', { name: 'Working directory' });
    await expect(input).toHaveValue(displayPath);
    await expect(input).toHaveAttribute('title', displayPath);
    await expect(
      page.getByRole('radio', { name: 'Direct editing' })
    ).toBeChecked();
    expect(inspectedPaths).toEqual([rawPath]);
    expect(defaultWrites).toEqual([]);
    expect(workflowPayloads).toEqual([]);

    await page.getByRole('button', { name: 'Browse', exact: true }).click();
    await expect(input).toHaveValue(displayPath);
    await expect(
      page.getByRole('radio', { name: 'Direct editing' })
    ).toBeVisible();
    expect(pickerRequests).toEqual([
      { initial_path: rawPath, title: 'Choose working directory' },
    ]);
    expect(inspectedPaths).toEqual([rawPath, rawPath]);
    expect(defaultWrites).toEqual([]);
    await page.getByRole('radio', { name: 'Direct editing' }).check();
    await page
      .getByRole('button', { name: 'Use this directory', exact: true })
      .click();
    await expect(page.locator('output')).toContainText('directory_path');
    expect(defaultWrites).toEqual([
      {
        payload: {
          type: 'PROJECT_REPO_DEFAULTS',
          data: { repos: [], directory_path: rawPath },
        },
      },
    ]);
    expect(workflowPayloads).toHaveLength(2);
    for (const payload of workflowPayloads)
      expect(payload).toMatchObject({ repos: [], directory_path: rawPath });
  });
}

test('editing a friendly path sends the chosen new directory to inspection', async ({
  page,
}) => {
  const rawPath = String.raw`\\?\D:\dev_space\chat`;
  const editedPath = String.raw`D:\dev_space\other`;
  const inspectedPaths: string[] = [];
  await page.route('**/api/**', async (route) => {
    const url = route.request().url();
    let data: unknown = {};
    if (url.includes('/repos')) data = [];
    else if (url.includes('/scratch/'))
      data = {
        payload: {
          type: 'PROJECT_REPO_DEFAULTS',
          data: { repos: [], directory_path: rawPath },
        },
      };
    else if (url.includes('/inspect-directory')) {
      const path = route.request().postDataJSON().path;
      inspectedPaths.push(path);
      data = { path, is_git_repo: false, repo: null };
    }
    await route.fulfill({ json: { success: true, data, message: null } });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Choose workflow location' }).click();
  const input = page.getByRole('textbox', { name: 'Working directory' });
  await expect(input).toHaveValue(String.raw`D:\dev_space\chat`);
  await expect(
    page.getByRole('radio', { name: 'Direct editing' })
  ).toBeChecked();
  await input.fill(editedPath);
  await input.press('Enter');
  await expect(
    page.getByRole('radio', { name: 'Direct editing' })
  ).toBeVisible();
  expect(inspectedPaths).toEqual([rawPath, editedPath]);
  await expect(input).toHaveValue(editedPath);
});

test('Git worktree selection preserves repository identity and base branch with a friendly directory', async ({
  page,
}) => {
  const rawPath = String.raw`\\?\D:\dev_space\repo`;
  const repo = {
    id: 'repo-a',
    path: rawPath,
    name: 'repo',
    display_name: 'Repository',
  };
  const payloads: unknown[] = [];
  const inspections: string[] = [];
  await page.route('**/api/**', async (route) => {
    const url = route.request().url();
    let data: unknown = {};
    if (url.endsWith('/repos')) data = [repo];
    else if (url.includes('/branches'))
      data = [{ name: 'main', is_current: true, is_remote: false }];
    else if (url.endsWith('/repos/repo-a')) data = repo;
    else if (url.includes('/scratch/'))
      data = {
        payload: {
          type: 'PROJECT_REPO_DEFAULTS',
          data: {
            repos: [{ repo_id: 'repo-a', target_branch: 'main' }],
            directory_path: null,
          },
        },
      };
    else if (url.includes('/inspect-directory')) {
      inspections.push(route.request().postDataJSON().path);
      data = { path: rawPath, is_git_repo: true, repo, current_branch: 'main' };
    } else if (url.includes('/workflow-attempts')) {
      payloads.push(route.request().postDataJSON());
      data = { id: 'attempt-a' };
    }
    await route.fulfill({
      json: url.includes('/local/v1/')
        ? { data, txid: 1 }
        : { success: true, data, message: null },
    });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Choose workflow location' }).click();
  await expect(
    page.getByRole('textbox', { name: 'Working directory' })
  ).toHaveValue(String.raw`D:\dev_space\repo`);
  await expect(
    page.getByRole('radio', { name: 'Isolated Worktree' })
  ).toBeChecked();
  await expect(
    page.getByRole('button', { name: 'main', exact: true })
  ).toBeVisible();
  await page
    .getByRole('button', { name: 'Use this workspace', exact: true })
    .click();
  await expect(page.locator('output')).toContainText('repo-a');
  expect(inspections).toEqual([rawPath]);
  expect(payloads).toHaveLength(2);
  for (const payload of payloads) {
    expect(payload).toMatchObject({
      repos: [{ repo_id: 'repo-a', target_branch: 'main' }],
    });
    expect(payload).not.toHaveProperty('directory_path');
  }
});

test('picker confirmation after route unmount cannot save defaults or start a workflow', async ({
  page,
}) => {
  const mutations: string[] = [];
  await page.route('**/api/**', async (route) => {
    const url = route.request().url();
    if (
      (url.includes('/scratch/') && route.request().method() !== 'GET') ||
      url.includes('/workflow-attempts')
    )
      mutations.push(url);
    const data = url.includes('/repos')
      ? []
      : url.includes('/inspect-directory')
        ? { path: 'F:\\notes', is_git_repo: false, repo: null }
        : {
            payload: {
              type: 'PROJECT_REPO_DEFAULTS',
              data: { repos: [], directory_path: 'F:\\notes' },
            },
          };
    await route.fulfill({ json: { success: true, data, message: null } });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Choose workflow location' }).click();
  const confirm = page.getByRole('button', {
    name: 'Use this directory',
    exact: true,
  });
  await expect(confirm).toBeEnabled();
  await page.evaluate(() =>
    window.dispatchEvent(new Event('unmount-workflow-fixture'))
  );
  await expect(page.getByText('Workflow route closed')).toBeVisible();
  await confirm.click();
  await expect(confirm).not.toBeVisible();
  expect(mutations).toEqual([]);
});

test('real workflow chooser prefills a non-Git project directory and carries it into create/run requests', async ({
  page,
}) => {
  const payloads: unknown[] = [];
  let defaultWrites = 0;
  await page.route('**/api/**', async (route) => {
    const url = route.request().url();
    if (url.includes('/scratch/') && route.request().method() !== 'GET')
      defaultWrites++;
    let data: unknown = {};
    if (url.includes('/repos')) data = [];
    else if (url.includes('/scratch/'))
      data = {
        payload: {
          type: 'PROJECT_REPO_DEFAULTS',
          data: { repos: [], directory_path: 'F:\\notes' },
        },
      };
    else if (url.includes('/inspect-directory'))
      data = { path: 'F:\\notes', is_git_repo: false, repo: null };
    else if (url.includes('/workflow-attempts')) {
      payloads.push(route.request().postDataJSON());
      data = { id: 'attempt-a' };
    }
    await route.fulfill({
      json: url.includes('/local/v1/')
        ? { data, txid: 1 }
        : { success: true, data, message: null },
    });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Choose workflow location' }).click();
  await expect(page.locator('input').first()).toHaveValue('F:\\notes');
  await expect(
    page.getByRole('radio', { name: 'Direct editing' })
  ).toBeChecked();
  await expect(
    page.getByRole('radio', { name: 'Isolated Worktree' })
  ).toBeDisabled();
  await expect(page.getByText('F:\\notes', { exact: true })).toHaveCount(0);
  await expect(
    page.getByText('The agent will edit files in this directory.')
  ).toBeVisible();
  const unavailable = page.getByRole('button', {
    name: 'Requires a Git repository',
  });
  await unavailable.focus();
  await expect(page.getByRole('tooltip')).toContainText(
    'Requires a Git repository'
  );
  await page.setViewportSize({ width: 375, height: 812 });
  await expect(
    page.getByRole('button', { name: 'Use this directory', exact: true })
  ).toBeVisible();
  const dialog = await page.getByRole('dialog').boundingBox();
  expect(dialog!.width).toBeLessThanOrEqual(375);
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.locator('output')).toHaveText('canceled');
  expect(payloads).toHaveLength(0);
  expect(defaultWrites).toBe(0);
  await page.getByRole('button', { name: 'Choose workflow location' }).click();
  const confirm = page.getByRole('button', {
    name: 'Use this directory',
    exact: true,
  });
  await expect(confirm).toBeEnabled();
  await confirm.click();
  await expect(page.locator('output')).toContainText('directory_path');
  expect(payloads).toHaveLength(2);
  for (const payload of payloads)
    expect(payload).toMatchObject({ repos: [], directory_path: 'F:\\notes' });
});
