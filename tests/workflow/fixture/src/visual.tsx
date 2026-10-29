import { useState } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ReactFlowProvider } from "@xyflow/react";
import { ThemeMode, type WorkflowRunResponse } from "shared/types";
import i18n from "../../../../packages/web-core/src/i18n/config";
import "../../../../packages/web-core/src/app/styles/new/index.css";
import "../../../../packages/ui/src/styles/tokens.css";
import { ThemeProvider } from "@/shared/providers/ThemeProvider";
import { useTheme } from "@/shared/hooks/useTheme";
import { AppNavigationProvider } from "@/shared/hooks/useAppNavigation";
import type { AppNavigation } from "@/shared/lib/routes/appNavigation";
import { workflowTemplateQueryKeys } from "@/shared/hooks/useWorkflowTemplates";
import { WorkflowCanvas } from "@/features/workflow/ui/WorkflowCanvas";
import { WorkflowConfigurationFrame } from "@/features/workflow/ui/WorkflowConfigurationFrame";
import { WorkflowRunCanvasTab } from "@/features/workflow/ui/WorkflowRunCanvasTab";
import { WorkflowRunResult } from "@/features/workflow/ui/WorkflowRunResult";
import {
  createConversationFixtureRouter,
  RouterProvider,
  ScratchScopeFixture,
} from "./conversation-support";
import { WorkspaceCreationHandoffPreview } from "./WorkspaceCreationHandoffPreview";
import {
  getWorkflowNodeActionGate,
  getWorkflowNodeExecutionForWork,
  getWorkflowRuntimeView,
} from "@/features/workflow/model/workflowRuntimeView";
import { buildWorkflowRunDashboardSummary } from "@/features/workflow/model/workflowRunView";
import {
  migrateWorkflowGraph,
  WORKFLOW_GRAPH_VERSION,
  type WorkflowGraph,
} from "@/features/workflow/model/workflowGraph";

void i18n.changeLanguage("en");

const graph: WorkflowGraph = migrateWorkflowGraph({
  version: WORKFLOW_GRAPH_VERSION,
  nodes: [
    {
      id: "start",
      type: "start",
      data: { display_name: "Start" },
      position: { x: 0, y: 180 },
    },
    {
      id: "plan",
      type: "agent",
      data: {
        display_name: "Plan the change",
        prompt_template: "Create an implementation plan.",
        executor_config: { executor: "CODEX" },
      },
      position: { x: 200, y: 100 },
    },
    {
      id: "build",
      type: "agent",
      data: {
        display_name: "Implement the change",
        prompt_template: "Implement the approved plan.",
        executor_config: { executor: "CLAUDE_CODE" },
      },
      position: { x: 560, y: 100 },
    },
    {
      id: "end",
      type: "end",
      data: { display_name: "End" },
      position: { x: 920, y: 180 },
    },
  ],
  edges: [
    { id: "start-plan", source: "start", target: "plan", type: "default" },
    { id: "plan-build", source: "plan", target: "build", type: "default" },
    { id: "build-end", source: "build", target: "end", type: "default" },
  ],
});

const client = new QueryClient({
  defaultOptions: { queries: { retry: false, staleTime: Infinity } },
});
const fileDriven =
  new URLSearchParams(location.search).get("mode") === "file-run";
const fixtureGraph: WorkflowGraph = fileDriven
  ? {
      ...graph,
      nodes: graph.nodes.map((node) =>
        node.id === "plan"
          ? {
              ...node,
              type: "task_splitter",
              data: { ...node.data, display_name: "Split the task" },
            }
          : node
      ),
    }
  : graph;
client.setQueryData(
  workflowTemplateQueryKeys.detail(
    "visual-template",
    new URLSearchParams(location.search).get("host")
  ),
  {
    id: "visual-template",
    graph_json: JSON.stringify(fixtureGraph),
  }
);

