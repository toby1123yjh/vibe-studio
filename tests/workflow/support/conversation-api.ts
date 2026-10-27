import type { Page, WebSocketRoute } from "@playwright/test";
import {
  AgentRuntimeToolStatus,
  type AgentRunStatus,
  type ConversationBlock,
} from "../../../shared/types";

const timestamp = "2026-10-15T00:00:00Z";
const sessions = ["research", "review", "analysis", "main"].map((work) => ({
  id: `${work}-session`,
  workspace_id: "visual-workspace",
  name: work,
  executor: "CODEX",
  agent_working_dir: null,
  created_at: timestamp,
  updated_at: timestamp,
}));

function runState(
  sessionId: string,
  withToolOutput = false,
  status: `${AgentRunStatus}` = "succeeded",
) {
  return {
    state_schema_version: 1,
    reducer_version: 1,
    session_id: sessionId,
    agent_run_id: `agent-${sessionId}`,
    turn_id: `turn-${sessionId}`,
    status,
    projection_status: "current",
    last_run_attempt_id: `attempt-${sessionId}`,
    last_run_attempt_number: 1,
    last_event_sequence: withToolOutput ? 2 : 1,
    last_event_id: `${withToolOutput ? "tool" : "event"}-${sessionId}`,
    provider_session: null,
    terminal_output: null,
    last_error: null,
    unknown_event_count: 0,
    updated_at: timestamp,
  };
}

