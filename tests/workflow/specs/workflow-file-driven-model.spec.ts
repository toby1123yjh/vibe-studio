import { expect, test } from "@playwright/test";
import type {
  WorkflowRunResponse,
  WorkflowSplitBatchView,
  WorkflowSplitSubtaskView,
} from "../../../shared/types";
import type { WorkflowGraph } from "../../../packages/web-core/src/features/workflow/model/workflowGraph";
import { getWorkflowSplitScopes } from "../../../packages/web-core/src/features/workflow/model/workflowSplitScope";
import {
  buildWorkflowPartialSummarySubmission,
  canRequestWorkflowPartialSummary,
  getWorkflowSplitBatchesForNode,
  getWorkflowSplitNodeProgress,
  getWorkflowSplitProgress,
  getWorkflowSplitStages,
} from "../../../packages/web-core/src/features/workflow/model/workflowRuntimeView";
import { getWorkflowNodeCatalogSections } from "../../../packages/web-core/src/features/workflow/model/workflowNodeCatalog";

function graph(): WorkflowGraph {
  return {
    version: 2,
    nodes: [
      { id: "start", type: "start", data: {} },
      { id: "split", type: "task_splitter", data: {} },
      { id: "writer", type: "agent", data: {} },
      { id: "reviewer", type: "agent", data: {} },
      { id: "summary", type: "agent", data: {} },
      { id: "end", type: "end", data: {} },
    ],
    edges: [
      { id: "s", source: "start", target: "split", type: "default" },
      { id: "w", source: "split", target: "writer", type: "default" },
      { id: "r", source: "split", target: "reviewer", type: "default" },
      { id: "ws", source: "writer", target: "summary", type: "default" },
      { id: "rs", source: "reviewer", target: "summary", type: "default" },
      { id: "e", source: "summary", target: "end", type: "default" },
    ],
  };
}

function subtask(
  id: string,
  overrides: Partial<WorkflowSplitSubtaskView> = {},
): WorkflowSplitSubtaskView {
  return {
    id: `record-${id}`,
    subtask_id: id,
    title: `Task ${id}`,
    instruction_path: `.vibe-kanban/workflows/run/instructions/${id}.md`,
    node_id: "writer",
    stage_index: 0,
    state: "pending",
    depends_on: [],
    node_execution_id: null,
    session_id: null,
    agent_run_id: null,
    error_text: null,
    handoff_path: null,
    ...overrides,
  };
}

function batch(): WorkflowSplitBatchView {
  return {
    id: "batch",
    run_id: "run",
    node_execution_id: "split-execution",
    splitter_node_id: "split",
    summary_node_id: "summary",
    manifest_path: ".vibe-kanban/workflows/run/tasks.json",
    manifest_sha256: "a".repeat(64),
    dispatch_path: ".vibe-kanban/workflows/run/dispatch.json",
    state: "partial",
    error_text: null,
    source_batch_id: null,
    summary_node_execution_id: null,
    handover: null,
    subtasks: [
      subtask("done", {
        state: "succeeded",
        handoff_path: "done/handoff.json",
        session_id: "session-a",
      }),
      subtask("failed", {
        state: "failed",
        node_id: "reviewer",
        error_text: "instructions missing",
        session_id: "session-b",
      }),
      subtask("later", {
        state: "blocked",
        stage_index: 1,
        depends_on: ["failed"],
      }),
    ],
  };
}

function run(): WorkflowRunResponse {
  return {
    id: "run",
    orchestration_run_id: null,
    workflow_id: "workflow",
    attempt_id: "instance",
    task_id: "business-task",
    workspace_id: "workspace",
    trigger_source: "manual",
    input_text: "Original request",
    output_text: null,
    status: "failed",
    started_at: null,
    finished_at: null,
    error_text: null,
    created_at: "2026-10-13T00:00:00Z",
    updated_at: "2026-10-13T00:00:00Z",
    nodes: [],
    split_batches: [batch()],
    handoffs: [],
    delivery: null,
    request_path: null,
  };
}

test("splitters are available without exposing structural nodes or internal task entries", () => {
  const types = getWorkflowNodeCatalogSections().flatMap((section) =>
    section.entries.map((entry) => entry.type),
  );
  expect(types).toEqual([
    "agent",
    "task_splitter",
    "arena",
    "condition",
    "human_gate",
    "transform",
  ]);
});

test("the frozen dispatch scope comes only from the existing executor connections", () => {
  const value = graph();
  const before = structuredClone(value);
  expect(getWorkflowSplitScopes(value)).toEqual({
    scopes: [
      {
        splitterNodeId: "split",
        candidateNodeIds: ["writer", "reviewer"],
        summaryNodeId: "summary",
      },
    ],
    issues: [],
  });
  expect(value).toEqual(before);
});

for (const type of ["agent", "transform", "end"] as const) {
  test(`a ${type} may serve as the fixed summary exit`, () => {
    const value = graph();
    value.nodes.find((node) => node.id === "summary")!.type = type;
    expect(getWorkflowSplitScopes(value).issues).toEqual([]);
  });
}