// Navigation must never accidentally leave the isolated fixture or run an Agent.
function unexpectedNavigation(): never {
  throw new Error("Visual fixture must not navigate to an execution");
}
const navigation: AppNavigation = {
  resolveFromPath: () => {
    const hostId = new URLSearchParams(location.search).get("host");
    return hostId ? { kind: "workspaces", hostId } : null;
  },
  goToRoot: unexpectedNavigation,
  goToOnboarding: unexpectedNavigation,
  goToOnboardingSignIn: unexpectedNavigation,
  goToWorkspaces: unexpectedNavigation,
  goToWorkspacesCreate: unexpectedNavigation,
  goToWorkspace: unexpectedNavigation,
  goToWorkspaceVsCode: unexpectedNavigation,
  goToExport: unexpectedNavigation,
  goToProject: unexpectedNavigation,
  goToProjectWorkflows: unexpectedNavigation,
  goToProjectWorkflowEdit: unexpectedNavigation,
  goToProjectWorkflowRun: unexpectedNavigation,
  goToProjectTask: unexpectedNavigation,
  goToProjectTaskWorkspace: unexpectedNavigation,
  goToProjectTaskWorkspaceCreate: unexpectedNavigation,
  goToProjectWorkspaceCreate: unexpectedNavigation,
};

function makeRun(status: "running" | "succeeded"): WorkflowRunResponse {
  return {
    id: "visual-run",
    orchestration_run_id: null,
    workflow_id: "visual-template",
    attempt_id: null,
    task_id: "visual-issue",
    workspace_id: "visual-workspace",
    trigger_source: "manual",
    input_text: "Visual fixture",
    output_text: null,
    status,
    started_at: "2026-09-07T00:00:00Z",
    finished_at: null,
    error_text: null,
    created_at: "2026-09-07T00:00:00Z",
    updated_at: "2026-09-07T00:00:01Z",
    nodes: [],
    split_batches: [],
    handoffs: [],
    delivery: null,
    request_path: null,
    runtime_view: {
      run_id: "visual-run",
      status,
      active_node_count: status === "running" ? 1 : 0,
      completed_node_count: status === "running" ? 2 : 4,
      reused_node_count: 0,
      skipped_node_count: 0,
      pending_node_count: status === "running" ? 1 : 0,
      waiting_node_count: 0,
      failed_node_count: 0,
      node_work: fixtureGraph.nodes.map((node) => ({
        node_id: node.id,
        node_type: node.type,
        iteration: 0n,
        status:
          node.id === "build"
            ? status
            : node.id === "end" && status === "running"
              ? "pending"
              : "succeeded",
        pending_work_count: 0,
        starting_child_count: 0,
        active_execution_id:
          node.id === "build" && status === "running"
            ? "visual-execution"
            : null,
        active_session_id: null,
        orchestration_node_execution_id: null,
        active_agent_run_id: null,
        projection_status: "current",
        active_started_at: null,
        active_elapsed_ms: null,
        active_slow: false,
        active_slow_threshold_ms: 300000,
        runtime_health: "ok",
        can_open_session: false,
        can_retry: false,
        can_approve: false,
        can_reject: false,
        can_select_arena_winner: false,
        can_select_condition_branch: false,
        can_cancel_node: false,
        reused_results: [],
      })),
    },
  };
}

