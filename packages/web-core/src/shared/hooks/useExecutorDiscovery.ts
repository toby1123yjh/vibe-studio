import { useCallback, useEffect, useMemo, useState } from 'react';
import { useHostId } from '@/shared/providers/HostIdProvider';
import type { BaseCodingAgent, ExecutorDiscoveredOptions } from 'shared/types';
import { useJsonPatchWsStream } from '@/shared/hooks/useJsonPatchWsStream';
import { agentsApi } from '@/shared/lib/api';
import type { ExecutorDiscoveryContext } from '@/shared/lib/executorDiscovery';

type ExecutorDiscoveryStreamState = {
  options: ExecutorDiscoveredOptions | null;
};

const defaultOptions: ExecutorDiscoveredOptions = {
  model_selector: {
    providers: [],
    models: [],
    default_model: null,
    agents: [],
    permissions: [],
  },
  slash_commands: [],
  skills: [],
  skill_errors: [],
  loading_models: true,
  loading_agents: true,
  loading_slash_commands: true,
  loading_skills: true,
  error: null,
};

function useExecutorDiscovery(
  agent: BaseCodingAgent | null | undefined,
  opts?: ExecutorDiscoveryContext
) {
  const { workspaceId, sessionId, repoId, variant } = opts ?? {};
  const hostId = useHostId();
  const [refreshRequest, setRefreshRequest] = useState<{
    scope: string;
    nonce: number;
  } | null>(null);
  const scope = JSON.stringify([
    agent,
    hostId,
    workspaceId,
    sessionId,
    repoId,
    variant,
  ]);
  const refreshNonce =
    refreshRequest?.scope === scope ? refreshRequest.nonce : 0;
  const refresh = useCallback(
    () =>
      setRefreshRequest((previous) => ({
        scope,
        nonce: (previous?.scope === scope ? previous.nonce : 0) + 1,
      })),
    [scope]
  );
  const endpoint = useMemo(() => {
    if (!agent) return undefined;
    const url = agentsApi.getDiscoveredOptionsStreamUrl(agent, {
      workspaceId,
      sessionId,
      repoId,
      variant,
      refresh: refreshNonce > 0,
    });
    return refreshNonce ? `${url}&refresh_nonce=${refreshNonce}` : url;
  }, [agent, workspaceId, sessionId, repoId, variant, refreshNonce]);

  const initialData = useCallback(
    (): ExecutorDiscoveryStreamState => ({
      options: { ...defaultOptions },
    }),
    []
  );

  const { data, error, isConnected, isInitialized } =
    useJsonPatchWsStream<ExecutorDiscoveryStreamState>(
      endpoint,
      !!endpoint,
      initialData,
      { transport: { hostScope: 'explicit', hostId } }
    );

  // Prefer the backend-reported error from the data payload. Only fall back
  // to the WebSocket transport error when no data has been received yet —
  // transient connection failures (e.g. React StrictMode double-mount or
  // Safari/macOS 26 WebSocket instability) should not persist once data
  // has successfully loaded.
  const combinedError = data?.options?.error ?? (isInitialized ? null : error);

  useEffect(() => {
    if (combinedError) {
      console.error(
        'Failed to fetch executor discovery options',
        combinedError
      );
    }
  }, [combinedError]);

  return {
    options: data?.options ?? null,
    error: combinedError,
    isConnected,
    isInitialized,
    refresh,
  };
}

export function useModelSelectorConfig(
  agent: BaseCodingAgent | null | undefined,
  opts?: ExecutorDiscoveryContext
) {
  const { options, error, isConnected, isInitialized, refresh } =
    useExecutorDiscovery(agent, opts);

  return {
    config: options?.model_selector ?? null,
    loadingModels: options?.loading_models ?? false,
    loadingAgents: options?.loading_agents ?? false,
    refresh,
    error,
    isConnected,
    isInitialized,
  };
}

export function useSlashCommands(
  agent: BaseCodingAgent | null | undefined,
  opts?: ExecutorDiscoveryContext
) {
  const { options, error, isConnected, isInitialized } = useExecutorDiscovery(
    agent,
    opts
  );

  return {
    commands: options?.slash_commands ?? [],
    discovering: options?.loading_slash_commands ?? false,
    error,
    isConnected,
    isInitialized,
  };
}

export function useExecutorTooling(
  agent: BaseCodingAgent | null | undefined,
  opts?: ExecutorDiscoveryContext
) {
  const { options, error, isConnected, isInitialized } = useExecutorDiscovery(
    agent,
    opts
  );

  return {
    commands: options?.slash_commands ?? [],
    skills: options?.skills ?? [],
    skillErrors: options?.skill_errors ?? [],
    discoveringCommands: options?.loading_slash_commands ?? false,
    discoveringSkills: options?.loading_skills ?? false,
    error,
    isConnected,
    isInitialized,
  };
}
