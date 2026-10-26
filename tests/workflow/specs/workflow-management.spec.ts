import { expect, test, type Page, type Route } from '@playwright/test';
import type {
  ExecutorConfig,
  PrepareWorkflowMainSessionRequest,
  WorkflowRunResponse,
  WorkflowTemplateResponse,
} from '../../../shared/types';

const timestamp = '2026-10-04T01:00:00Z';
const mainConfig = {
  executor: 'CODEX',
  variant: 'DEFAULT',
  model_id: 'gpt-publication-test',
  reasoning_id: 'high',
} satisfies ExecutorConfig;
const graph = {
  version: 2,
  nodes: [
    {
      id: 'start',
      type: 'start',
      data: { display_name: 'Start' },
      position: { x: 40, y: 140 },
    },
    {
      id: 'agent',
      type: 'agent',
      data: {
        display_name: 'Prepare report',
        executor_config: mainConfig,
        prompt_template: 'Prepare a report.',
      },
      position: { x: 340, y: 140 },
    },
    {
      id: 'end',
      type: 'end',
      data: { display_name: 'End' },
      position: { x: 640, y: 140 },
    },
  ],
  edges: [
    {
      id: 'start-agent',
      source: 'start',
      source_handle: 'output',
      target: 'agent',
      target_handle: 'input',
      type: 'default',
    },
    {
      id: 'agent-end',
      source: 'agent',
      source_handle: 'output',
      target: 'end',
      target_handle: 'input',
      type: 'default',
    },
  ],
};
const template = {
  id: 'report-template',
  source: 'project',
  project_id: 'project-one',
  name: 'Report workflow',
  description: 'Prepare and review a report',
  graph_json: JSON.stringify(graph),
  revision: 11,
  external_enabled: false,
  main_agent_config: mainConfig,
  main_agent_prompt: 'Discuss requirements before submitting work.',
  created_at: timestamp,
  updated_at: timestamp,
};
const instance = {
  id: 'report-instance',
  project_id: 'project-one',
  task_id: 'report-issue',
  workflow_id: template.id,
  template_id: template.id,
  latest_run_id: null,
  workspace_id: 'prepared-workspace',
  name: 'Report instance',
  status: 'draft',
  main_session_id: null,
  main_session_bound_at: null,
  definition_locked_at: null,
  created_at: timestamp,
  updated_at: timestamp,
};
const capturedContext = {
  project_id: 'project-one',
  main_session_id: 'main-session',
  workflow_id: template.id,
  workflow_name: template.name,
  main_agent_config: mainConfig,
  main_agent_prompt: template.main_agent_prompt,
  prepared_issue_id: null,
  issue_id: null,
  instance_id: null,
  workspace_id: 'prepared-workspace',
  latest_run_id: null,
  definition_locked_at: null,
  allowed_actions: ['start'],
};
const preparedSession = {
  session: {
    id: 'main-session',
    workspace_id: 'prepared-workspace',
    name: 'Report discussion',
    executor: 'CODEX',
    agent_working_dir: null,
    created_at: timestamp,
    updated_at: timestamp,
  },
  context: capturedContext,
};

interface ApiCall {
  method: string;
  path: string;
  body: Record<string, unknown> | null;
}

interface MockOptions {
  mainAgentConfigured?: boolean;
  list?: (route: Route) => Promise<void>;
  workflows?: unknown[];
  projectAttempts?: (route: Route, projectId: string) => Promise<void>;
  prepare?: (route: Route, call: ApiCall) => Promise<void>;
  update?: (route: Route, call: ApiCall) => Promise<void>;
  notifications?: (route: Route, url: URL) => Promise<void>;
  context?: (route: Route, sessionId: string) => Promise<void>;
  attempt?: (route: Route) => Promise<void>;
  run?: (route: Route, call: ApiCall) => Promise<void>;
}

async function json(route: Route, data: unknown, status = 200) {
  await route.fulfill({ status, json: data });
}

function success(data: unknown) {
  return { success: true, data, error_data: null, message: null };
}