const invalidScopes: {
  name: string;
  expected: string;
  change: (value: WorkflowGraph) => void;
}[] = [
  {
    name: "no candidates",
    expected: "missingExecutors",
    change: (value) => {
      value.edges = value.edges.filter((edge) => edge.source !== "split");
    },
  },
  {
    name: "non-Agent executor",
    expected: "invalidExecutor",
    change: (value) => {
      value.nodes.find((node) => node.id === "writer")!.type = "condition";
    },
  },
  {
    name: "duplicate candidate",
    expected: "competingExecutorInputs",
    change: (value) => {
      value.edges.push({
        id: "duplicate",
        source: "split",
        target: "writer",
        type: "default",
      });
    },
  },
  {
    name: "non-default executor edge",
    expected: "invalidExecutor",
    change: (value) => {
      value.edges.find((edge) => edge.id === "w")!.type = "approval";
    },
  },
  {
    name: "competing candidate input",
    expected: "competingExecutorInputs",
    change: (value) => {
      value.edges.push({
        id: "other",
        source: "start",
        target: "writer",
        type: "default",
      });
    },
  },
  {
    name: "missing summary",
    expected: "missingSummary",
    change: (value) => {
      value.edges = value.edges.filter((edge) => edge.id !== "ws");
    },
  },
  {
    name: "different summaries",
    expected: "differentSummaries",
    change: (value) => {
      value.edges.find((edge) => edge.id === "rs")!.target = "end";
    },
  },
  {
    name: "summary with an outside input",
    expected: "competingSummaryInputs",
    change: (value) => {
      value.edges.push({
        id: "outside",
        source: "start",
        target: "summary",
        type: "default",
      });
    },
  },
  {
    name: "unsupported summary",
    expected: "invalidSummary",
    change: (value) => {
      value.nodes.find((node) => node.id === "summary")!.type = "human_gate";
    },
  },
];
for (const scenario of invalidScopes) {
  test(`rejects ${scenario.name} before dispatch`, () => {
    const value = graph();
    scenario.change(value);
    expect(getWorkflowSplitScopes(value).issues).toEqual([
      { nodeId: "split", code: scenario.expected },
    ]);
    expect(getWorkflowSplitScopes(value).scopes).toEqual([]);
  });
}

test("parallel subtasks keep separate sessions and serial stages retain failed dependencies", () => {
  const value = batch();
  value.subtasks.reverse();
  const stages = getWorkflowSplitStages(value);
  expect(stages.map((stage) => stage.index)).toEqual([0, 1]);
  expect(stages[0].subtasks.map((task) => task.session_id).sort()).toEqual([
    "session-a",
    "session-b",
  ]);
  expect(stages[1].subtasks[0]).toMatchObject({
    state: "blocked",
    depends_on: ["failed"],
  });
  expect(value.subtasks[1].error_text).toBe("instructions missing");
});

test("a successful partial summary cannot inflate original subtask completion", () => {
  const value = run();
  value.status = "succeeded";
  value.delivery = {
    completeness: "partial",
    source_run_id: "previous-run",
    source_batch_id: "batch",
    missing_subtasks: [
      {
        subtask_id: "failed",
        title: "Task failed",
        state: "failed",
        reason: "instructions missing",
      },
    ],
  };
  expect(getWorkflowSplitProgress(value.split_batches[0].subtasks)).toEqual({
    total: 3,
    completed: 1,
    failed: 1,
    running: 0,
  });
  expect(getWorkflowSplitNodeProgress(value, "writer")).toEqual({
    total: 2,
    completed: 1,
    failed: 0,
    running: 0,
  });
  expect(getWorkflowSplitNodeProgress(value, "reviewer")).toEqual({
    total: 1,
    completed: 0,
    failed: 1,
    running: 0,
  });
  expect(getWorkflowSplitNodeProgress(value, "unrelated")).toBeNull();
  expect(getWorkflowSplitBatchesForNode(value, "summary")).toHaveLength(1);
  expect(value.delivery.missing_subtasks[0].state).toBe("failed");
});

test("an explicit partial request preserves source identity and waits for ongoing work", () => {
  const value = run();
  value.status = "running";
  const payload = buildWorkflowPartialSummarySubmission(
    value,
    value.split_batches[0],
    "request-1",
    "Read the accepted manifest",
  );
  expect(payload).toEqual({
    request_id: "request-1",
    action: "rework",
    source_run_id: value.id,
    source_batch_id: "batch",
    delivery_intent: "partial",
    scope: { type: "from_nodes", node_ids: ["summary"] },
    active_policy: "after_current",
    input_text: "Read the accepted manifest",
    material_paths: [".vibe-kanban/workflows/run/tasks.json"],
  });
  expect(
    buildWorkflowPartialSummarySubmission(
      value,
      value.split_batches[0],
      "request-1",
      "Read the accepted manifest",
    ),
  ).toEqual(payload);
  expect(value.split_batches[0].subtasks[1].state).toBe("failed");
});

for (const scenario of [
  "empty",
  "no-files",
  "all-succeeded",
  "foreign-run",
  "stopping",
  "no-instance",
] as const) {
  test(`partial summary is unavailable for ${scenario}`, () => {
    const value = run();
    const split = value.split_batches[0];
    if (scenario === "empty") split.subtasks = [];
    if (scenario === "no-files") split.subtasks[0].handoff_path = null;
    if (scenario === "all-succeeded")
      split.subtasks.forEach((task) => {
        task.state = "succeeded";
      });
    if (scenario === "foreign-run") split.run_id = "other-run";
    if (scenario === "stopping") value.status = "cancelling";
    if (scenario === "no-instance") value.attempt_id = null;
    expect(canRequestWorkflowPartialSummary(value, split)).toBe(false);
    expect(
      buildWorkflowPartialSummarySubmission(
        value,
        split,
        "request",
        "No implicit submission",
      ),
    ).toBeNull();
  });
}