function makeReusedRun(): WorkflowRunResponse {
  const run = makeRun("succeeded");
  run.nodes = [
    {
      id: "fresh-build-execution",
      run_id: run.id,
      execution_id: "fresh-build-task",
      node_id: "build",
      node_type: "agent",
      iteration: 0n,
      status: "succeeded",
      input_text: "Use the earlier plan",
      output_text: "Fresh build result",
      session_id: "fresh-build-session",
      orchestration_node_execution_id: "fresh-build-orchestration",
      agent_run_id: "fresh-build-agent-run",
      projection_status: "current",
      execution_process_id: null,
      arena_group_id: null,
      tokens_used: 42n,
      cost_estimate: 0.01,
      started_at: "2026-09-07T00:00:00Z",
      finished_at: "2026-09-07T00:00:01Z",
      error_text: null,
      created_at: "2026-09-07T00:00:00Z",
      updated_at: "2026-09-07T00:00:01Z",
    },
  ];
  const view = run.runtime_view!;
  view.completed_node_count = 1;
  view.reused_node_count = 2;
  view.skipped_node_count = 1;
  view.node_work = view.node_work.map((work) => {
    if (work.node_id === "start" || work.node_id === "plan") {
      return {
        ...work,
        status: "reused",
        iteration: 1n,
        // Deliberately inconsistent action flags verify frontend fail-closed
        // guards rather than hiding controls only because a fixture says false.
        can_open_session: true,
        can_retry: true,
        can_approve: true,
        can_reject: true,
        can_select_arena_winner: true,
        can_select_condition_branch: true,
        can_cancel_node: true,
        reused_results: [
          {
            node_id: work.node_id,
            iteration: 0,
            source_node_execution_id: `${work.node_id}-source-0`,
            source_run_id: "earlier-run",
            output_text:
              work.node_id === "plan" ? "Earlier plan, first iteration" : null,
          },
          ...(work.node_id === "plan"
            ? [
                {
                  node_id: work.node_id,
                  iteration: 1,
                  source_node_execution_id: "plan-source-1",
                  source_run_id: "earlier-run",
                  output_text: "Earlier plan, second iteration",
                },
              ]
            : []),
        ],
      };
    }
    if (work.node_id === "end") return { ...work, status: "skipped" };
    return {
      ...work,
      orchestration_node_execution_id: "fresh-build-orchestration",
      active_agent_run_id: "fresh-build-agent-run",
    };
  });
  return run;
}

function makeFileDrivenRun(): WorkflowRunResponse {
  const run = makeRun("running");
  run.attempt_id = "visual-instance";
  run.request_path = ".vibe-kanban/workflows/visual-run/request.md";
  run.split_batches = [
    {
      id: "visual-batch",
      run_id: run.id,
      node_execution_id: "split-execution",
      splitter_node_id: "plan",
      summary_node_id: "end",
      manifest_path: ".vibe-kanban/workflows/visual-run/tasks.json",
      manifest_sha256: "a".repeat(64),
      dispatch_path: ".vibe-kanban/workflows/visual-run/dispatch.json",
      state: "running",
      error_text: null,
      source_batch_id: null,
      summary_node_execution_id: null,
      handover: {
        state: "completed",
        session_id: "main-session",
        agent_run_id: "main-run",
        run_attempt_id: "main-attempt",
        error_text: null,
      },
      subtasks: [
        {
          id: "research",
          subtask_id: "research",
          title: "Research the requirements",
          instruction_path:
            ".vibe-kanban/workflows/visual-run/instructions/research.md",
          node_id: "build",
          stage_index: 0,
          state: "succeeded",
          depends_on: [],
          node_execution_id: "research-execution",
          session_id: "research-session",
          agent_run_id: "research-run",
          error_text: null,
          handoff_path:
            ".vibe-kanban/workflows/visual-run/research/handoff.json",
        },
        {
          id: "review",
          subtask_id: "review",
          title: "Review the source material",
          instruction_path:
            ".vibe-kanban/workflows/visual-run/instructions/review.md",
          node_id: "build",
          stage_index: 0,
          state: "failed",
          depends_on: [],
          node_execution_id: "review-execution",
          session_id: "review-session",
          agent_run_id: "review-run",
          error_text: "Source material was removed",
          handoff_path: null,
        },
        {
          id: "independent",
          subtask_id: "independent",
          title: "Prepare independent analysis",
          instruction_path:
            ".vibe-kanban/workflows/visual-run/instructions/analysis.md",
          node_id: "build",
          stage_index: 0,
          state: "running",
          depends_on: [],
          node_execution_id: "analysis-execution",
          session_id: "analysis-session",
          agent_run_id: "analysis-run",
          error_text: null,
          handoff_path: null,
        },
        {
          id: "followup",
          subtask_id: "followup",
          title: "Use the review result",
          instruction_path:
            ".vibe-kanban/workflows/visual-run/instructions/followup.md",
          node_id: "build",
          stage_index: 1,
          state: "blocked",
          depends_on: ["review"],
          node_execution_id: null,
          session_id: null,
          agent_run_id: null,
          error_text: "Review dependency failed",
          handoff_path: null,
        },
      ],
    },
  ];
  run.handoffs = [
    {
      node_execution_id: "research-execution",
      run_id: run.id,
      handoff_path: ".vibe-kanban/workflows/visual-run/research/handoff.json",
      files: [
        { path: "deliverables/research report.md", sha256: "b".repeat(64) },
      ],
    },
  ];
  // Match real runtime bindings: internal work shares a Node, not a Session.
  const baseExecution = makeReusedRun().nodes[0];
  run.nodes = [
    {
      ...baseExecution,
      id: "split-execution",
      run_id: run.id,
      node_id: "plan",
      node_type: "task_splitter",
      session_id: null,
    },
    ...run.split_batches[0].subtasks
      .filter((task) => task.node_execution_id && task.session_id)
      .map((task) => ({
        ...baseExecution,
        id: task.node_execution_id!,
        run_id: run.id,
        node_id: task.node_id!,
        session_id: task.session_id,
        status:
          task.state === "failed"
            ? "failed"
            : task.state === "succeeded"
              ? "succeeded"
              : "running",
      })),
  ];
  const view = run.runtime_view!;
  view.node_work = view.node_work.map((work) => ({
    ...work,
    orchestration_node_execution_id: `orchestration-${work.node_id}`,
    active_agent_run_id: work.node_id === "build" ? "analysis-run" : null,
  }));
  if (new URLSearchParams(location.search).get("clone") === "1") {
    run.split_batches[0].source_batch_id = "original-batch";
    run.nodes = run.nodes.filter(
      (execution) => execution.id !== "research-execution"
    );
  }
  return run;
}