async function mockManagement(page: Page, options: MockOptions = {}) {
  const calls: ApiCall[] = [];
  let currentTemplate: WorkflowTemplateResponse = {
    ...template,
    source: 'project',
    main_agent_config:
      options.mainAgentConfigured === false
        ? null
        : { ...mainConfig, executor: 'CODEX' },
    main_agent_prompt:
      options.mainAgentConfigured === false ? null : template.main_agent_prompt,
  };

  // Model discovery is also a read-only backend boundary. Exercise the real
  // controls without needing a local CLI or a second browser-side store.
  await page.routeWebSocket(
    '**/api/agents/discovered-options/ws?*',
    (socket) => {
      socket.send(
        JSON.stringify({
          JsonPatch: [
            {
              op: 'replace',
              path: '/options',
              value: {
                model_selector: {
                  providers: [],
                  models: [],
                  default_model: 'gpt-discovery-test',
                  agents: [],
                  permissions: [],
                },
                slash_commands: [],
                skills: [],
                skill_errors: [],
                loading_models: false,
                loading_agents: false,
                loading_slash_commands: false,
                loading_skills: false,
                error: null,
              },
            },
          ],
        })
      );
      socket.send(JSON.stringify({ Ready: true }));
    }
  );

  await page.route('**/api/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const call: ApiCall = {
      method: request.method(),
      path: url.pathname,
      body: request.postData() ? request.postDataJSON() : null,
    };
    calls.push(call);

    if (url.pathname === '/api/local/v1/fallback/workflows') {
      if (options.list) await options.list(route);
      else
        await json(route, {
          workflows: options.workflows ?? [currentTemplate],
        });
    } else if (
      /\/projects\/[^/]+\/workflows$/.test(url.pathname) &&
      call.method === 'GET'
    ) {
      const projectId = url.pathname.split('/').at(-2);
      await json(route, {
        workflows: [{ ...currentTemplate, project_id: projectId }],
      });
    } else if (
      /\/projects\/[^/]+\/workflows$/.test(url.pathname) &&
      call.method === 'POST'
    ) {
      currentTemplate = {
        ...currentTemplate,
        id: 'created-template',
        project_id: url.pathname.split('/').at(-2)!,
        name: String(call.body?.name),
      };
      await json(route, { data: currentTemplate, txid: 1 });
    } else if (/\/projects\/[^/]+\/workflow-attempts$/.test(url.pathname)) {
      const projectId = url.pathname.split('/').at(-2)!;
      if (options.projectAttempts) {
        await options.projectAttempts(route, projectId);
      } else {
        await json(route, { attempts: [] });
      }
    } else if (/\/projects\/[^/]+\/scheduled-tasks$/.test(url.pathname)) {
      await json(route, { tasks: [] });
    } else if (
      url.pathname === '/api/local/v1/workflows/report-template' &&
      call.method === 'GET'
    ) {
      await json(route, currentTemplate);
    } else if (
      url.pathname === '/api/local/v1/workflows/report-template/attempt'
    ) {
      if (options.attempt) await options.attempt(route);
      else await json(route, null);
    } else if (
      url.pathname === '/api/local/v1/workflow-attempts/report-instance/run' &&
      options.run
    ) {
      await options.run(route, call);
    } else if (
      url.pathname ===
      '/api/local/v1/projects/project-one/tasks/report-issue/workflow-attempts'
    ) {
      currentTemplate = {
        ...currentTemplate,
        name: String(call.body?.name),
        graph_json: String(call.body?.graph_json),
      };
      await json(route, { data: instance, txid: 1 });
    } else if (url.pathname === '/api/local/v1/workflow-runs/report-run') {
      await json(route, {
        id: 'report-run',
        orchestration_run_id: null,
        workflow_id: template.id,
        attempt_id: instance.id,
        task_id: instance.task_id,
        workspace_id: instance.workspace_id,
        trigger_source: 'manual',
        input_text: 'Prepare a report.',
        output_text: null,
        status: 'succeeded',
        started_at: timestamp,
        finished_at: timestamp,
        error_text: null,
        created_at: timestamp,
        updated_at: timestamp,
        nodes: [],
        split_batches: [],
        handoffs: [],
        delivery: null,
        request_path: null,
      } satisfies WorkflowRunResponse);
    } else if (url.pathname === '/api/workspaces/prepared-workspace') {
      await json(
        route,
        success({
          id: 'prepared-workspace',
          container_ref: '/fixtures/report-workspace',
          workspace_kind: 'direct_folder',
          container_ownership: 'external',
          branch: 'direct-folder',
          setup_completed_at: null,
          created_at: timestamp,
          updated_at: timestamp,
          archived: false,
          pinned: false,
          name: 'Report workspace',
          worktree_deleted: false,
        })
      );
    } else if (
      url.pathname === '/api/sessions' &&
      url.searchParams.get('workspace_id') === 'prepared-workspace'
    ) {
      await json(route, success([preparedSession.session]));
    } else if (url.pathname === '/api/workspaces/prepared-workspace/repos') {
      await json(route, success([]));
    } else if (url.pathname === '/api/agents/garage') {
      await json(
        route,
        success(
          ['CODEX', 'CLAUDE_CODE'].map((executor) => ({
            executor,
            policy: {
              disabled: false,
              readiness: 'READY',
              capabilities: ['INITIAL_RUN', 'FOLLOW_UP', 'MCP'],
              diagnostics: [],
            },
          }))
        )
      );
    } else if (url.pathname === '/api/agents/preset-options') {
      await json(route, success(mainConfig));
    } else if (
      url.pathname === '/api/local/v1/workflows/report-template' &&
      call.method === 'PUT'
    ) {
      if (options.update) {
        await options.update(route, call);
      } else {
        currentTemplate = {
          ...currentTemplate,
          ...(call.body?.main_agent_config != null
            ? {
                main_agent_config: call.body
                  .main_agent_config as typeof mainConfig,
              }
            : {}),
          ...(call.body?.main_agent_prompt != null
            ? { main_agent_prompt: call.body.main_agent_prompt as string }
            : {}),
          ...(call.body?.graph_json != null
            ? { graph_json: call.body.graph_json as string }
            : {}),
          ...(call.body?.name != null
            ? { name: call.body.name as string }
            : {}),
          ...(call.body?.description != null
            ? { description: call.body.description as string }
            : {}),
          revision: currentTemplate.revision + 1,
        };
        await json(route, { data: currentTemplate, txid: 1 });
      }
    } else if (
      url.pathname === '/api/workflow-management/prepare-main-session'
    ) {
      // The production Rust request denies unknown fields. Keep that boundary
      // here so UI Task terminology cannot silently change the wire contract.
      const allowedFields = new Set<string>([
        'project_id',
        'workflow_id',
        'request_id',
        'issue_id',
      ] satisfies Array<keyof PrepareWorkflowMainSessionRequest>);
      if (
        !call.body ||
        Object.keys(call.body).some((key) => !allowedFields.has(key))
      ) {
        await json(
          route,
          { success: false, message: 'Unknown main-session request field' },
          422
        );
        return;
      }
      if (options.prepare) await options.prepare(route, call);
      else await json(route, success(preparedSession));
    } else if (
      /\/api\/workflow-management\/sessions\/[^/]+\/context$/.test(url.pathname)
    ) {
      const sessionId = url.pathname.split('/').at(-2)!;
      if (options.context) await options.context(route, sessionId);
      else
        await json(
          route,
          success(sessionId === 'main-session' ? capturedContext : null)
        );
    } else if (
      /\/api\/workflow-management\/sessions\/[^/]+\/notifications$/.test(
        url.pathname
      )
    ) {
      if (options.notifications) await options.notifications(route, url);
      else await json(route, success({ notifications: [], next_cursor: null }));
    } else {
      // Fail closed for any accidental submit/start/fallback request.
      await json(
        route,
        {
          success: false,
          message: `Unexpected fixture API: ${call.method} ${call.path}`,
        },
        500
      );
    }
  });

  return { calls };
}

function mutations(calls: ApiCall[]) {
  return calls.filter((call) => call.method !== 'GET');
}

async function openMainSettings(page: Page) {
  await expect(
    page.getByRole('heading', { name: 'Report workflow' })
  ).toBeVisible();
  await page
    .getByRole('button', { name: 'Actions for Report workflow', exact: true })
    .click();
  await page.getByRole('button', { name: 'Main Agent', exact: true }).click();
  await expect(page.getByLabel('Main Agent instructions')).toBeVisible();
}

test('shows workflows and owning projects immediately without a project chooser or execution', async ({
  page,
}) => {
  const { calls } = await mockManagement(page);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/?mode=management');
  await expect(
    page.getByRole('heading', { name: 'Report workflow' })
  ).toBeVisible();
  await expect(page.getByText('Project · Report project')).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Project' })).toHaveCount(0);
  await expect(page.getByTestId('project-pages')).toHaveText('0');
  await openMainSettings(page);
  await expect(page.getByLabel('Main Agent instructions')).toHaveValue(
    template.main_agent_prompt
  );
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByLabel('Main Agent instructions')).toHaveCount(0);
  expect(
    calls.some((call) => call.path === '/api/local/v1/fallback/workflows')
  ).toBe(true);
  expect(mutations(calls)).toEqual([]);
  expect(errors).toEqual([]);
});

