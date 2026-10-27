import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useJsonPatchWsStream } from '@/shared/hooks/useJsonPatchWsStream';
import { workspaceSummaryKeys } from '@/shared/hooks/workspaceSummaryKeys';
import { makeLocalApiRequest } from '@/shared/lib/localApiTransport';
import { useHostId } from '@/shared/providers/HostIdProvider';
import {
  deriveWorkspaceListState,
  type WorkspaceListState,
} from '@/shared/lib/workspaceListState';
import type {
  AgentRunStatus,
  WorkspaceWithStatus,
  WorkspaceSummary,
  WorkspaceSummaryResponse,
  ApiResponse,
} from 'shared/types';

// UI-specific workspace type for sidebar display
export interface SidebarWorkspace {
  id: string;
  name: string;
  branch: string;
  createdAt: string;
  updatedAt: string;
  description: string;
  filesChanged?: number;
  linesAdded?: number;
  linesRemoved?: number;
  isRunning?: boolean;
  isPinned?: boolean;
  isArchived?: boolean;
  hasPendingApproval?: boolean;
  hasRunningDevServer?: boolean;
  hasUnseenActivity?: boolean;
  latestProcessCompletedAt?: string;
  latestProcessStatus?: AgentRunStatus;
  prStatus?: 'open' | 'merged' | 'closed' | 'unknown';
  prNumber?: number;
  prUrl?: string;
}

// Keep the old export name for backwards compatibility
export type Workspace = SidebarWorkspace;

export interface UseWorkspacesResult {
  workspaces: SidebarWorkspace[];
  archivedWorkspaces: SidebarWorkspace[];
  state: WorkspaceListState;
  isLoading: boolean;
  isConnected: boolean;
  isRetrying: boolean;
  error: unknown;
  retry: () => Promise<void>;
}

// State shape from the WebSocket stream
type WorkspacesState = {
  workspaces: Record<string, WorkspaceWithStatus>;
};

const EMPTY_WORKSPACE_SUMMARIES = new Map<string, WorkspaceSummary>();

function indexWorkspaceSummaries(summaries: WorkspaceSummary[]) {
  return new Map(summaries.map((summary) => [summary.workspace_id, summary]));
}

// Transform WorkspaceWithStatus to SidebarWorkspace, optionally merging summary data
function toSidebarWorkspace(
  ws: WorkspaceWithStatus,
  summary?: WorkspaceSummary
): SidebarWorkspace {
  return {
    id: ws.id,
    name: ws.name ?? ws.branch, // Use name if available, fallback to branch
    branch: ws.branch,
    createdAt: ws.created_at,
    updatedAt: ws.updated_at,
    description: '',
    // Use real stats from summary if available
    filesChanged: summary?.files_changed ?? undefined,
    linesAdded: summary?.lines_added ?? undefined,
    linesRemoved: summary?.lines_removed ?? undefined,
    // Real data from stream
    isRunning: ws.is_running,
    isPinned: ws.pinned,
    isArchived: ws.archived,
    // Additional data from summary
    hasPendingApproval: summary?.has_pending_approval,
    hasRunningDevServer: summary?.has_running_dev_server,
    hasUnseenActivity: summary?.has_unseen_turns,
    latestProcessCompletedAt: summary?.latest_process_completed_at ?? undefined,
    latestProcessStatus: summary?.latest_process_status ?? undefined,
    prStatus: summary?.pr_status ?? undefined,
    prNumber:
      summary?.pr_number != null ? Number(summary.pr_number) : undefined,
    prUrl: summary?.pr_url ?? undefined,
  };
}

export const workspaceKeys = {
  all: ['workspaces'] as const,
};

// workspaceSummaryKeys is imported from @/shared/hooks/workspaceSummaryKeys

// Fetch workspace summaries from the API by archived status
async function fetchWorkspaceSummariesByArchived(
  archived: boolean,
  hostId: string | null
): Promise<WorkspaceSummary[]> {
  const response = await makeLocalApiRequest('/api/workspaces/summaries', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ archived }),
    hostScope: 'explicit',
    hostId,
  });

  if (!response.ok) {
    throw new Error(`Workspace summaries request failed (${response.status})`);
  }

  const data: ApiResponse<WorkspaceSummaryResponse> = await response.json();
  if (!data.success || !data.data?.summaries) {
    throw new Error('Workspace summaries response was invalid');
  }

  // Keep query data JSON-compatible so unchanged polls retain their identity.
  // A Map in the cache bypasses React Query's structural sharing and rebuilds
  // the global workspace context even when every summary is unchanged.
  return data.data.summaries;
}

