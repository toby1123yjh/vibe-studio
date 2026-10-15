import { getCurrentHostId } from '@/shared/providers/HostIdProvider';
import { makeLocalApiRequest } from '@/shared/lib/localApiTransport';
import { handleApiResponse } from '@/shared/lib/api';
import { scopeWorkspaceFileRawUrl } from '@/shared/lib/workspaceFileRawUrl';
import type {
  WorkflowTemplateListResponse,
  WorkflowTemplateResponse,
  CreateWorkflowRequest,
  UpdateWorkflowRequest,
  TriggerWorkflowRequest,
  CreateWorkflowAttemptRequest,
  DraftWorkspaceRepo,
  RunWorkflowAttemptRequest,
  WorkflowAttemptListResponse,
  WorkflowAttemptResponse,
  WorkflowRunResponse,
  WorkflowFileContent,
  WorkflowFileContextView,
  WorkflowActionResponse,
  WorkflowRevisionConflict,
  WorkflowFileChangeSummary,
} from 'shared/types';

const LOCAL_BASE = '/api/local/v1';

interface MutationResponse<T> {
  data: T;
  txid: number;
}

export class WorkflowRevisionConflictError extends Error {
  constructor(public readonly conflict: WorkflowRevisionConflict) {
    super(
      `Workflow changed from revision ${conflict.expected_revision} to ${conflict.current_revision}`
    );
    this.name = 'WorkflowRevisionConflictError';
  }
}

export type CreateWorkflowAttemptPayload = CreateWorkflowAttemptRequest & {
  repos?: DraftWorkspaceRepo[];
};

export type RunWorkflowAttemptPayload = RunWorkflowAttemptRequest & {
  repos?: DraftWorkspaceRepo[];
};

export type WorkflowEventKind =
  | 'run_status'
  | 'node_status'
  | 'node_output'
  | 'node_error'
  | 'node_waiting_human'
  | 'node_waiting_arena';

export interface WorkflowRuntimeEvent {
  sequence: number;
  run_id: string;
  node_id: string | null;
  kind: WorkflowEventKind;
  status: string | null;
  payload: unknown;
}

export function parseWorkflowEvent(
  event: MessageEvent
): WorkflowRuntimeEvent | null {
  try {
    const data = JSON.parse(event.data);
    if (data && typeof data === 'object' && 'kind' in data) {
      return data as WorkflowRuntimeEvent;
    }
  } catch {
    // Ignore parse errors
  }
  return null;
}

async function localRequest(
  path: string,
  init: RequestInit = {},
  hostId?: string | null
): Promise<Response> {
  const headers = new Headers(init.headers ?? {});
  if (init.body && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }
  return makeLocalApiRequest(`${LOCAL_BASE}${path}`, {
    ...init,
    headers,
    hostScope: 'explicit',
    hostId: hostId === undefined ? getCurrentHostId() : hostId,
  });
}

async function parseError(
  response: Response,
  fallback: string
): Promise<Error> {
  try {
    const body = await response.json();
    if (
      response.status === 409 &&
      body?.error_data &&
      typeof body.error_data.workflow_id === 'string' &&
      typeof body.error_data.expected_revision === 'number' &&
      typeof body.error_data.current_revision === 'number'
    ) {
      return new WorkflowRevisionConflictError(body.error_data);
    }
    const message = body?.message || body?.error || fallback;
    return new Error(`${message} (${response.status} ${response.statusText})`);
  } catch {
    return new Error(`${fallback} (${response.status} ${response.statusText})`);
  }
}

async function getJson<T>(
  response: Response,
  errorMessage: string
): Promise<T> {
  if (!response.ok) {
    throw await parseError(response, errorMessage);
  }
  return response.json();
}

async function mutate<T>(response: Response, errorMessage: string): Promise<T> {
  if (!response.ok) {
    throw await parseError(response, errorMessage);
  }
  const wrapped: MutationResponse<T> = await response.json();
  return wrapped.data;
}

export interface ApproveNodeRequest {
  node_execution_id: string;
  message?: string;
}

export interface RejectNodeRequest {
  node_execution_id: string;
  message?: string;
}

export interface SelectArenaWinnerRequest {
  node_execution_id: string;
  candidate_id: string;
}