test('project discovery failure preserves workflows and retries their project metadata', async ({
  page,
}) => {
  const { calls } = await mockManagement(page);
  await page.goto('/?mode=management&projectsError=1');
  await expect(
    page.getByRole('heading', { name: 'Report workflow' })
  ).toBeVisible();
  await expect(page.getByText('Could not load projects')).toBeVisible();
  await page.getByRole('button', { name: 'Retry' }).click();
  await expect(page.getByText('Project · Report project')).toBeVisible();
  await expect(page.getByTestId('project-retries')).toHaveText('1');
  expect(mutations(calls)).toEqual([]);
});

test('task workflows appear without reusable definitions and open their existing run and task', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    workflows: [],
    projectAttempts: async (route, projectId) =>
      json(route, {
        attempts:
          projectId === 'project-one'
            ? [
                {
                  ...instance,
                  workflow_id: 'task-backing-workflow',
                  template_id: null,
                  status: 'succeeded',
                  latest_run_id: 'completed-report-run',
                  definition_locked_at: timestamp,
                },
              ]
            : [],
      }),
  });
  await page.goto('/?mode=management');
  await expect(
    page.getByRole('heading', { name: 'Report instance' })
  ).toBeVisible();
  await expect(page.getByText('Project · Report project')).toBeVisible();
  await expect(page.getByText('Task workflow', { exact: true })).toBeVisible();
  await expect(page.getByText('Succeeded', { exact: true })).toBeVisible();
  await expect(page.getByText('No workflows yet')).toHaveCount(0);
  await page
    .getByRole('button', { name: 'Actions for Report instance', exact: true })
    .click();
  await expect(page.getByRole('switch')).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Main Agent', exact: true })
  ).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Scheduled task', exact: true })
  ).toHaveCount(0);
  await page.getByRole('button', { name: 'View task', exact: true }).click();
  await expect(page.getByTestId('opened-task')).toHaveText(
    JSON.stringify({ projectId: 'project-one', taskId: 'report-issue' })
  );
  await expect(page.getByTestId('opened-run')).toBeEmpty();
  await page.getByRole('heading', { name: 'Report instance' }).click();
  await expect(page.getByTestId('opened-run')).toHaveText(
    'completed-report-run'
  );
  expect(mutations(calls)).toEqual([]);
});

test('discovers task-only workflows on later project pages and opens saved drafts by their backing workflow id', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    workflows: [],
    projectAttempts: async (route, projectId) =>
      json(route, {
        attempts:
          projectId === 'project-two'
            ? [
                {
                  ...instance,
                  project_id: projectId,
                  workflow_id: 'research-draft-workflow',
                  template_id: null,
                },
              ]
            : [],
      }),
  });
  await page.goto('/?mode=management&lateProject=1');
  await expect(
    page.getByRole('heading', { name: 'Report instance' })
  ).toBeVisible();
  await expect(page.getByText('Project · Research project')).toBeVisible();
  await expect(page.getByText('Draft', { exact: true })).toBeVisible();
  await expect(page.getByTestId('project-pages')).toHaveText('1');
  const card = page.getByRole('button').filter({
    has: page.getByRole('heading', { name: 'Report instance' }),
  });
  await card.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('opened-workflow')).toHaveText(
    JSON.stringify({
      projectId: 'project-two',
      workflowId: 'research-draft-workflow',
    })
  );
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(mutations(calls)).toEqual([]);
});

test('waits for task workflow discovery before displaying an empty list', async ({
  page,
}) => {
  let release: (() => void) | undefined;
  const delayed = new Promise<void>((resolve) => {
    release = resolve;
  });
  const { calls } = await mockManagement(page, {
    workflows: [],
    projectAttempts: async (route) => {
      await delayed;
      await json(route, { attempts: [] });
    },
  });
  await page.goto('/?mode=management');
  await expect
    .poll(() => calls.some((call) => call.path.endsWith('/workflow-attempts')))
    .toBe(true);
  await expect(page.getByText('No workflows yet')).toHaveCount(0);
  release!();
  await expect(page.getByText('No workflows yet')).toBeVisible();
  expect(mutations(calls)).toEqual([]);
});

test('task workflow discovery errors retain definitions and retry the failed project', async ({
  page,
}) => {
  let failed = true;
  const { calls } = await mockManagement(page, {
    projectAttempts: async (route, projectId) => {
      if (projectId !== 'project-one') {
        await json(route, { attempts: [] });
      } else if (failed) {
        await json(
          route,
          { message: 'Project workflow list unavailable' },
          503
        );
      } else {
        await json(route, {
          attempts: [{ ...instance, workflow_id: 'task-backing-workflow' }],
        });
      }
    },
  });
  await page.goto('/?mode=management');
  await expect(
    page.getByRole('heading', { name: 'Report workflow' })
  ).toBeVisible();
  await expect(
    page.getByText(/Project workflow list unavailable/)
  ).toBeVisible();
  failed = false;
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect(
    page.getByRole('heading', { name: 'Report instance' })
  ).toBeVisible();
  await expect(
    page.getByRole('heading', { name: 'Report workflow' })
  ).toBeVisible();
  await expect(page.getByText(/Project workflow list unavailable/)).toHaveCount(
    0
  );
  expect(mutations(calls)).toEqual([]);
});

test('opens each workflow with its owning project without another selection', async ({
  page,
}) => {
  const researchTemplate = {
    ...template,
    id: 'research-template',
    project_id: 'project-two',
    name: 'Research workflow',
  };
  const { calls } = await mockManagement(page, {
    workflows: [template, researchTemplate],
  });
  await page.goto('/?mode=management');
  await expect(page.getByText('Project · Report project')).toBeVisible();
  await expect(page.getByText('Project · Research project')).toBeVisible();
  await page.getByRole('heading', { name: 'Research workflow' }).click();
  await expect(page.getByTestId('opened-workflow')).toHaveText(
    JSON.stringify({
      projectId: 'project-two',
      workflowId: 'research-template',
    })
  );
  await page.getByRole('heading', { name: 'Report workflow' }).click();
  await expect(page.getByTestId('opened-workflow')).toHaveText(
    JSON.stringify({ projectId: 'project-one', workflowId: 'report-template' })
  );
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(mutations(calls)).toEqual([]);
  // Schedules are fetched once per project, never once per card.
  expect(
    calls
      .filter((call) => call.path.endsWith('/scheduled-tasks'))
      .map((call) => call.path)
      .sort()
  ).toEqual([
    '/api/local/v1/projects/project-one/scheduled-tasks',
    '/api/local/v1/projects/project-two/scheduled-tasks',
  ]);
});