export function useWorkspaces(): UseWorkspacesResult {
  const hostId = useHostId();
  const retryLockRef = useRef(false);
  const retryEpochRef = useRef(0);
  const [isSummaryRetrying, setIsSummaryRetrying] = useState(false);

  // Two separate WebSocket connections: one for active, one for archived
  // No limit param - we fetch all and slice on frontend so backfill works when archiving
  const activeEndpoint = '/api/workspaces/streams/ws?archived=false';
  const archivedEndpoint = '/api/workspaces/streams/ws?archived=true';

  const initialData = useCallback(
    (): WorkspacesState => ({ workspaces: {} }),
    []
  );

  const activeStream = useJsonPatchWsStream<WorkspacesState>(
    activeEndpoint,
    true,
    initialData,
    { transport: { hostScope: 'explicit', hostId } }
  );
  const {
    data: activeData,
    isConnected: activeIsConnected,
    isInitialized: activeIsInitialized,
    isRetrying: activeIsRetrying,
    error: activeError,
    retry: retryActiveStream,
  } = activeStream;

  const archivedStream = useJsonPatchWsStream<WorkspacesState>(
    archivedEndpoint,
    true,
    initialData,
    { transport: { hostScope: 'explicit', hostId } }
  );
  const {
    data: archivedData,
    isConnected: archivedIsConnected,
    isInitialized: archivedIsInitialized,
    isRetrying: archivedIsRetrying,
    error: archivedError,
    retry: retryArchivedStream,
  } = archivedStream;

  // Wait for both streams to be initialized before fetching summaries
  // Fetch summaries for active workspaces
  const {
    data: activeSummaries = EMPTY_WORKSPACE_SUMMARIES,
    error: activeSummaryError,
    refetch: refetchActiveSummaries,
  } = useQuery({
    queryKey: workspaceSummaryKeys.byArchived(false, hostId),
    queryFn: () => fetchWorkspaceSummariesByArchived(false, hostId),
    select: indexWorkspaceSummaries,
    enabled: activeIsInitialized,
    staleTime: 1000,
    refetchInterval: 15000,
    refetchOnWindowFocus: false,
    refetchOnMount: 'always',
  });

  // Fetch summaries for archived workspaces
  const {
    data: archivedSummaries = EMPTY_WORKSPACE_SUMMARIES,
    error: archivedSummaryError,
    refetch: refetchArchivedSummaries,
  } = useQuery({
    queryKey: workspaceSummaryKeys.byArchived(true, hostId),
    queryFn: () => fetchWorkspaceSummariesByArchived(true, hostId),
    select: indexWorkspaceSummaries,
    enabled: archivedIsInitialized,
    staleTime: 1000,
    refetchInterval: 15000,
    refetchOnWindowFocus: false,
    refetchOnMount: 'always',
  });

  const workspaces = useMemo(() => {
    if (!activeData?.workspaces) return [];
    return Object.values(activeData.workspaces)
      .sort((a, b) => {
        // First sort by pinned (pinned first)
        if (a.pinned !== b.pinned) {
          return a.pinned ? -1 : 1;
        }
        // Then by created_at (newest first)
        return (
          new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
        );
      })
      .map((ws) => toSidebarWorkspace(ws, activeSummaries.get(ws.id)));
  }, [activeData, activeSummaries]);

  const archivedWorkspaces = useMemo(() => {
    if (!archivedData?.workspaces) return [];
    return Object.values(archivedData.workspaces)
      .sort((a, b) => {
        // First sort by pinned (pinned first)
        if (a.pinned !== b.pinned) {
          return a.pinned ? -1 : 1;
        }
        // Then by created_at (newest first)
        return (
          new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
        );
      })
      .map((ws) => toSidebarWorkspace(ws, archivedSummaries.get(ws.id)));
  }, [archivedData, archivedSummaries]);

  const isConnected = activeIsConnected && archivedIsConnected;
  const summaryError = activeSummaryError ?? archivedSummaryError;
  const error = activeError ?? archivedError ?? summaryError;
  const state = deriveWorkspaceListState({
    activeInitialized: activeIsInitialized,
    archivedInitialized: archivedIsInitialized,
    activeConnected: activeIsConnected,
    archivedConnected: archivedIsConnected,
    activeError,
    archivedError,
    summaryError,
    itemCount: workspaces.length + archivedWorkspaces.length,
  });

  useEffect(() => {
    retryEpochRef.current += 1;
    retryLockRef.current = false;
    setIsSummaryRetrying(false);
  }, [hostId]);

  const retry = useCallback(async () => {
    if (retryLockRef.current) return;
    retryLockRef.current = true;
    const epoch = retryEpochRef.current;
    setIsSummaryRetrying(true);
    retryActiveStream();
    retryArchivedStream();
    try {
      await Promise.allSettled([
        ...(activeIsInitialized ? [refetchActiveSummaries()] : []),
        ...(archivedIsInitialized ? [refetchArchivedSummaries()] : []),
      ]);
    } finally {
      if (retryEpochRef.current === epoch) {
        retryLockRef.current = false;
        setIsSummaryRetrying(false);
      }
    }
  }, [
    activeIsInitialized,
    archivedIsInitialized,
    refetchActiveSummaries,
    refetchArchivedSummaries,
    retryActiveStream,
    retryArchivedStream,
  ]);
  const isRetrying =
    isSummaryRetrying || activeIsRetrying || archivedIsRetrying;

  return {
    workspaces,
    archivedWorkspaces,
    state,
    isLoading: state === 'loading',
    isConnected,
    isRetrying,
    error,
    retry,
  };
}
