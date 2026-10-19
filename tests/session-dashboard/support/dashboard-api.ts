import type { Page } from "@playwright/test";
import type {
  DashboardChildRow,
  DashboardPageResponse,
  DashboardQuery,
  DashboardWorkRow,
  DashboardOpenTarget,
} from "../../../shared/types";

const timestamp = "2026-10-19T01:00:00Z";
export function sessionTarget(
  session = "analysis-session",
  workflow = false,
): Extract<DashboardOpenTarget, { kind: "session" }> {
  return {
    kind: "session",
    workspace_id: "visual-workspace",
    session_id: session,
    agent_run_id: `agent-${session}`,
    workflow_run_id: workflow ? "visual-run" : null,
    node_execution_id: workflow
      ? `${session.replace("-session", "")}-execution`
      : null,
    workflow_node_id: workflow ? "build" : null,
  };
}

export function workRow(
  kind: DashboardWorkRow["work"]["kind"],
  title: string,
): DashboardWorkRow {
  const target = sessionTarget(
    kind === "task" ? "research-session" : "analysis-session",
  );
  return {
    work: { kind, id: `${kind}-work` },
    title,
    project: { id: "project-1", title: "Studio" },
    task:
      kind === "session" || kind === "unresolved"
        ? null
        : { id: "task-1", title: "Launch plan" },
    execution_id: kind === "task" ? "execution-1" : null,
    workflow_run_id: kind === "workflow" ? "visual-run" : null,
    raw_status: kind === "workflow" ? "awaiting_human" : "running",
    status_group: kind === "workflow" ? "waiting" : "running",
    directory: {
      path: "C:/work/studio",
      kind: "direct_folder",
      git_branch: "main",
    },
    agents: [
      {
        provider_id: "codex",
        runtime_profile_id: "CODEX:DEFAULT",
        model_id: "fixture-model",
        reasoning_id: "high",
        configuration_source: "observed",
      },
    ],
    active_session_count: 1,
    active_run_count: 1,
    matching_session_count: 1,
    waiting_count: kind === "workflow" ? 1 : 0,
    child_count: kind === "workflow" || kind === "arena" ? 4 : 0,
    preview: {
      text: `Latest reply for ${title}`,
      truncated: false,
      occurred_at: timestamp,
      message_id: "reply-1",
      session_id: target.session_id,
      workspace_id: target.workspace_id,
      agent_run_id: target.agent_run_id!,
      run_attempt_id: "attempt-1",
      event_id: "event-1",
      source_role: "session",
      source_title: "Analysis Agent",
      caught_up: true,
      open_target: target,
    },
    preview_status: "ready",
    activity: {
      kind: "tool",
      description: "inspect_project",
      status: "succeeded",
      occurred_at: timestamp,
    },
    open_target:
      kind === "workflow"
        ? {
            kind: "workflow",
            attempt_id: "workflow-work",
            workflow_id: "template-1",
            workflow_run_id: "visual-run",
            project_id: "project-1",
          }
        : kind === "arena"
          ? {
              kind: "arena",
              arena_group_id: "arena-1",
              project_id: "project-1",
            }
          : target,
    updated_at: timestamp,
    issues: [],
    stale_membership: false,
    invalidated: false,
  };
}

export function childrenFor(row: DashboardWorkRow): DashboardChildRow[] {
  return ["main_agent", "node", "subtask", "arena_candidate"].map(
    (role, index) => {
      const target = sessionTarget(
        [
          "main-session",
          "analysis-session",
          "research-session",
          "review-session",
        ][index],
        true,
      );
      if (role === "main_agent") {
        target.node_execution_id = null;
        target.workflow_node_id = null;
      }
      return {
        id: `child-${role}`,
        parent: row.work,
        role,
        title: ["Coordinator", "Implementation", "Research", "Candidate A"][
          index
        ],
        workflow_run_id: row.workflow_run_id,
        node_execution_id: target.node_execution_id,
        subtask_id: role === "subtask" ? "split-work-1" : null,
        arena_candidate_id: role === "arena_candidate" ? "candidate-1" : null,
        session_id: target.session_id,
        agent_run_id: target.agent_run_id,
        raw_status: role === "node" ? "awaiting_human" : "succeeded",
        agent_run_status: "succeeded",
        status_group: role === "node" ? "waiting" : "finished",
        agent: row.agents[0],
        directory: row.directory,
        preview: null,
        preview_status: "missing",
        activity: null,
        open_target: target,
        reused: false,
        matches_filter: role === "node",
        issues: [],
        updated_at: timestamp,
      };
    },
  );
}