test('workflow metadata discovers later project pages automatically', async ({
  page,
}) => {
  await mockManagement(page, {
    workflows: [{ ...template, project_id: 'project-two' }],
  });
  await page.goto('/?mode=management&lateProject=1');
  await expect(page.getByText('Project · Research project')).toBeVisible();
  await expect(page.getByTestId('project-pages')).toHaveText('1');
  await expect(page.getByRole('combobox')).toHaveCount(0);
});

test('Main Agent preparation uses the workflow owner without a separate project selection', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    workflows: [
      { ...template, id: 'research-template', project_id: 'project-two' },
    ],
    prepare: async (route) =>
      json(
        route,
        success({
          ...preparedSession,
          context: {
            ...capturedContext,
            project_id: 'project-two',
            workflow_id: 'research-template',
          },
        })
      ),
  });
  await page.goto('/?mode=management');
  await openMainSettings(page);
  await page.getByRole('button', { name: 'Open main conversation' }).click();
  await expect(page.getByTestId('prepared-workspace')).toBeVisible();
  expect(mutations(calls)).toMatchObject([
    {
      method: 'POST',
      path: '/api/workflow-management/prepare-main-session',
      body: { project_id: 'project-two', workflow_id: 'research-template' },
    },
  ]);
});

test('workflow management shows project workflows without built-in templates or source badges', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    workflows: [
      template,
      {
        ...template,
        id: 'system-template',
        project_id: null,
        source: 'system',
        name: 'Built-in workflow',
      },
    ],
  });
  await page.goto('/?mode=management');
  await expect(
    page.getByRole('heading', { name: 'Report workflow' })
  ).toBeVisible();
  await expect(page.getByText('Project · Report project')).toBeVisible();
  await expect(
    page.getByRole('heading', { name: 'Built-in workflow' })
  ).toHaveCount(0);
  await expect(page.getByText('Built-in', { exact: true })).toHaveCount(0);
  await expect(page.getByText('Project workflow', { exact: true })).toHaveCount(
    0
  );
  await expect(page.getByText('Available to all projects')).toHaveCount(0);
  await expect(page.getByRole('combobox')).toHaveCount(0);
  await page
    .getByRole('button', { name: 'Actions for Report workflow', exact: true })
    .click();
  await expect(
    page.getByRole('button', { name: 'Main Agent', exact: true })
  ).toHaveCount(1);
  await expect(
    page.getByRole('button', { name: 'Scheduled task', exact: true })
  ).toHaveCount(1);
  await expect(page.getByRole('switch')).toHaveCount(1);
  await page.keyboard.press('Escape');
  await page.getByRole('heading', { name: 'Report workflow' }).click();
  await expect(page.getByTestId('opened-workflow')).toHaveText(
    JSON.stringify({ projectId: 'project-one', workflowId: 'report-template' })
  );
  expect(mutations(calls)).toEqual([]);
});

test('only built-in templates leaves workflow management empty with a create action', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    workflows: [
      {
        ...template,
        id: 'system-template',
        project_id: null,
        source: 'system',
        name: 'Built-in workflow',
      },
    ],
  });
  await page.goto('/?mode=management');
  await expect(page.getByText('No workflows yet')).toBeVisible();
  await expect(
    page.getByRole('heading', { name: 'Built-in workflow' })
  ).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Main Agent', exact: true })
  ).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Scheduled task', exact: true })
  ).toHaveCount(0);
  await expect(page.getByRole('switch')).toHaveCount(0);
  await page
    .getByRole('button', { name: 'Create workflow', exact: true })
    .click();
  await expect(page.getByRole('combobox', { name: 'Project' })).toBeVisible();
  expect(
    calls.filter((call) => call.path.endsWith('/scheduled-tasks'))
  ).toEqual([]);
  expect(mutations(calls)).toEqual([]);
});

test('creating a global workflow chooses its owner once and refreshes global discovery', async ({
  page,
}) => {
  const { calls } = await mockManagement(page);
  await page.goto('/?mode=management');
  await page.getByRole('button', { name: 'New workflow', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Create workflow', exact: true })
  ).toBeDisabled();
  await page.getByRole('combobox', { name: 'Project' }).click();
  await page
    .getByRole('option', { name: 'Research project', exact: true })
    .click();
  await page
    .getByRole('button', { name: 'Create workflow', exact: true })
    .click();
  await expect(page.getByTestId('opened-workflow')).toHaveText(
    JSON.stringify({
      projectId: 'project-two',
      workflowId: 'created-template',
    })
  );
  await expect(
    page.getByRole('heading', { name: 'New workflow', exact: true })
  ).toBeVisible();
  await expect(page.getByText('Project · Research project')).toBeVisible();
  expect(mutations(calls)).toMatchObject([
    { method: 'POST', path: '/api/local/v1/projects/project-two/workflows' },
  ]);
  expect(
    calls.filter((call) => call.path === '/api/local/v1/fallback/workflows')
      .length
  ).toBeGreaterThan(1);
});

test('global list loading failure retries and a successful empty list retains creation', async ({
  page,
}) => {
  let failed = true;
  await mockManagement(page, {
    list: async (route) => {
      if (failed) await json(route, { message: 'List offline' }, 503);
      else await json(route, { workflows: [] });
    },
  });
  await page.goto('/?mode=management');
  await expect(
    page.getByText(/Failed to load workflows: List offline/)
  ).toBeVisible();
  failed = false;
  await page.getByRole('button', { name: 'Retry' }).click();
  await expect(page.getByText('No workflows yet')).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Create workflow', exact: true })
  ).toBeVisible();
});

test('failed creation keeps its selected project and exposes a retry in the dialog', async ({
  page,
}) => {
  await mockManagement(page);
  let failures = 0;
  await page.route(
    '**/api/local/v1/projects/project-two/workflows',
    async (route) => {
      if (route.request().method() === 'POST' && failures++ === 0) {
        await json(route, { message: 'Could not create workflow' }, 503);
      } else await route.fallback();
    }
  );
  await page.goto('/?mode=management');
  await page.getByRole('button', { name: 'New workflow', exact: true }).click();
  await page.getByRole('combobox', { name: 'Project' }).click();
  await page
    .getByRole('option', { name: 'Research project', exact: true })
    .click();
  await page
    .getByRole('button', { name: 'Create workflow', exact: true })
    .click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByRole('alert')).toContainText(
    'Could not create workflow'
  );
  await expect(page.getByRole('combobox', { name: 'Project' })).toContainText(
    'Research project'
  );
  await page
    .getByRole('button', { name: 'Create workflow', exact: true })
    .click();
  await expect(page.getByTestId('opened-workflow')).toHaveText(
    JSON.stringify({
      projectId: 'project-two',
      workflowId: 'created-template',
    })
  );
});

