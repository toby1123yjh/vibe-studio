import { expect, test } from "@playwright/test";
import type {
  WorkflowContextFileView,
  WorkflowFileContextView,
  WorkflowNodeExecutionResponse,
  WorkflowRunResponse,
  WorkflowSplitSubtaskView,
} from "../../../shared/types";
import {
  buildWorkflowConversationHref,
  getWorkflowNodeConversations,
  getWorkflowOutputCount,
  resolveWorkflowConversationContext,
  resolveWorkflowSourceConversation,
  selectWorkflowConversation,
  workflowCandidateConversation,
  workflowConversationScopeKey,
  workflowSourceConversation,
  workflowSubtaskConversation,
} from "../../../packages/web-core/src/features/workflow/model/workflowConversationTarget";
import { getWorkflowRuntimeView } from "../../../packages/web-core/src/features/workflow/model/workflowRuntimeView";
import { conversationScopeKey } from "../../../packages/web-core/src/features/workspace-chat/model/conversationViewState";

function execution(
  id: string,
  overrides: Partial<WorkflowNodeExecutionResponse> = {},
): WorkflowNodeExecutionResponse {
  return {
    id,
    run_id: "run",
    execution_id: null,
    node_id: "writer",
    node_type: "agent",
    iteration: 0n,
    status: "running",
    input_text: null,
    output_text: null,
    session_id: `session-${id}`,
    orchestration_node_execution_id: `orchestration-${id}`,
    agent_run_id: `agent-${id}`,
    projection_status: "current",
    execution_process_id: null,
    arena_group_id: null,
    tokens_used: null,
    cost_estimate: null,
    started_at: null,
    finished_at: null,
    error_text: null,
    created_at: "2026-10-15T00:00:00Z",
    updated_at: "2026-10-15T00:00:00Z",
    ...overrides,
  };
}

function run(): WorkflowRunResponse {
  return {
    id: "run",
    orchestration_run_id: null,
    workflow_id: "template",
    attempt_id: "instance",
    task_id: "business-task",
    workspace_id: "workspace",
    trigger_source: "manual",
    input_text: "",
    output_text: null,
    status: "running",
    started_at: null,
    finished_at: null,
    error_text: null,
    created_at: "2026-10-15T00:00:00Z",
    updated_at: "2026-10-15T00:00:00Z",
    nodes: [execution("first"), execution("second")],
    split_batches: [],
    handoffs: [],
    delivery: null,
    request_path: null,
  };
}

function subtask(
  id: string,
  overrides: Partial<WorkflowSplitSubtaskView> = {},
): WorkflowSplitSubtaskView {
  return {
    id,
    subtask_id: id,
    title: `Work ${id}`,
    instruction_path: `instructions/${id}.md`,
    node_id: "writer",
    stage_index: 0,
    state: "running",
    depends_on: [],
    node_execution_id: id,
    session_id: `session-${id}`,
    agent_run_id: `agent-${id}`,
    error_text: null,
    handoff_path: null,
    ...overrides,
  };
}

test("same-Node internal work resolves its exact execution and never a sibling Session", () => {
  const value = run();
  expect(
    workflowSubtaskConversation(value, "batch", subtask("first")),
  ).toMatchObject({
    nodeExecutionId: "first",
    sessionId: "session-first",
    workspaceId: "workspace",
  });
  for (const invalid of [
    subtask("first", { session_id: "session-second" }),
    subtask("first", { node_id: "another-node" }),
    subtask("missing"),
    subtask("pending", { node_execution_id: null, session_id: null }),
  ]) {
    expect(
      resolveWorkflowConversationContext(
        workflowSubtaskConversation(value, "batch", invalid),
        undefined,
      ).sessionId,
    ).toBeNull();
  }
});

test("cloned accepted work uses exact execution provenance without inferring a Session from files", () => {
  const target = workflowSubtaskConversation(
    run(),
    "cloned-batch",
    subtask("original-worker"),
  );
  expect(target).toMatchObject({
    nodeExecutionId: "original-worker",
    workspaceId: null,
    readOnly: true,
  });
  const context: WorkflowFileContextView = {
    run_id: "run",
    workspace_id: "workspace",
    node_execution_id: "original-worker",
    execution: {
      run_id: "original-run",
      workspace_id: "original-workspace",
      node_id: "writer",
      node_execution_id: "original-worker",
      session_id: "session-original-worker",
      status: "succeeded",
    },
    files: [],
    missing_subtasks: [],
  };
  expect(resolveWorkflowConversationContext(target, context)).toMatchObject({
    kind: "source",
    runId: "original-run",
    workspaceId: "original-workspace",
    sessionId: "session-original-worker",
    readOnly: true,
  });
  for (const execution of [
    { ...context.execution!, node_execution_id: "other-worker" },
    { ...context.execution!, node_id: "other-node" },
    { ...context.execution!, session_id: "session-other-worker" },
    null,
  ]) {
    expect(
      resolveWorkflowConversationContext(target, { ...context, execution })
        .sessionId,
    ).toBeNull();
  }
  expect(
    resolveWorkflowConversationContext(target, {
      ...context,
      run_id: "wrong-run",
    }).sessionId,
  ).toBeNull();
});