/** Mocks transport only: production timeline, editor, draft and actions are real. */
export async function mockWorkflowConversations(
  page: Page,
  {
    holdFirstDraftWrite = false,
    withToolOutput = false,
    initialRunStatus = "succeeded",
    initialHistory = true,
  }: {
    holdFirstDraftWrite?: boolean;
    withToolOutput?: boolean;
    initialRunStatus?: `${AgentRunStatus}`;
    initialHistory?: boolean;
  } = {},
) {
  const writes: string[] = [];
  const drafts = new Map<string, unknown>();
  const scratches = new Map<string, Set<WebSocketRoute>>();
  const requests: string[] = [];
  const runSockets = new Map<string, Set<WebSocketRoute>>();
  const runStatuses = new Map<string, `${AgentRunStatus}`>();
  const userTurns = new Map<
    string,
    Array<{ runId: string; prompt: string; status: `${AgentRunStatus}` }>
  >();
  let revision = 1;
  const currentState = (sessionId: string) =>
    runState(
      sessionId,
      withToolOutput,
      runStatuses.get(sessionId) ?? initialRunStatus,
    );
  const acknowledgement = (
    sessionId: string,
    runId = `agent-${sessionId}`,
    status: `${AgentRunStatus}` = "running",
  ) => ({
    agent_run_id: runId,
    state: {
      ...runState(sessionId, false, status),
      agent_run_id: runId,
      turn_id: `turn-${runId}`,
      last_run_attempt_id: `attempt-${runId}`,
    },
  });
  let heldFirstDraftWrite = false;
  let releaseDraftWrite = () => {};
  const conversation = (sessionId: string) => {
    const block = (
      kind: ConversationBlock["kind"],
      fields: ConversationBlock["fields"],
    ) => ({
      block_id: `${kind}-${sessionId}`,
      agent_run_id: `agent-${sessionId}`,
      run_attempt_id: `attempt-${sessionId}`,
      kind,
      role: "assistant",
      native_id: null,
      parent_block_id: null,
      position: kind === "text" ? "1" : "2",
      revision: String(revision),
      fields,
      bodies: [],
      finished: true,
      finish_reason: "completed",
      source_event_id: `event-${sessionId}`,
      created_at: timestamp,
      updated_at: timestamp,
    });
    return {
      session_id: sessionId,
      cursor: { epoch: `epoch-${sessionId}`, revision: String(revision) },
      blocks: [
        ...(initialHistory
          ? [block("text", { text: `Original reply for ${sessionId}` })]
          : []),
        ...(initialHistory && withToolOutput
          ? [
              block("tool", {
                tool_call_id: `tool-call-${sessionId}`,
                tool_name: "inspect_fixture",
                status: AgentRuntimeToolStatus.succeeded,
                arguments: { query: "fixture documents" },
                output_text:
                  "Exact tool output retained in the original viewer",
              }),
            ]
          : []),
        ...(userTurns.get(sessionId) ?? []).map((turn, index) => ({
          ...block("text", { text: turn.prompt }),
          block_id: `user-${turn.runId}`,
          agent_run_id: turn.runId,
          run_attempt_id: `attempt-${turn.runId}`,
          role: "user",
          position: `3:${String(index).padStart(4, "0")}`,
          source_event_id: `submitted-${turn.runId}`,
        })),
      ],
      runs: [
        ...(initialHistory
          ? [
              {
                agent_run_id: `agent-${sessionId}`,
                session_id: sessionId,
                turn_id: `turn-${sessionId}`,
                state: currentState(sessionId),
                created_at: timestamp,
                updated_at: timestamp,
              },
            ]
          : []),
        ...(userTurns.get(sessionId) ?? []).map((turn) => ({
          ...acknowledgement(sessionId, turn.runId, turn.status),
          session_id: sessionId,
          turn_id: `turn-${turn.runId}`,
          created_at: timestamp,
          updated_at: timestamp,
        })),
      ],
      pending_interactions: [],
      previous_cursor: null,
      has_more: false,
    };
  };
  const publishConversation = (sessionId: string) => {
    revision += 1;
    for (const socket of runSockets.get(sessionId) ?? []) {
      socket.send(
        JSON.stringify({
          type: "changes",
          data: {
            ...conversation(sessionId),
            tombstones: [],
            reset_required: false,
          },
        }),
      );
    }
  };

  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    requests.push(`${request.method()} ${path}`);
    const fulfill = (data: unknown) =>
      route.fulfill({
        json: { success: true, data, error_data: null, message: null },
      });
    if (path === "/api/sessions") return fulfill(sessions);
    if (path === "/api/workspaces/visual-workspace")
      return fulfill({
        id: "visual-workspace",
        container_ref: "C:/fixture",
        workspace_kind: "direct_folder",
        container_ownership: "external",
        branch: "direct-folder",
        setup_completed_at: timestamp,
        created_at: timestamp,
        updated_at: timestamp,
        archived: false,
        pinned: false,
        name: "Fixture",
        worktree_deleted: false,
      });
    if (
      path === "/api/workspaces/visual-workspace/repos" ||
      path.endsWith("/git/status") ||
      path.endsWith("/garage")
    )
      return fulfill([]);
    if (path.endsWith("/executor-config"))
      return fulfill({
        executor: "CODEX",
        variant: "DEFAULT",
        model_id: "fixture-model",
      });
    if (/\/sessions\/[^/]+\/conversation$/.test(path))
      return fulfill(conversation(path.split("/")[3]));
    if (path.startsWith("/api/agent-runs/session/")) {
      const sessionId = path.split("/").at(-1)!;
      return fulfill(conversation(sessionId).runs);
    }
    if (path.includes("/workflow-management/") && path.endsWith("/context"))
      return fulfill(null);
    if (path.startsWith("/api/scratch/") && request.method() === "PUT") {
      const sessionId = path.split("/").at(-1)!;
      const scratch = {
        id: sessionId,
        payload: request.postDataJSON().payload,
      };
      writes.push(sessionId);
      if (holdFirstDraftWrite && !heldFirstDraftWrite) {
        heldFirstDraftWrite = true;
        await new Promise<void>((resolve) => {
          releaseDraftWrite = resolve;
        });
      }
      drafts.set(sessionId, scratch);
      for (const socket of scratches.get(sessionId) ?? []) {
        socket.send(
          JSON.stringify({
            JsonPatch: [{ op: "replace", path: "/scratch", value: scratch }],
          }),
        );
      }
      return fulfill(scratch);
    }
    if (path.startsWith("/api/scratch/") && request.method() === "DELETE") {
      const sessionId = path.split("/").at(-1)!;
      drafts.delete(sessionId);
      for (const socket of scratches.get(sessionId) ?? []) {
        socket.send(
          JSON.stringify({
            JsonPatch: [{ op: "replace", path: "/scratch", value: null }],
          }),
        );
      }
      return fulfill(null);
    }
    return route.fallback();
  });

  await page.routeWebSocket("**/api/**", (socket) => {
    const url = new URL(socket.url());
    if (url.pathname.includes("/scratch/")) {
      const sessionId = url.pathname.split("/").at(-3)!;
      const listeners = scratches.get(sessionId) ?? new Set<WebSocketRoute>();
      listeners.add(socket);
      scratches.set(sessionId, listeners);
      socket.onClose(() => listeners.delete(socket));
      socket.send(
        JSON.stringify({
          JsonPatch: [
            {
              op: "replace",
              path: "/scratch",
              value: drafts.get(sessionId) ?? null,
            },
          ],
        }),
      );
      socket.send(JSON.stringify({ Ready: true }));
    } else if (url.pathname.includes("/conversation/stream/ws")) {
      const sessionId = url.pathname.split("/")[3];
      const listeners = runSockets.get(sessionId) ?? new Set<WebSocketRoute>();
      listeners.add(socket);
      runSockets.set(sessionId, listeners);
      socket.onClose(() => listeners.delete(socket));
      socket.send(
        JSON.stringify({
          type: "ready",
          data: conversation(sessionId).cursor,
        }),
      );
    } else {
      // Empty process/diff/discovery streams do not create an Agent process.
      socket.send(JSON.stringify({ Ready: true }));
    }
  });
  return {
    writes,
    drafts,
    requests,
    releaseDraftWrite: () => releaseDraftWrite(),
    state: currentState,
    acknowledgement,
    publishUserMessage: (
      sessionId: string,
      prompt: string,
      runId: string,
      status: `${AgentRunStatus}` = "running",
    ) => {
      const turns = userTurns.get(sessionId) ?? [];
      const index = turns.findIndex((turn) => turn.runId === runId);
      const turn = { runId, prompt, status };
      if (index === -1) turns.push(turn);
      else turns[index] = turn;
      userTurns.set(sessionId, turns);
      publishConversation(sessionId);
      return acknowledgement(sessionId, runId, status);
    },
    setRunStatus: (sessionId: string, status: `${AgentRunStatus}`) => {
      runStatuses.set(sessionId, status);
      publishConversation(sessionId);
    },
  };
}