test('publishes full provider/model/reasoning and prompt with the current revision, without graph changes', async ({
  page,
}) => {
  const { calls } = await mockManagement(page);
  await page.goto('/?mode=management');
  await openMainSettings(page);
  await expect(
    page.getByRole('button', { name: 'Save', exact: true })
  ).toBeDisabled();
  await page
    .getByLabel('Main Agent instructions')
    .fill('Discuss materials, then submit the approved report.');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Save', exact: true })
  ).toBeDisabled();
  expect(mutations(calls)).toEqual([
    {
      method: 'PUT',
      path: '/api/local/v1/workflows/report-template',
      body: {
        expected_revision: 11,
        name: null,
        description: null,
        graph_json: null,
        main_agent_config: mainConfig,
        main_agent_prompt:
          'Discuss materials, then submit the approved report.',
      },
    },
  ]);

  await page.getByLabel('Main Agent instructions').fill('Second publication.');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Save', exact: true })
  ).toBeDisabled();
  expect(mutations(calls).at(-1)?.body?.expected_revision).toBe(12);
});

test('save conflict keeps the draft and prevents preparation or automatic execution', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    update: async (route) =>
      json(
        route,
        {
          error_data: {
            workflow_id: template.id,
            expected_revision: 11,
            current_revision: 12,
          },
        },
        409
      ),
  });
  await page.goto('/?mode=management');
  await openMainSettings(page);
  await page
    .getByLabel('Main Agent instructions')
    .fill('My unsaved requirements');
  await page.getByRole('button', { name: 'Open main conversation' }).click();
  await expect(page.getByRole('alert')).toContainText(
    'Workflow changed from revision 11 to 12'
  );
  await expect(page.getByLabel('Main Agent instructions')).toHaveValue(
    'My unsaved requirements'
  );
  expect(mutations(calls).map((call) => call.path)).toEqual([
    '/api/local/v1/workflows/report-template',
  ]);
});

test('opening a conversation saves first and navigates to the returned exact Session without submission', async ({
  page,
}) => {
  const { calls } = await mockManagement(page);
  await page.goto('/?mode=management');
  await openMainSettings(page);
  await page
    .getByLabel('Main Agent instructions')
    .fill('Updated for new Sessions only.');
  await page.getByRole('button', { name: 'Open main conversation' }).click();
  await expect(page.getByTestId('prepared-workspace')).toBeVisible();
  await expect(page.getByTestId('management-location')).toHaveText(
    JSON.stringify({
      pathname: '/workspaces/prepared-workspace',
      search: { session_id: 'main-session' },
    })
  );
  expect(mutations(calls).map((call) => call.path)).toEqual([
    '/api/local/v1/workflows/report-template',
    '/api/workflow-management/prepare-main-session',
  ]);
  expect(mutations(calls)[1]?.body).toEqual({
    project_id: 'project-one',
    workflow_id: template.id,
    request_id: expect.any(String),
  });
});

test('preparation retry keeps request identity and explicit Task, including remote exact-session navigation', async ({
  page,
}) => {
  let attempts = 0;
  const { calls } = await mockManagement(page, {
    prepare: async (route) => {
      attempts += 1;
      if (attempts === 1)
        await json(
          route,
          { success: false, message: 'Temporary preparation error' },
          503
        );
      else await json(route, success(preparedSession));
    },
  });
  await page.goto('/?mode=management-button&remote=1&task=report-issue');
  await page.getByRole('button', { name: 'Open main conversation' }).click();
  await expect(page.getByRole('alert')).toContainText(
    'Temporary preparation error'
  );
  await page.getByRole('button', { name: 'Open main conversation' }).click();
  await expect(page.getByTestId('management-location')).toHaveText(
    JSON.stringify({
      pathname: '/hosts/remote-host/workspaces/prepared-workspace',
      search: { session_id: 'main-session' },
    })
  );
  const prepares = mutations(calls);
  expect(prepares).toHaveLength(2);
  expect(prepares[0].body).toEqual(prepares[1].body);
  expect(prepares[0].body).toEqual({
    project_id: 'project-one',
    workflow_id: template.id,
    issue_id: 'report-issue',
    request_id: expect.any(String),
  });
  expect(prepares.map((call) => call.path)).toEqual([
    '/api/workflow-management/prepare-main-session',
    '/api/workflow-management/prepare-main-session',
  ]);
});

for (const { locale, buttonName, errorMessage } of [
  {
    locale: 'en-US',
    buttonName: 'Open main conversation',
    errorMessage:
      'This workflow has no main Agent configured, so its main Session cannot be opened. Configure the main Agent before starting the workflow.',
  },
  {
    locale: 'zh-CN',
    buttonName: '打开主会话',
    errorMessage:
      '本次工作流未配置主智能体，无法打开主会话。请在工作流开始前配置主智能体。',
  },
]) {
  test.describe(`missing captured main Agent (${locale})`, () => {
    test.use({ locale });

    test('explains the structured configuration conflict without changing the instance', async ({
      page,
    }) => {
      const { calls } = await mockManagement(page, {
        prepare: async (route) =>
          json(
            route,
            {
              success: false,
              data: null,
              error_data: {
                code: 'workflow_conflict',
                message:
                  "Configure this workflow's captured main Agent before opening its Session",
              },
              message: null,
            },
            409
          ),
      });
      await page.goto('/?mode=management-button&task=report-issue');
      await page.getByRole('button', { name: buttonName }).click();
      await expect(page.getByRole('alert')).toHaveText(errorMessage);
      await page.getByRole('button', { name: buttonName }).click();
      await expect(page.getByRole('alert')).toHaveText(errorMessage);
      await expect(page.getByTestId('prepared-workspace')).toHaveCount(0);
      const prepares = mutations(calls);
      expect(prepares.map((call) => call.path)).toEqual([
        '/api/workflow-management/prepare-main-session',
        '/api/workflow-management/prepare-main-session',
      ]);
      expect(prepares[0].body).toEqual(prepares[1].body);
      expect(prepares[0].body).toEqual({
        project_id: 'project-one',
        workflow_id: template.id,
        issue_id: 'report-issue',
        request_id: expect.any(String),
      });
    });
  });
}