export interface SelectConditionBranchRequest {
  node_execution_id: string;
  selected_target_node_ids: string[];
  reason?: string | null;
}

export function createWorkflowApi(hostId?: string | null) {
  const localFetch = (path: string, init?: RequestInit) =>
    localRequest(path, init, hostId);
  return {
    async fileContext(
      runId: string,
      nodeExecutionId?: string | null,
      signal?: AbortSignal
    ): Promise<WorkflowFileContextView> {
      const query = new URLSearchParams();
      if (nodeExecutionId) query.set('node_execution_id', nodeExecutionId);
      const suffix = query.size ? `?${query}` : '';
      return handleApiResponse(
        await makeLocalApiRequest(
          `/api/workflow-runs/${encodeURIComponent(runId)}/file-context${suffix}`,
          { signal, hostScope: 'explicit', hostId }
        )
      );
    },

    async fileContent(
      runId: string,
      path: string,
      signal?: AbortSignal
    ): Promise<WorkflowFileContent> {
      const query = new URLSearchParams({ path });
      return handleApiResponse(
        await makeLocalApiRequest(
          `/api/workflow-runs/${encodeURIComponent(runId)}/files/content?${query}`,
          { signal, hostScope: 'explicit', hostId }
        )
      );
    },

    fileRawUrl(runId: string, path: string): string {
      const query = new URLSearchParams({ path });
      const rawUrl = `/api/workflow-runs/${encodeURIComponent(runId)}/files/raw?${query}`;
      return scopeWorkspaceFileRawUrl(rawUrl, hostId ?? null) ?? rawUrl;
    },

    async fileChanges(runId: string): Promise<WorkflowFileChangeSummary> {
      return getJson(
        await localFetch(`/workflow-runs/${runId}/file-changes`),
        'Failed to load workflow file changes'
      );
    },
    async setExternalAccess(
      workflowId: string,
      enabled: boolean
    ): Promise<WorkflowTemplateResponse> {
      return mutate(
        await localFetch(`/workflows/${workflowId}/external-access`, {
          method: 'PUT',
          body: JSON.stringify({ enabled }),
        }),
        'Failed to update external access'
      );
    },

    async list(projectId: string): Promise<WorkflowTemplateListResponse> {
      return getJson(
        await localFetch(`/projects/${projectId}/workflows`),
        'Failed to list workflows'
      );
    },

    async create(
      projectId: string,
      payload: CreateWorkflowRequest
    ): Promise<WorkflowTemplateResponse> {
      return mutate(
        await localFetch(`/projects/${projectId}/workflows`, {
          method: 'POST',
          body: JSON.stringify(payload),
        }),
        'Failed to create workflow'
      );
    },

    async get(workflowId: string): Promise<WorkflowTemplateResponse> {
      return getJson(
        await localFetch(`/workflows/${workflowId}`),
        'Failed to get workflow'
      );
    },

    async update(
      workflowId: string,
      payload: UpdateWorkflowRequest
    ): Promise<WorkflowTemplateResponse> {
      return mutate(
        await localFetch(`/workflows/${workflowId}`, {
          method: 'PUT',
          body: JSON.stringify(payload),
        }),
        'Failed to update workflow'
      );
    },

    async delete(workflowId: string): Promise<void> {
      const response = await localFetch(`/workflows/${workflowId}`, {
        method: 'DELETE',
      });
      if (!response.ok) {
        throw await parseError(response, 'Failed to delete workflow');
      }
      // Consume DeleteResponse payload
      await response.json();
    },

    async trigger(
      workflowId: string,
      payload: TriggerWorkflowRequest
    ): Promise<WorkflowRunResponse> {
      return mutate(
        await localFetch(`/workflows/${workflowId}/trigger`, {
          method: 'POST',
          body: JSON.stringify(payload),
        }),
        'Failed to trigger workflow'
      );
    },

    async listAttempts(
      projectId: string,
      taskId: string
    ): Promise<WorkflowAttemptListResponse> {
      return getJson(
        await localFetch(
          `/projects/${projectId}/tasks/${taskId}/workflow-attempts`
        ),
        'Failed to list workflow attempts'
      );
    },

    async listProjectAttempts(
      projectId: string
    ): Promise<WorkflowAttemptListResponse> {
      return getJson(
        await localFetch(`/projects/${projectId}/workflow-attempts`),
        'Failed to list project workflow attempts'
      );
    },

    async createAttempt(
      projectId: string,
      taskId: string,
      payload: CreateWorkflowAttemptPayload
    ): Promise<WorkflowAttemptResponse> {
      return mutate(
        await localFetch(
          `/projects/${projectId}/tasks/${taskId}/workflow-attempts`,
          {
            method: 'POST',
            body: JSON.stringify(payload),
          }
        ),
        'Failed to create workflow attempt'
      );
    },

    async getAttempt(attemptId: string): Promise<WorkflowAttemptResponse> {
      return getJson(
        await localFetch(`/workflow-attempts/${attemptId}`),
        'Failed to get workflow attempt'
      );
    },

    async deleteAttempt(attemptId: string): Promise<void> {
      const response = await localFetch(`/workflow-attempts/${attemptId}`, {
        method: 'DELETE',
      });
      if (!response.ok) {
        throw await parseError(response, 'Failed to delete workflow attempt');
      }
      await response.json();
    },

    async getAttemptForWorkflow(
      workflowId: string
    ): Promise<WorkflowAttemptResponse | null> {
      return getJson(
        await localFetch(`/workflows/${workflowId}/attempt`),
        'Failed to get workflow attempt'
      );
    },

    async runAttempt(
      attemptId: string,
      payload: RunWorkflowAttemptPayload
    ): Promise<WorkflowRunResponse> {
      return mutate(
        await localFetch(`/workflow-attempts/${attemptId}/run`, {
          method: 'POST',
          body: JSON.stringify(payload),
        }),
        'Failed to run workflow attempt'
      );
    },

    async getRun(runId: string): Promise<WorkflowRunResponse> {
      return getJson(
        await localFetch(`/workflow-runs/${runId}`),
        'Failed to get workflow run'
      );
    },

    async cancelRun(runId: string): Promise<WorkflowActionResponse> {
      return mutate(
        await localFetch(`/workflow-runs/${runId}/cancel`, {
          method: 'POST',
        }),
        'Failed to cancel workflow run'
      );
    },

    async approve(
      runId: string,
      nodeId: string,
      payload: ApproveNodeRequest
    ): Promise<WorkflowActionResponse> {
      return mutate(
        await localFetch(`/workflow-runs/${runId}/nodes/${nodeId}/approve`, {
          method: 'POST',
          body: JSON.stringify(payload),
        }),
        'Failed to approve node'
      );
    },

    async reject(
      runId: string,
      nodeId: string,
      payload: RejectNodeRequest
    ): Promise<WorkflowActionResponse> {
      return mutate(
        await localFetch(`/workflow-runs/${runId}/nodes/${nodeId}/reject`, {
          method: 'POST',
          body: JSON.stringify(payload),
        }),
        'Failed to reject node'
      );
    },

    async retry(
      runId: string,
      nodeId: string
    ): Promise<WorkflowActionResponse> {
      return mutate(
        await localFetch(`/workflow-runs/${runId}/nodes/${nodeId}/retry`, {
          method: 'POST',
        }),
        'Failed to retry node'
      );
    },

    async selectArenaWinner(
      runId: string,
      nodeId: string,
      payload: SelectArenaWinnerRequest
    ): Promise<WorkflowActionResponse> {
      return mutate(
        await localFetch(
          `/workflow-runs/${runId}/nodes/${nodeId}/arena-winner`,
          {
            method: 'POST',
            body: JSON.stringify(payload),
          }
        ),
        'Failed to select arena winner'
      );
    },

    async selectConditionBranch(
      runId: string,
      nodeId: string,
      payload: SelectConditionBranchRequest
    ): Promise<WorkflowActionResponse> {
      return mutate(
        await localFetch(
          `/workflow-runs/${runId}/nodes/${nodeId}/condition-branch`,
          {
            method: 'POST',
            body: JSON.stringify(payload),
          }
        ),
        'Failed to select condition branch'
      );
    },

    eventsUrl(runId: string): string {
      return `${LOCAL_BASE}/workflow-runs/${runId}/events`;
    },
  };
}

export const workflowApi = createWorkflowApi();