export async function mockWorkflowFileContext(page: Page) {
  await page.route(
    "**/api/workflow-runs/visual-run/file-context**",
    async (route) => {
      const nodeExecutionId = new URL(route.request().url()).searchParams.get(
        "node_execution_id",
      );
      await route.fulfill({
        json: {
          success: true,
          data: {
            run_id: "visual-run",
            workspace_id: "visual-workspace",
            node_execution_id: nodeExecutionId,
            execution: nodeExecutionId
              ? {
                  node_execution_id: nodeExecutionId,
                  node_id:
                    nodeExecutionId === "split-execution" ? "plan" : "build",
                  run_id: "visual-run",
                  workspace_id: "visual-workspace",
                  session_id:
                    (
                      {
                        "research-execution": "research-session",
                        "review-execution": "review-session",
                        "analysis-execution": "analysis-session",
                      } as Record<string, string>
                    )[nodeExecutionId] ?? null,
                  status:
                    nodeExecutionId === "analysis-execution"
                      ? "running"
                      : "succeeded",
                }
              : null,
            files: [
              {
                role: "output",
                path: "deliverables/research report.md",
                sha256: "b".repeat(64),
                source_node_execution_id: "research-execution",
              },
              {
                role: "input",
                path: ".vibe-kanban/workflows/visual-run/request.md",
                sha256: null,
                source_node_execution_id: null,
              },
              {
                role: "control",
                path: ".vibe-kanban/workflows/visual-run/tasks.json",
                sha256: "a".repeat(64),
                source_node_execution_id: "split-execution",
              },
              {
                role: "control",
                path: ".vibe-kanban/workflows/visual-run/dispatch.json",
                sha256: null,
                source_node_execution_id: "split-execution",
              },
            ].map((file) => ({
              ...file,
              source_run_id: "visual-run",
              source_workspace_id: "visual-workspace",
              source_node_id: file.role === "output" ? "build" : "plan",
              source_session_id:
                file.role === "output" ? "research-session" : null,
            })),
            missing_subtasks: [],
          },
        },
      });
    },
  );
}