export function dashboardResponse(
  rows = [
    workRow("task", "Ship release"),
    workRow("workflow", "Launch workflow"),
    workRow("session", "Explore idea"),
    workRow("arena", "Compare solutions"),
    workRow("unresolved", "Check binding"),
  ],
): DashboardPageResponse {
  return {
    generated_at: timestamp,
    snapshot_id: "snapshot-1",
    snapshot_status: "ready",
    page: 0,
    page_size: 20,
    total: rows.length,
    latest_total: rows.length,
    counts: {
      work_count: rows.length,
      active_session_count: rows.length,
      active_run_count: rows.length,
      waiting: 1,
      running: rows.length - 1,
      failed: 0,
      finished: 0,
      current: rows.length,
      all: rows.length,
    },
    rows,
    expanded: [],
    has_updates: false,
    issues: [],
    projects: [{ id: "project-1", title: "Studio" }],
    providers: ["codex", "claude_code"],
  };
}

export async function mockDashboard(
  page: Page,
  resolve: (
    query: DashboardQuery,
    host: string | null,
  ) => DashboardPageResponse | Promise<DashboardPageResponse> = () =>
    dashboardResponse(),
) {
  const requests: Array<{ query: DashboardQuery; host: string | null }> = [];
  const contexts: string[] = [];
  await page.route("**/api/**/query", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (!path.endsWith("/dashboard/query")) return route.fallback();
    const host = path.match(/\/host\/([^/]+)\//)?.[1] ?? null;
    const query = route.request().postDataJSON() as DashboardQuery;
    requests.push({ query, host });
    const response = await resolve(query, host);
    const expanded = query.expanded.map((entry) => {
      const supplied = response.expanded.find(
        (page) =>
          page.work.kind === entry.work.kind && page.work.id === entry.work.id,
      );
      if (supplied) return supplied;
      const row = response.rows.find((item) => item.work.id === entry.work.id)!;
      return {
        work: entry.work,
        workflow_run_id: row.workflow_run_id,
        snapshot_id: "children-1",
        page: entry.page,
        page_size: 20,
        total: 4,
        rows: childrenFor(row),
        has_updates: false,
        invalidation: null,
      };
    });
    await route
      .fulfill({
        json: {
          success: true,
          data: { ...response, expanded },
          error_data: null,
          message: null,
        },
      })
      .catch(() => {});
  });
  await page.route("**/api/**/context?*", async (route) => {
    const url = new URL(route.request().url());
    contexts.push(url.pathname);
    await route.fulfill({
      json: {
        success: true,
        data: {
          work: { kind: "task", id: "task-work" },
          generated_at: timestamp,
          sessions: [
            {
              session_id: "research-session",
              workspace_id: "visual-workspace",
              agent_run_id: "agent-research-session",
              directory: {
                path: "C:/work/studio",
                kind: "direct_folder",
                git_branch: "main",
              },
              agent: null,
              started_at: timestamp,
              finished_at: null,
            },
          ],
          repositories: [
            {
              path: "C:/work/studio",
              status: "git",
              branch: "main",
              commit: "abc123",
              remotes: [
                {
                  name: "origin",
                  url: "git@github.com:owner/studio.git",
                  web_url: "https://github.com/owner/studio",
                },
              ],
              issue: null,
            },
          ],
          issues: [],
        },
      },
    });
  });
  return { requests, contexts };
}