test('a deleted original main Session is not silently replaced by another create path', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    prepare: async (route) =>
      json(
        route,
        {
          success: false,
          data: null,
          error_data: {
            code: 'instance_binding_conflict',
            message:
              'The original main Session was deleted and cannot be replaced.',
          },
          message: null,
        },
        409
      ),
  });
  await page.goto('/?mode=management-button&task=report-issue');
  await page.getByRole('button', { name: 'Open main conversation' }).click();
  await expect(page.getByRole('alert')).toContainText('cannot be replaced');
  await page.getByRole('button', { name: 'Open main conversation' }).click();
  await expect(page.getByRole('alert')).toContainText('cannot be replaced');
  await expect(page.getByTestId('prepared-workspace')).toHaveCount(0);
  expect(mutations(calls).map((call) => call.path)).toEqual([
    '/api/workflow-management/prepare-main-session',
    '/api/workflow-management/prepare-main-session',
  ]);
  expect(mutations(calls)[0].body).toEqual(mutations(calls)[1].body);
});

test('an unavailable execution Host disables preparation without any requests', async ({
  page,
}) => {
  const { calls } = await mockManagement(page);
  await page.goto('/?mode=management-button&blocked=1');
  await expect(
    page.getByRole('button', { name: 'Open main conversation' })
  ).toBeDisabled();
  expect(calls).toEqual([]);
});

async function openEditorMainSettings(page: Page) {
  await page
    .getByRole('button', { name: 'Configure main Agent', exact: true })
    .click();
  await expect(page.getByLabel('Main Agent instructions')).toBeVisible();
}

test('an unconfigured main Agent has a direct editor entry and can be saved before opening a conversation', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    mainAgentConfigured: false,
    attempt: async (route) => json(route, instance),
  });
  await page.goto('/?mode=management-editor');
  await expect(page.getByText('Not configured', { exact: true })).toBeVisible();
  await expect(page.getByText('Optional.', { exact: false })).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Open main conversation' })
  ).toBeDisabled();
  // A main Agent is optional for runtime-controlled execution.
  await expect(
    page.getByRole('button', { name: 'Run workflow attempt' })
  ).toBeEnabled();
  await openEditorMainSettings(page);
  const dialog = page.locator('.vk-keyboard-dialog');
  await dialog.getByRole('button', { name: 'Select executor' }).click();
  await page.getByRole('menuitem', { name: 'Codex', exact: true }).click();
  await dialog.getByLabel('Main Agent instructions').fill('Manage the report.');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(
    dialog.getByRole('button', { name: 'Save', exact: true })
  ).toBeDisabled();
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByText('Not configured', { exact: true })).toHaveCount(
    0
  );
  await expect(
    page.getByRole('button', { name: 'Open main conversation' })
  ).toBeEnabled();
  expect(mutations(calls).map((call) => call.path)).toEqual([
    '/api/local/v1/workflows/report-template',
  ]);
  expect(mutations(calls)[0].body).toMatchObject({
    main_agent_config: { executor: 'CODEX' },
    main_agent_prompt: 'Manage the report.',
    graph_json: null,
  });
});

test('a new local draft saves its canvas and opens main Agent configuration without starting work', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    mainAgentConfigured: false,
    attempt: async (route) => json(route, instance),
  });
  await page.addInitScript(
    (draft) => {
      sessionStorage.setItem(
        `vibe.workflowAttemptDraft.${draft.id}`,
        JSON.stringify(draft)
      );
    },
    {
      id: 'local-report',
      projectId: 'project-one',
      taskId: 'report-issue',
      taskTitle: 'Prepare report',
      name: 'Draft report workflow',
      graphJson: JSON.stringify(graph),
      repos: [],
      directoryPath: '/fixtures/report-workspace',
      createdAt: timestamp,
    }
  );
  await page.goto('/?mode=management-editor&workflow=draft-local-report');
  await expect(page.getByLabel('Workflow name')).toHaveValue(
    'Draft report workflow'
  );
  await page.getByLabel('Workflow name').fill('My prepared workflow');
  await expect(
    page.getByText('Save and configure', { exact: true })
  ).toBeVisible();
  await openEditorMainSettings(page);
  expect(mutations(calls)).toMatchObject([
    {
      method: 'POST',
      path: '/api/local/v1/projects/project-one/tasks/report-issue/workflow-attempts',
      body: {
        name: 'My prepared workflow',
        directory_path: '/fixtures/report-workspace',
      },
    },
  ]);
  const savedGraph = JSON.parse(String(mutations(calls)[0].body?.graph_json));
  expect(savedGraph.nodes).toHaveLength(graph.nodes.length);
  await expect(page.getByTestId('opened-workflow')).toContainText(
    'report-template'
  );
  expect(
    await page.evaluate(() =>
      sessionStorage.getItem('vibe.workflowAttemptDraft.local-report')
    )
  ).toBeNull();
});

test('an accepted workflow without a main Agent explains the unavailable conversation without allowing edits', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    mainAgentConfigured: false,
    attempt: async (route) =>
      json(route, { ...instance, definition_locked_at: timestamp }),
  });
  await page.goto('/?mode=management-editor');
  await expect(
    page.getByText('No main Agent was configured when this workflow started.', {
      exact: false,
    })
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Open main conversation' })
  ).toBeDisabled();
  await expect(
    page.getByRole('button', { name: 'Configure main Agent', exact: true })
  ).toHaveCount(0);
  expect(mutations(calls)).toEqual([]);
});

test('a run with no captured main Agent disables preparation and retains its canvas', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    mainAgentConfigured: false,
    attempt: async (route) =>
      json(route, { ...instance, definition_locked_at: timestamp }),
  });
  await page.goto('/?mode=management-run');
  await expect(
    page.getByText('No main Agent was configured when this workflow started.', {
      exact: false,
    })
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Open main conversation' })
  ).toBeDisabled();
  await expect(page.locator('.react-flow')).toBeAttached();
  await expect(page.getByRole('alert')).toHaveCount(0);
  expect(mutations(calls)).toEqual([]);
});

test('an original main Session stays reopenable even when main Agent configuration is absent', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    mainAgentConfigured: false,
    attempt: async (route) =>
      json(route, {
        ...instance,
        main_session_id: 'main-session',
        main_session_bound_at: timestamp,
        definition_locked_at: timestamp,
      }),
  });
  await page.goto('/?mode=management-run');
  await expect(
    page.getByRole('button', { name: 'Open main conversation' })
  ).toBeEnabled();
  await expect(
    page.getByText('No main Agent was configured when this workflow started.', {
      exact: false,
    })
  ).toHaveCount(0);
  expect(mutations(calls)).toEqual([]);
});