test("polling preserves selected work; first open uses active work then stable order", () => {
  const value = run();
  value.split_batches = [
    {
      id: "batch",
      run_id: value.id,
      node_execution_id: "split",
      splitter_node_id: "splitter",
      summary_node_id: "summary",
      manifest_path: "tasks.json",
      manifest_sha256: "a".repeat(64),
      dispatch_path: "dispatch.json",
      state: "running",
      error_text: null,
      source_batch_id: null,
      summary_node_execution_id: null,
      handover: null,
      subtasks: [
        subtask("second", { stage_index: 1 }),
        subtask("first", { state: "succeeded" }),
      ],
    },
  ];
  const targets = getWorkflowNodeConversations(
    value,
    getWorkflowRuntimeView(value),
    "writer",
    "Writer",
  );
  expect(targets.map((target) => target.sessionId)).toEqual([
    "session-first",
    "session-second",
  ]);
  expect(selectWorkflowConversation(targets)?.sessionId).toBe("session-second");
  expect(selectWorkflowConversation(targets, targets[0].key)?.sessionId).toBe(
    "session-first",
  );
  expect(
    selectWorkflowConversation(
      targets.map((target) => ({ ...target, status: "succeeded" })),
    )?.sessionId,
  ).toBe("session-first");
  expect(selectWorkflowConversation([], targets[0].key)).toBeNull();
});

test("candidate uses its own workspace and host-scoped full-session route", () => {
  const value = run();
  const target = workflowCandidateConversation({
    run: value,
    execution: execution("arena", { node_type: "arena", session_id: null }),
    candidateId: "candidate",
    workspaceId: "candidate-workspace",
    sessionId: "candidate-session",
    title: "Candidate",
    status: "running",
  });
  expect(target.workspaceId).toBe("candidate-workspace");
  expect(
    buildWorkflowConversationHref({
      hostId: "remote host",
      projectId: "project",
      taskId: value.task_id,
      workspaceId: target.workspaceId,
      sessionId: target.sessionId,
    }),
  ).toBe(
    "/hosts/remote%20host/projects/project/tasks/business-task/workspaces/candidate-workspace?session_id=candidate-session",
  );
  expect(
    buildWorkflowConversationHref({
      hostId: null,
      projectId: "project",
      taskId: value.task_id,
      workspaceId: null,
      sessionId: target.sessionId,
    }),
  ).toBeNull();
});

test("historical source requires exact Run, Node and execution and stays read-only", () => {
  const source = workflowSourceConversation(
    {
      node_id: "writer",
      iteration: 0,
      source_run_id: "earlier",
      source_node_execution_id: "first",
      output_text: "Accepted earlier result",
    },
    "Earlier writer",
  );
  expect(source.sessionId).toBeNull();
  expect(resolveWorkflowSourceConversation(source, run()).sessionId).toBeNull();
  const earlier = {
    ...run(),
    id: "earlier",
    workspace_id: "earlier-workspace",
  };
  expect(resolveWorkflowSourceConversation(source, earlier)).toMatchObject({
    sessionId: "session-first",
    workspaceId: "earlier-workspace",
    readOnly: true,
  });
  earlier.nodes[0].node_id = "different-node";
  expect(
    resolveWorkflowSourceConversation(source, earlier).sessionId,
  ).toBeNull();
});

test("presentation scopes isolate identical Session IDs across hosts and workspaces", () => {
  const target = workflowSubtaskConversation(run(), "batch", subtask("first"));
  const scopes = [
    workflowConversationScopeKey(null, target),
    workflowConversationScopeKey("remote", target),
    workflowConversationScopeKey(null, { ...target, workspaceId: "other" }),
    workflowConversationScopeKey(null, { ...target, runId: "other-run" }),
    workflowConversationScopeKey(null, {
      ...target,
      nodeExecutionId: "other-execution",
    }),
  ];
  expect(new Set(scopes).size).toBe(scopes.length);
  expect(conversationScopeKey(null, "workspace", "session")).not.toBe(
    conversationScopeKey("remote", "workspace", "session"),
  );
  expect(conversationScopeKey(null, "workspace", "session")).not.toBe(
    conversationScopeKey(null, "candidate-workspace", "session"),
  );
});

test("deliverable count excludes inputs and protocol files, deduplicating accepted output references", () => {
  function file(
    role: WorkflowContextFileView["role"],
    path: string,
    workspace = "workspace",
  ): WorkflowContextFileView {
    return {
      role,
      path,
      sha256: null,
      source_run_id: "run",
      source_workspace_id: workspace,
      source_node_execution_id: "first",
      source_node_id: "writer",
      source_session_id: "session-first",
    };
  }
  expect(
    getWorkflowOutputCount([
      file("output", "report.md"),
      file("output", "report.md"),
      file("input", "request.md"),
      file("control", "tasks.json"),
      file("control", "dispatch.json"),
      file("handoff", "handoff.json"),
    ]),
  ).toBe(1);
  expect(
    getWorkflowOutputCount([
      file("output", "report.md"),
      file("output", "report.md", "candidate-workspace"),
    ]),
  ).toBe(2);
});
