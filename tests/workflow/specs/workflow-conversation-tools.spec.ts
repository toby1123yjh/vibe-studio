import { expect, test, type Page } from '@playwright/test';
import {
  mockWorkflowConversations,
  mockWorkflowFileContext,
} from '../support/conversation-api';

async function mockWorkspaceTools(page: Page) {
  const fileReads: string[] = [];
  const diffStreams: string[] = [];
  const terminalStreams: string[] = [];
  const timestamp = '2026-10-15T00:00:00Z';
  await page.route('**/api/workspaces/visual-workspace/repos', (route) =>
    route.fulfill({
      json: {
        success: true,
        data: [
          {
            id: 'fixture-repo',
            name: 'fixture',
            display_name: 'Fixture',
            path: 'C:/fixture',
            target_branch: 'main',
            setup_script: null,
            cleanup_script: null,
            archive_script: null,
            copy_files: null,
            parallel_setup_script: false,
            dev_server_script: null,
            default_target_branch: null,
            default_working_dir: null,
            created_at: timestamp,
            updated_at: timestamp,
          },
        ],
      },
    })
  );
  await page.route('**/api/workspaces/visual-workspace/files/**', (route) => {
    const url = new URL(route.request().url());
    fileReads.push(url.pathname + url.search);
    const file = {
      workspace_id: 'visual-workspace',
      repo_id: 'fixture-repo',
      repo_name: 'fixture',
      path: 'report.md',
      name: 'report.md',
      kind: 'file',
      size_bytes: 12,
      modified_at: null,
      mime_type: 'text/markdown',
      is_binary: false,
    };
    return route.fulfill({
      json: {
        success: true,
        data: url.pathname.endsWith('/content')
          ? {
              ...file,
              kind: 'text',
              content: '# Delivered\n',
              language: 'markdown',
              raw_url: null,
              truncated: false,
            }
          : {
              repos: [
                {
                  workspace_id: 'visual-workspace',
                  repo_id: 'fixture-repo',
                  repo_name: 'fixture',
                  repo_display_name: 'Fixture',
                  truncated: false,
                  entries: [file],
                },
              ],
            },
      },
    });
  });
  await page.routeWebSocket(
    '**/api/workspaces/visual-workspace/git/diff/ws*',
    (socket) => {
      const url = new URL(socket.url());
      diffStreams.push(url.pathname + url.search);
      const statsOnly = url.searchParams.get('stats_only') === 'true';
      socket.send(
        JSON.stringify({
          JsonPatch: [
            {
              op: 'replace',
              path: '/entries',
              value: {
                fixture: {
                  'report.md': {
                    type: 'DIFF',
                    content: {
                      change: 'added',
                      oldPath: null,
                      newPath: 'report.md',
                      oldContent: null,
                      newContent: statsOnly ? null : '# Delivered\n',
                      contentOmitted: statsOnly,
                      additions: 1,
                      deletions: 0,
                      repoId: 'fixture-repo',
                    },
                  },
                },
              },
            },
          ],
        })
      );
      socket.send(JSON.stringify({ Ready: true }));
    }
  );
  await page.routeWebSocket('**/api/terminal/ws*', (socket) => {
    terminalStreams.push(socket.url());
  });
  return { fileReads, diffStreams, terminalStreams };
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.route('**/api/**', (route) => route.abort());
  await page.route('https://fonts.googleapis.com/**', (route) => route.abort());
  await mockWorkflowFileContext(page);
});