test('captured configuration lookup failure has a local retry and enables opening after recovery', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    attempt: async (route) => json(route, instance),
  });
  let failed = true;
  await page.route(
    '**/api/local/v1/workflows/report-template',
    async (route) => {
      if (failed)
        await json(route, { message: 'Configuration unavailable' }, 503);
      else await route.fallback();
    }
  );
  await page.goto('/?mode=management-run');
  await expect(
    page.getByText('Could not load the captured main Agent configuration.', {
      exact: false,
    })
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Open main conversation' })
  ).toBeDisabled();
  failed = false;
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Open main conversation' })
  ).toBeEnabled();
  await expect(page.locator('.react-flow')).toBeAttached();
  expect(mutations(calls)).toEqual([]);
});

test('the production instance dialog prepares the existing Task rather than an unrelated discussion', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    attempt: async (route) => json(route, instance),
  });
  await page.goto('/?mode=management-editor');
  await expect(page.getByLabel('Workflow name')).toHaveValue(template.name);
  await expect(
    page.getByRole('button', { name: 'Add Node', exact: true })
  ).toBeEnabled();
  await openEditorMainSettings(page);
  // Two explicit entries point to the same Issue; choose the dialog entry.
  await page
    .locator('.vk-keyboard-dialog')
    .getByRole('button', { name: 'Open main conversation' })
    .click();
  await expect(page.getByTestId('prepared-workspace')).toBeVisible();
  expect(mutations(calls)).toHaveLength(1);
  expect(mutations(calls)[0]).toEqual({
    method: 'POST',
    path: '/api/workflow-management/prepare-main-session',
    body: {
      issue_id: 'report-issue',
      project_id: 'project-one',
      workflow_id: template.id,
      request_id: expect.any(String),
    },
  });
  await expect(page.getByTestId('management-location')).toContainText(
    '"session_id":"main-session"'
  );
});

test('the instance configuration dialog opens the original publication Session', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    attempt: async (route) =>
      json(route, { ...instance, template_id: 'original-publication' }),
  });
  await page.goto('/?mode=management-editor');
  await openEditorMainSettings(page);
  await page
    .locator('.vk-keyboard-dialog')
    .getByRole('button', { name: 'Open main conversation' })
    .click();
  await expect(page.getByTestId('prepared-workspace')).toBeVisible();
  expect(mutations(calls)).toMatchObject([
    {
      path: '/api/workflow-management/prepare-main-session',
      body: {
        issue_id: instance.task_id,
        workflow_id: 'original-publication',
      },
    },
  ]);
});

test('publishing Main Agent configuration retains unsaved canvas work and advances its save revision', async ({
  page,
}) => {
  const { calls } = await mockManagement(page);
  await page.goto('/?mode=management-editor');
  await expect(
    page.getByRole('button', { name: 'Add Node', exact: true })
  ).toBeEnabled();
  await page
    .locator('.react-flow__node[data-id="agent"]')
    .dblclick({ force: true });
  await page
    .getByLabel('Prompt template', { exact: true })
    .fill('Unsaved report Node requirement.');
  await page.getByRole('button', { name: 'Close configuration' }).click();
  await page.getByLabel('Workflow name').fill('My unsaved workflow title');
  await openEditorMainSettings(page);
  await page
    .getByLabel('Main Agent instructions')
    .fill('Published instructions must not overwrite canvas work.');
  const dialog = page.locator('.vk-keyboard-dialog');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(
    dialog.getByRole('button', { name: 'Save', exact: true })
  ).toBeDisabled();
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();

  await expect(page.getByLabel('Workflow name')).toHaveValue(
    'My unsaved workflow title'
  );
  await expect(
    page.getByRole('button', { name: 'Undo', exact: true })
  ).toBeEnabled();
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect.poll(() => mutations(calls).length).toBe(2);
  const [publication, canvasSave] = mutations(calls);
  expect(publication.body).toMatchObject({
    expected_revision: 11,
    graph_json: null,
  });
  expect(canvasSave.body).toMatchObject({
    expected_revision: 12,
    name: 'My unsaved workflow title',
  });
  const savedGraph = JSON.parse(
    String(canvasSave.body?.graph_json)
  ) as typeof graph;
  expect(
    savedGraph.nodes.find((node) => node.id === 'agent')?.data.prompt_template
  ).toBe('Unsaved report Node requirement.');
  expect(savedGraph.edges.map((edge) => edge.id)).toEqual(
    graph.edges.map((edge) => edge.id)
  );
  expect(
    mutations(calls).every(
      (call) => call.path === '/api/local/v1/workflows/report-template'
    )
  ).toBe(true);
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('an unresolved instance lookup never exposes editable definitions and stays locked after terminal success', async ({
  page,
}) => {
  let release: (() => void) | undefined;
  const delayed = new Promise<void>((resolve) => {
    release = resolve;
  });
  const { calls } = await mockManagement(page, {
    attempt: async (route) => {
      await delayed;
      await json(route, {
        ...instance,
        status: 'succeeded',
        definition_locked_at: timestamp,
      });
    },
  });
  await page.goto('/?mode=management-editor');
  const name = page.getByLabel('Workflow name');
  await expect(name).toHaveValue(template.name);
  await expect(name).toHaveAttribute('readonly', '');
  await expect(
    page.getByRole('button', { name: 'Add Node', exact: true })
  ).toBeDisabled();
  await expect(
    page.getByRole('button', { name: 'Save', exact: true })
  ).toBeDisabled();
  await expect(
    page.getByRole('button', { name: 'Configure main Agent', exact: true })
  ).toBeDisabled();

  release!();
  await expect(
    page.getByText('Its workflow is permanently read-only', { exact: false })
  ).toBeVisible();
  await expect(name).toHaveAttribute('readonly', '');
  await expect(
    page.getByRole('button', { name: 'Add Node', exact: true })
  ).toBeDisabled();
  await expect(
    page.getByRole('button', { name: 'Save', exact: true })
  ).toBeDisabled();
  expect(mutations(calls)).toEqual([]);
});

test('instance discovery errors fail closed without enabling publication, rename, or graph mutation', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    attempt: async (route) =>
      json(route, { message: 'Instance unavailable' }, 503),
  });
  await page.goto('/?mode=management-editor');
  await expect(page.getByRole('alert')).toContainText(
    'Could not verify the instance'
  );
  await expect(page.getByLabel('Workflow name')).toHaveAttribute(
    'readonly',
    ''
  );
  await expect(
    page.getByRole('button', { name: 'Add Node', exact: true })
  ).toBeDisabled();
  await expect(
    page.getByRole('button', { name: 'Save', exact: true })
  ).toBeDisabled();
  await expect(
    page.getByRole('button', { name: 'Configure main Agent', exact: true })
  ).toBeDisabled();
  expect(mutations(calls)).toEqual([]);
});