function makePartialRun(): WorkflowRunResponse {
  const run = makeFileDrivenRun();
  run.id = "partial-run";
  run.status = "succeeded";
  run.split_batches = [];
  run.output_text =
    "Available research summarized. The review remains incomplete.";
  run.delivery = {
    completeness: "partial",
    source_run_id: "visual-run",
    source_batch_id: "visual-batch",
    missing_subtasks: [
      {
        subtask_id: "review",
        title: "Review the source material",
        state: "failed",
        reason: "Source material was removed",
      },
    ],
  };
  run.runtime_view = {
    ...run.runtime_view!,
    run_id: run.id,
    status: "succeeded",
  };
  return run;
}

const runtime =
  fileDriven || new URLSearchParams(location.search).get("mode") === "run";

function Fixture() {
  const { setTheme } = useTheme();
  const [currentGraph, setGraph] = useState(fixtureGraph);
  const [selection, setSelection] = useState<string | null>(null);
  const [run, setRun] = useState(() =>
    fileDriven ? makeFileDrivenRun() : makeRun("running")
  );
  const runtimeView = getWorkflowRuntimeView(run);
  const runtimeSummary = buildWorkflowRunDashboardSummary(run, runtimeView);
  const reusedWork = runtimeView.node_work.find(
    (work) => work.status === "reused"
  );
  const selected = currentGraph.nodes.find((node) => node.id === selection);

  return (
    <div
      className="new-design"
      style={{ height: "100dvh", display: "flex", flexDirection: "column" }}
    >
      <header style={{ display: "flex", gap: 16, padding: 12 }}>
        <button onClick={() => setTheme(ThemeMode.LIGHT)}>Light</button>
        <button onClick={() => setTheme(ThemeMode.DARK)}>Dark</button>
        <button onClick={() => setTheme(ThemeMode.SYSTEM)}>System</button>
        {runtime ? (
          <>
            {fileDriven ? (
              <>
                <WorkflowRunResult run={run} projectId="visual-project" />
                <button onClick={() => setRun(makePartialRun())}>
                  Show partial delivery
                </button>
                <button
                  onClick={() => {
                    const next = makeFileDrivenRun();
                    next.split_batches[0].state = "pending";
                    next.split_batches[0].dispatch_path = null;
                    next.split_batches[0].handover!.state = "claimed";
                    next.split_batches[0].subtasks =
                      next.split_batches[0].subtasks.map((task) => ({
                        ...task,
                        state: "pending",
                        node_id: null,
                        node_execution_id: null,
                        session_id: null,
                        handoff_path: null,
                      }));
                    setRun(next);
                  }}
                >
                  Await dispatch
                </button>
              </>
            ) : null}
            <button onClick={() => setRun(makeRun("succeeded"))}>
              Complete run
            </button>
            <button onClick={() => setRun(makeReusedRun())}>
              Rework with reused results
            </button>
            <output hidden data-testid="runtime-facts">
              {JSON.stringify({
                completed: runtimeSummary.completedSteps,
                freshSteps: runtimeSummary.freshSteps,
                progressPercent: runtimeSummary.progressPercent,
                reused: runtimeSummary.reusedSteps,
                skipped: runtimeSummary.skippedSteps,
                tokens: runtimeSummary.totalTokens,
                actualExecutionIds: run.nodes.map((execution) => execution.id),
                reusedExecution: getWorkflowNodeExecutionForWork(
                  run,
                  reusedWork
                ),
                reusedActions: getWorkflowNodeActionGate(reusedWork),
              })}
            </output>
          </>
        ) : null}
      </header>
      <div
        data-testid="visual-canvas"
        style={{ flex: 1, minHeight: 0, position: "relative" }}
      >
        {runtime ? (
          <WorkflowRunCanvasTab projectId="visual-project" run={run} />
        ) : (
          <WorkflowCanvas
            graph={currentGraph}
            onChange={setGraph}
            selectedNodeId={selection}
            onSelectionChange={(next) => setSelection(next.nodeId)}
            onNodeEdit={setSelection}
          />
        )}
        {!runtime ? (
          <WorkflowConfigurationFrame
            open={!!selected}
            title={selected?.data.display_name ?? "Node"}
            description="Configure this Node"
            objectKey={selection ?? ""}
            onClose={() => setSelection(null)}
          >
            <label style={{ display: "block", padding: 16 }}>
              Task name
              <input
                aria-label="Task name"
                value={selected?.data.display_name ?? ""}
                onChange={(event) =>
                  setGraph((current) => ({
                    ...current,
                    nodes: current.nodes.map((node) =>
                      node.id === selection
                        ? {
                            ...node,
                            data: {
                              ...node.data,
                              display_name: event.target.value,
                            },
                          }
                        : node
                    ),
                  }))
                }
              />
            </label>
            <div style={{ height: 1200, padding: 16 }}>
              Scrollable Node configuration
            </div>
          </WorkflowConfigurationFrame>
        ) : null}
      </div>
    </div>
  );
}

const router = createConversationFixtureRouter(
  new URLSearchParams(location.search).get("mode") === "scratch-scope" ? (
    <ScratchScopeFixture />
  ) : new URLSearchParams(location.search).get("mode") ===
    "creation-handoff" ? (
    <WorkspaceCreationHandoffPreview />
  ) : (
    <ReactFlowProvider>
      <Fixture />
    </ReactFlowProvider>
  )
);

createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={client}>
    <AppNavigationProvider value={navigation}>
      <ThemeProvider initialTheme={ThemeMode.LIGHT}>
        <RouterProvider router={router} />
      </ThemeProvider>
    </AppNavigationProvider>
  </QueryClientProvider>
);