test('running workflow uses the shared tools inside the same window and preserves the composer', async ({
  page,
}, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const api = await mockWorkflowConversations(page, {
    initialRunStatus: 'running',
  });
  const toolsApi = await mockWorkspaceTools(page);
  await page.goto('/visual.html?mode=file-run');
  await page.getByTestId('workflow-run-node-build').click();
  const panel = page.getByTestId('workflow-conversation-panel');
  const editor = panel.locator(
    '[data-chatbox-container] [data-lexical-editor="true"]'
  );
  await expect(editor).toBeVisible();
  await editor.fill('Draft kept while checking the running work');
  const editorHandle = await editor.elementHandle();
  expect(toolsApi.fileReads).toEqual([]);
  expect(
    toolsApi.diffStreams.every((url) => url.includes('stats_only=true'))
  ).toBe(true);

  const more = panel.getByRole('button', { name: 'More actions' });
  await more.click();
  await expect(page.getByRole('menuitem', { name: 'New session' })).toHaveCount(
    0
  );
  await expect(
    page.getByRole('menuitem', { name: 'Start review' })
  ).toHaveCount(0);
  const openTools = page.getByRole('menuitem', {
    name: 'Inspector',
    exact: true,
  });
  // The tools remain available while the Agent is running.
  await expect(openTools).toBeEnabled();
  await openTools.click();
  const tools = panel.getByTestId('conversation-tools');
  await expect(tools).toBeVisible();
  await expect(tools).toHaveAttribute('data-workspace-id', 'visual-workspace');
  await expect(tools.getByRole('tab')).toHaveCount(6);
  await expect(
    tools.getByRole('tab', { name: 'Files', exact: true })
  ).toHaveAttribute('aria-selected', 'true');
  await tools.locator('[data-workspace-file-path="report.md"]').click();
  await expect(
    tools.getByRole('heading', { name: 'Delivered', exact: true })
  ).toBeVisible();
  const contentRead = toolsApi.fileReads.find((url) =>
    url.includes('/content?')
  );
  expect(contentRead).toContain('repo_id=fixture-repo');
  expect(contentRead).toContain('path=report.md');

  await tools.getByRole('button', { name: 'Changes', exact: true }).click();
  await expect(
    tools.getByRole('tab', { name: 'Changes', exact: true })
  ).toHaveAttribute('aria-selected', 'true');
  await expect(tools.locator('[data-diff-path="report.md"]')).toBeVisible();
  expect(
    toolsApi.diffStreams.some((url) => !url.includes('stats_only=true'))
  ).toBe(true);
  await panel.getByRole('separator').press('Home');
  await expect
    .poll(() =>
      tools.evaluate((element) => element.scrollWidth <= element.clientWidth)
    )
    .toBe(true);
  await page.screenshot({
    path: testInfo.outputPath('conversation-tools.png'),
  });
  await tools.getByRole('button', { name: 'Back', exact: true }).click();
  await expect(editor).toBeVisible();
  await expect(editor).toHaveText('Draft kept while checking the running work');
  await expect(more).toBeFocused();
  expect(await editorHandle!.evaluate((element) => element.isConnected)).toBe(
    true
  );
  expect(api.requests.filter((request) => request.startsWith('POST '))).toEqual(
    []
  );
  expect(errors).toEqual([]);
});

test('terminal is bound to the workflow workspace, stays mounted and Escape returns to the chat first', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const api = await mockWorkflowConversations(page);
  const toolsApi = await mockWorkspaceTools(page);
  await page.goto('/visual.html?mode=file-run');
  await page.getByTestId('workflow-run-node-build').click();
  const panel = page.getByTestId('workflow-conversation-panel');
  const editor = panel.locator(
    '[data-chatbox-container] [data-lexical-editor="true"]'
  );
  await expect(editor).toBeVisible();
  await editor.fill('Draft while using the terminal');
  const more = panel.getByRole('button', { name: 'More actions' });
  await more.click();
  await page.getByRole('menuitem', { name: 'Inspector', exact: true }).click();
  const tools = panel.getByTestId('conversation-tools');
  await tools.getByRole('tab', { name: 'Terminal', exact: true }).click();
  await expect(tools.locator('.xterm')).toBeVisible();
  await expect.poll(() => toolsApi.terminalStreams.length).toBe(1);
  expect(
    new URL(toolsApi.terminalStreams[0]).searchParams.get('workspace_id')
  ).toBe('visual-workspace');
  const terminal = await tools.locator('.xterm').elementHandle();
  await tools.getByRole('tab', { name: 'Files', exact: true }).click();
  expect(await terminal!.evaluate((element) => element.isConnected)).toBe(true);
  await page.keyboard.press('Escape');
  await expect(panel).toBeVisible();
  await expect(editor).toBeVisible();
  await expect(editor).toHaveText('Draft while using the terminal');
  await expect(more).toBeFocused();
  expect(await terminal!.evaluate((element) => element.isConnected)).toBe(true);
  await more.click();
  await page.getByRole('menuitem', { name: 'Inspector', exact: true }).click();
  await tools.getByRole('tab', { name: 'Terminal', exact: true }).click();
  await expect(tools.locator('.xterm')).toBeVisible();
  expect(toolsApi.terminalStreams).toHaveLength(1);
  expect(api.requests.filter((request) => request.startsWith('POST '))).toEqual(
    []
  );
  expect(errors).toEqual([]);
});