test('a locked terminal instance keeps Session/results controls but never persists its graph during execution actions', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    attempt: async (route) =>
      json(route, {
        ...instance,
        status: 'failed',
        latest_run_id: 'failed-report-run',
        main_session_id: 'main-session',
        main_session_bound_at: timestamp,
        definition_locked_at: timestamp,
      }),
    run: async (route) =>
      json(
        route,
        { message: 'Use an explicit resume for this accepted instance.' },
        409
      ),
  });
  await page.goto('/?mode=management-editor');
  await expect(
    page.getByText('Its workflow is permanently read-only', { exact: false })
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Open main conversation' })
  ).toBeEnabled();
  await page.getByRole('button', { name: 'More actions' }).click();
  await expect(
    page.getByRole('menuitem', { name: 'Main Agent', exact: true })
  ).toHaveCount(0);
  await page.getByRole('menuitem', { name: 'Open latest run' }).click();
  await expect(page.getByTestId('opened-run')).toHaveText('failed-report-run');
  expect(mutations(calls)).toEqual([]);

  await page
    .locator('.react-flow__node[data-id="agent"]')
    .dblclick({ force: true });
  await expect(
    page.getByLabel('Prompt template', { exact: true })
  ).toBeDisabled();
  await page.getByRole('button', { name: 'Close configuration' }).click();
  const run = page.getByRole('button', { name: 'Run workflow attempt' });
  await expect(run).toBeEnabled();
  await run.click();
  await expect(page.getByRole('alert')).toContainText('explicit resume');
  expect(mutations(calls).map((call) => call.path)).toEqual([
    '/api/local/v1/workflow-attempts/report-instance/run',
  ]);
});

test('a deleted bound main Session disables instance preparation rather than permitting a replacement', async ({
  page,
}) => {
  const { calls } = await mockManagement(page, {
    attempt: async (route) =>
      json(route, {
        ...instance,
        status: 'cancelled',
        main_session_bound_at: timestamp,
        definition_locked_at: timestamp,
      }),
  });
  await page.goto('/?mode=management-editor');
  await expect(
    page.getByRole('button', { name: 'Open main conversation' })
  ).toBeDisabled();
  expect(mutations(calls)).toEqual([]);
});

const humanWait = {
  id: 'human-wait',
  sequence: 1,
  instance_id: 'report-instance',
  run_id: 'report-run',
  main_session_id: 'main-session',
  event_key: 'node-review:waiting',
  kind: 'human_wait',
  node_execution_id: 'review-execution',
  interaction_id: 'review-execution',
  observed_status: 'waiting_human',
  summary: 'Review requires a decision.',
  created_at: timestamp,
  current_status: 'waiting_human',
  is_resolved: false,
};
const completed = {
  ...humanWait,
  id: 'completed-run',
  sequence: 2,
  event_key: 'report-run:succeeded',
  kind: 'terminal',
  node_execution_id: null,
  interaction_id: null,
  observed_status: 'succeeded',
  current_status: 'succeeded',
  summary: 'Report completed.',
  is_resolved: true,
};

test('refreshes historical waits across pages in place, deduplicates replay, and retains messages on errors', async ({
  page,
}) => {
  let resolved = false;
  let failed = false;
  const { calls } = await mockManagement(page, {
    notifications: async (route, url) => {
      if (failed)
        return json(
          route,
          { success: false, message: 'Temporary notification error' },
          503
        );
      const wait = resolved
        ? { ...humanWait, current_status: 'succeeded', is_resolved: true }
        : humanWait;
      await json(
        route,
        success(
          url.searchParams.has('cursor')
            ? { notifications: [wait, completed], next_cursor: null }
            : { notifications: [wait], next_cursor: 1 }
        )
      );
    },
  });
  await page.goto('/?mode=management-notifications');
  const rows = page.getByTestId('workflow-system-messages').locator('li');
  await expect(rows).toHaveCount(2);
  const waitRow = page.locator(
    '[data-message-key="workflow-notification:human-wait"]'
  );
  await expect(waitRow).toContainText('waiting for a response');
  await expect(page.getByTestId('main-session-context')).toContainText(
    'gpt-publication-test'
  );
  const firstRow = await waitRow.elementHandle();

  resolved = true;
  await page.getByRole('button', { name: 'Refresh notifications' }).click();
  await expect(waitRow).toContainText('has been resolved');
  await expect(rows).toHaveCount(2);
  expect(await firstRow!.evaluate((element) => element.isConnected)).toBe(true);
  const notificationReads = calls.filter((call) =>
    call.path.endsWith('/notifications')
  );
  expect(notificationReads).toHaveLength(4);

  failed = true;
  await page.getByRole('button', { name: 'Refresh notifications' }).click();
  await expect(page.getByRole('alert')).toContainText('Refresh failed');
  await expect(rows).toHaveCount(2);
  await expect(waitRow).toContainText('has been resolved');
  expect(mutations(calls)).toEqual([]);
});

test('Session switching cannot display late notifications from the previous Session or launch an Agent', async ({
  page,
}) => {
  let release: (() => void) | undefined;
  const delayed = new Promise<void>((resolve) => {
    release = resolve;
  });
  const { calls } = await mockManagement(page, {
    notifications: async (route) => {
      await delayed;
      await json(
        route,
        success({ notifications: [humanWait], next_cursor: null })
      ).catch(() => undefined);
    },
  });
  await page.goto('/?mode=management-notifications');
  await expect
    .poll(
      () => calls.filter((call) => call.path.endsWith('/notifications')).length
    )
    .toBe(1);
  await page.getByRole('button', { name: 'Open another Session' }).click();
  await expect
    .poll(() =>
      calls.some((call) => call.path.includes('/other-session/context'))
    )
    .toBe(true);
  release!();
  await expect(
    page.getByTestId('workflow-system-messages').locator('li')
  ).toHaveCount(0);
  await expect(page.getByTestId('main-session-context')).toHaveText('null');
  expect(
    calls.some((call) => call.path.includes('/other-session/notifications'))
  ).toBe(false);
  expect(mutations(calls)).toEqual([]);
});
