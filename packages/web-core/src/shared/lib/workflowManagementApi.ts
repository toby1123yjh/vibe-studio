import type {
  PrepareWorkflowMainSessionRequest,
  WorkflowContextView,
  WorkflowMainSessionView,
  WorkflowNotificationPage,
  WorkflowNotificationView,
  WorkflowSubmission,
  AcceptedWorkflowSubmission,
} from 'shared/types';
import { handleApiResponse } from '@/shared/lib/api';
import { makeLocalApiRequest } from './localApiTransport';

const BASE = '/api/workflow-management';

export const workflowManagementApi = {
  async submit(
    instanceId: string,
    payload: WorkflowSubmission,
    hostId?: string | null
  ): Promise<AcceptedWorkflowSubmission> {
    return handleApiResponse(
      await makeLocalApiRequest(
        `${BASE}/instances/${encodeURIComponent(instanceId)}/submit`,
        {
          method: 'POST',
          hostScope: 'explicit',
          hostId,
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload),
        }
      )
    );
  },

  async prepareMainSession(
    payload: PrepareWorkflowMainSessionRequest,
    hostId?: string | null
  ): Promise<WorkflowMainSessionView> {
    return handleApiResponse(
      await makeLocalApiRequest(`${BASE}/prepare-main-session`, {
        method: 'POST',
        hostScope: 'explicit',
        hostId,
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      })
    );
  },

  async context(
    sessionId: string,
    signal?: AbortSignal,
    hostId?: string | null
  ): Promise<WorkflowContextView | null> {
    return handleApiResponse(
      await makeLocalApiRequest(
        `${BASE}/sessions/${encodeURIComponent(sessionId)}/context`,
        { signal, hostScope: 'explicit', hostId }
      )
    );
  },

  async notifications(
    sessionId: string,
    signal?: AbortSignal,
    hostId?: string | null
  ): Promise<WorkflowNotificationView[]> {
    const notifications = new Map<string, WorkflowNotificationView>();
    let cursor: WorkflowNotificationPage['next_cursor'] = null;
    const visitedCursors = new Set<string>();
    do {
      const query = new URLSearchParams({ limit: '50' });
      if (cursor !== null) query.set('cursor', String(cursor));
      const page = await handleApiResponse<WorkflowNotificationPage>(
        await makeLocalApiRequest(
          `${BASE}/sessions/${encodeURIComponent(sessionId)}/notifications?${query}`,
          { signal, hostScope: 'explicit', hostId }
        )
      );
      for (const notification of page.notifications) {
        notifications.set(notification.id, notification);
      }
      cursor = page.next_cursor;
      if (cursor !== null) {
        const key = String(cursor);
        if (visitedCursors.has(key)) {
          throw new Error('Workflow notification pagination did not advance');
        }
        visitedCursors.add(key);
      }
    } while (cursor !== null);
    return [...notifications.values()];
  },
};
