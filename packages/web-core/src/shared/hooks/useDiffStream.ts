import { useCallback, useMemo } from 'react';
import type { Diff, PatchType } from 'shared/types';
import { useJsonPatchWsStream } from '@/shared/hooks/useJsonPatchWsStream';
import { useWorkspaceRepo } from '@/shared/hooks/useWorkspaceRepo';
import { useHostId } from '@/shared/providers/HostIdProvider';

interface RepoDiffEntries {
  [filePath: string]: PatchType;
}

interface DiffEntries {
  [repoName: string]: RepoDiffEntries;
}

type DiffStreamEvent = {
  entries: DiffEntries;
};

export interface UseDiffStreamOptions {
  statsOnly?: boolean;
}

interface UseDiffStreamResult {
  diffs: Diff[];
  error: string | null;
  isInitialized: boolean;
}

export const useDiffStream = (
  workspaceId: string | null,
  enabled: boolean,
  options?: UseDiffStreamOptions
): UseDiffStreamResult => {
  const hostId = useHostId();
  const isEnabled = enabled && !!workspaceId;
  const {
    repos,
    isSuccess: areReposLoaded,
    error: reposError,
  } = useWorkspaceRepo(workspaceId ?? undefined, { enabled: isEnabled });
  // A directory without repositories has no Git diff stream to subscribe to.
  const streamEnabled = isEnabled && areReposLoaded && repos.length > 0;
  const endpoint = (() => {
    if (!workspaceId || !streamEnabled) return undefined;
    const apiBasePath = hostId ? `/api/host/${hostId}` : '/api';
    const query = `${apiBasePath}/workspaces/${workspaceId}/git/diff/ws`;
    if (typeof options?.statsOnly === 'boolean') {
      const params = new URLSearchParams();
      params.set('stats_only', String(options.statsOnly));
      return `${query}?${params.toString()}`;
    } else {
      return query;
    }
  })();

  const initialData = useCallback(
    (): DiffStreamEvent => ({
      entries: {},
    }),
    []
  );

  const {
    data,
    error: streamError,
    isInitialized,
  } = useJsonPatchWsStream<DiffStreamEvent>(
    endpoint,
    streamEnabled,
    initialData,
    { transport: { hostScope: 'explicit', hostId } }
  );

  const diffs = useMemo(() => {
    if (!streamEnabled) return [];
    return Object.values(data?.entries ?? {})
      .flatMap((repoEntries) => Object.values(repoEntries ?? {}))
      .filter(
        (entry): entry is Extract<PatchType, { type: 'DIFF' }> =>
          entry?.type === 'DIFF'
      )
      .map((entry) => entry.content);
  }, [data?.entries, streamEnabled]);

  return {
    diffs,
    error: isEnabled
      ? (reposError?.message ?? (streamEnabled ? streamError : null))
      : null,
    isInitialized:
      isEnabled && areReposLoaded && (repos.length === 0 || isInitialized),
  };
};
