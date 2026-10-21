import type {
  AgentEventCursor,
  AgentRunSummary,
  ConversationInteractionReply,
  ReplyAgentRunInteractionRequest,
  CancelAgentRunRequest,
  ResolveAgentRunApprovalRequest,
  RetryAgentRunRequest,
  RunAttemptMode,
  RunState,
  SubmitAgentRunInputRequest,
} from 'shared/types';
import { handleApiResponse } from '@/shared/lib/api';
import { makeLocalApiRequest } from '@/shared/lib/localApiTransport';

export interface AgentRunHistoryPage {
  agent_run_id: string;
  state: RunState;
  events: import('shared/types').AgentEventEnvelope[];
  next_cursor: AgentEventCursor | null;
  has_more: boolean;
}

export interface AgentRunStats {
  event_count: number;
  message_count: number;
  thinking_count: number;
  tool_call_count: number;
  approval_request_count: number;
  approval_resolution_count: number;
  input_request_count: number;
  input_resolution_count: number;
  error_count: number;
  provider_extension_count: number;
  unknown_event_count: number;
  input_tokens: number | bigint;
  output_tokens: number | bigint;
  cached_input_tokens: number | bigint;
  first_event_at: string | null;
  last_event_at: string | null;
  status: RunState['status'];
  projection_status: RunState['projection_status'];
}

function cursorParams(cursor: AgentEventCursor | null | undefined): string {
  if (!cursor) return '';
  const params = new URLSearchParams();
  params.set('after_attempt_number', String(cursor.run_attempt_number));
  params.set('after_sequence', String(cursor.sequence));
  return `?${params.toString()}`;
}

function controlIdentity(operation: string, targetId: string) {
  const commandId = crypto.randomUUID();
  return {
    command_id: commandId,
    idempotency_key: `web:${operation}:${targetId}:${commandId}`,
    correlation_id: crypto.randomUUID(),
    created_at: new Date().toISOString(),
  };
}

/** Capture once per decision. Retrying a confirmed failure uses the exact
 * same identity; uncertain delivery is refreshed, never automatically resent. */
export function createAgentInteractionReply(
  runAttemptId: string,
  reply: ConversationInteractionReply
): ReplyAgentRunInteractionRequest {
  const replyId = crypto.randomUUID();
  return {
    command_id: replyId,
    reply_id: replyId,
    run_attempt_id: runAttemptId,
    idempotency_key: `web:interaction:${runAttemptId}:${reply.request_id}:${replyId}`,
    correlation_id: crypto.randomUUID(),
    created_at: new Date().toISOString(),
    reply,
  };
}

async function postControl<TBody>(
  agentRunId: string,
  operation: string,
  body: TBody
): Promise<RunState> {
  return handleApiResponse<RunState>(
    await makeLocalApiRequest(`/api/agent-runs/${agentRunId}/${operation}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
  );
}

export const agentRunsApi = {
  async replyInteraction(
    agentRunId: string,
    request: ReplyAgentRunInteractionRequest,
    hostId: string | null
  ): Promise<RunState> {
    return handleApiResponse<RunState>(
      await makeLocalApiRequest(
        `/api/agent-runs/${encodeURIComponent(agentRunId)}/interaction`,
        {
          hostScope: 'explicit',
          hostId,
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(request),
        }
      )
    );
  },
  async listForSession(
    sessionId: string,
    hostId?: string | null
  ): Promise<AgentRunSummary[]> {
    return handleApiResponse<AgentRunSummary[]>(
      await makeLocalApiRequest(
        `/api/agent-runs/session/${sessionId}`,
        hostId === undefined ? undefined : { hostScope: 'explicit', hostId }
      )
    );
  },

  async getState(agentRunId: string): Promise<RunState> {
    return handleApiResponse<RunState>(
      await makeLocalApiRequest(`/api/agent-runs/${agentRunId}`)
    );
  },

  async getHistory(
    agentRunId: string,
    cursor?: AgentEventCursor | null,
    limit = 500
  ): Promise<AgentRunHistoryPage> {
    const suffix = cursorParams(cursor);
    const separator = suffix ? '&' : '?';
    return handleApiResponse<AgentRunHistoryPage>(
      await makeLocalApiRequest(
        `/api/agent-runs/${agentRunId}/events${suffix}${separator}limit=${Math.min(1000, Math.max(1, limit))}`
      )
    );
  },

  async getStats(agentRunId: string): Promise<AgentRunStats> {
    return handleApiResponse<AgentRunStats>(
      await makeLocalApiRequest(`/api/agent-runs/${agentRunId}/stats`)
    );
  },

  async cancel(agentRunId: string, reason: string): Promise<RunState> {
    const request: CancelAgentRunRequest = {
      ...controlIdentity('cancel', agentRunId),
      reason,
    };
    return postControl(agentRunId, 'cancel', request);
  },

  async submitInput(
    agentRunId: string,
    inputId: string,
    content: string
  ): Promise<RunState> {
    const request: SubmitAgentRunInputRequest = {
      ...controlIdentity('input', `${agentRunId}:${inputId}`),
      input_id: inputId,
      content,
    };
    return postControl(agentRunId, 'input', request);
  },

  async resolveApproval(
    agentRunId: string,
    approvalId: string,
    approved: boolean,
    reason?: string
  ): Promise<RunState> {
    const request: ResolveAgentRunApprovalRequest = {
      ...controlIdentity('approval', `${agentRunId}:${approvalId}`),
      approval_id: approvalId,
      approved,
      ...(reason ? { reason } : {}),
    };
    return postControl(agentRunId, 'approval', request);
  },

  async retry(
    agentRunId: string,
    runAttemptId: string,
    mode: RunAttemptMode
  ): Promise<RunState> {
    const request: RetryAgentRunRequest = {
      ...controlIdentity('retry', `${agentRunId}:${runAttemptId}`),
      run_attempt_id: runAttemptId,
      mode,
    };
    return postControl(agentRunId, 'retry', request);
  },
};
