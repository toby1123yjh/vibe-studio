import type {
  DashboardChildrenPage,
  DashboardContext,
  DashboardExpandedPage,
  DashboardPageResponse,
  DashboardQuery,
  DashboardWorkRef,
} from 'shared/types';
import { makeLocalApiRequest } from './localApiTransport';
import { handleApiResponse } from './api';

/** Capture Host once; all requests and aborts stay attached to that scope. */
export function createDashboardApi(hostId: string | null) {
  async function read<T>(path: string, init: RequestInit = {}): Promise<T> {
    const response = await makeLocalApiRequest(`/api/dashboard${path}`, {
      ...init,
      headers: { 'Content-Type': 'application/json', ...init.headers },
      hostScope: 'explicit',
      hostId,
    });
    return handleApiResponse<T>(response);
  }
  const workPath = (work: DashboardWorkRef) =>
    `/works/${encodeURIComponent(work.kind)}/${encodeURIComponent(work.id)}`;
  return {
    query: (query: DashboardQuery, signal: AbortSignal) =>
      read<DashboardPageResponse>('/query', {
        method: 'POST',
        body: JSON.stringify(query),
        signal,
      }),
    children: (
      page: DashboardExpandedPage,
      rootSnapshotId: string | null,
      signal: AbortSignal
    ) => {
      const params = new URLSearchParams({ page: String(page.page) });
      if (page.workflow_run_id)
        params.set('workflow_run_id', page.workflow_run_id);
      if (page.snapshot_id) params.set('snapshot_id', page.snapshot_id);
      if (rootSnapshotId) params.set('root_snapshot_id', rootSnapshotId);
      return read<DashboardChildrenPage>(
        `${workPath(page.work)}/children?${params}`,
        { signal }
      );
    },
    context: (
      work: DashboardWorkRef,
      options: { sessionId?: string; agentRunId?: string; refresh?: boolean },
      signal: AbortSignal
    ) => {
      const params = new URLSearchParams();
      if (options.sessionId) params.set('session_id', options.sessionId);
      if (options.agentRunId) params.set('agent_run_id', options.agentRunId);
      if (options.refresh) params.set('refresh', 'true');
      return read<DashboardContext>(`${workPath(work)}/context?${params}`, {
        signal,
      });
    },
  };
}
