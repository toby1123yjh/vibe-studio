import { useCallback, useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import {
  BaseCodingAgent,
  type ExecutorConfig,
  type ExecutorProfile,
  type ExecutorProfileId,
} from 'shared/types';
import { sessionsApi } from '@/shared/lib/api';
import { restoreExecutorConfig } from '@/shared/lib/executorConfig';
import { useExecutorConfig } from '@/shared/hooks/useExecutorConfig';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { sessionExecutorConfigKey } from '@/shared/hooks/sessionExecutorConfigKeys';
import { useWorkflowMainSessionContext } from '@/shared/hooks/useWorkflowManagement';

interface UseSessionExecutorConfigOptions {
  sessionId?: string;
  workspaceId?: string;
  sessionExecutor?: string | null;
  isNewSessionMode: boolean;
  preferredExecutorConfig?: ExecutorConfig | null;
  scratchConfig?: ExecutorConfig | null;
  isScratchLoading?: boolean;
  profiles: Record<string, ExecutorProfile> | null;
  configExecutorProfile?: ExecutorProfileId | null;
  hiddenAgents?: readonly BaseCodingAgent[] | null;
  onPersist?: (config: ExecutorConfig) => void;
}

/** Restore only this session's durable request; drafts never own its binding. */
export function useSessionExecutorConfig({
  sessionId,
  workspaceId,
  sessionExecutor,
  isNewSessionMode,
  preferredExecutorConfig,
  scratchConfig,
  isScratchLoading = false,
  ...options
}: UseSessionExecutorConfigOptions) {
  const hostId = useHostId();
  const existingSession = !isNewSessionMode && !!sessionId;
  const workflowContext = useWorkflowMainSessionContext(
    existingSession ? sessionId : undefined
  );
  const capturedMainConfig = useMemo(
    () =>
      workflowContext.data
        ? restoreExecutorConfig(workflowContext.data.main_agent_config)
        : null,
    [workflowContext.data]
  );
  const query = useQuery({
    queryKey: sessionExecutorConfigKey(hostId, sessionId),
    queryFn: () => sessionsApi.getExecutorConfig(sessionId!, hostId),
    enabled: existingSession,
    retry: false,
    // Reopening must observe a run sent by another mounted view/tab as well.
    staleTime: 0,
  });
  const restored = useMemo(
    () =>
      capturedMainConfig ??
      (query.data ? restoreExecutorConfig(query.data) : null),
    [capturedMainConfig, query.data]
  );
  const boundExecutor = Object.values(BaseCodingAgent).find(
    (agent) => agent === sessionExecutor
  );
  const configError =
    (existingSession ? workflowContext.error : null) ??
    (existingSession ? query.error : null) ??
    (existingSession && sessionExecutor && !boundExecutor
      ? new Error(`Unknown session executor: ${sessionExecutor}`)
      : null) ??
    (existingSession &&
    restored &&
    boundExecutor &&
    restored.executor !== boundExecutor
      ? new Error('Stored executor configuration does not match this session')
      : null);
  const isConfigInitialized =
    !isScratchLoading &&
    (!existingSession || (!query.isPending && !workflowContext.isPending));
  const isConfigLoading =
    !isConfigInitialized || (existingSession && query.isFetching);
  const isConfigReady = !isConfigLoading && !configError;
  const selection = useExecutorConfig({
    ...options,
    scopeKey: JSON.stringify([
      hostId,
      sessionId ?? workspaceId,
      isNewSessionMode,
    ]),
    enabled: isConfigReady && !capturedMainConfig,
    lockedExecutor: existingSession ? boundExecutor : null,
    lockedConfig: existingSession ? restored : null,
    lastUsedConfig: existingSession
      ? (restored ??
        (!boundExecutor ? (preferredExecutorConfig ?? null) : null))
      : (preferredExecutorConfig ?? null),
    // Embedded workflow defaults are initial values, never session overrides.
    // Bound-session drafts are complete snapshots. Rust omits null fields on
    // serialization, so restore those as Follow CLI instead of old run values.
    scratchConfig: capturedMainConfig
      ? null
      : isScratchLoading
        ? undefined
        : existingSession && restored && scratchConfig
          ? restoreExecutorConfig(scratchConfig)
          : scratchConfig,
  });
  const refetchSessionConfig = query.refetch;
  const refetchWorkflowContext = workflowContext.refetch;
  const refetchConfig = useCallback(async () => {
    await Promise.all([
      refetchSessionConfig(),
      ...(existingSession ? [refetchWorkflowContext()] : []),
    ]);
  }, [refetchSessionConfig, refetchWorkflowContext, existingSession]);
  return {
    ...selection,
    effectiveExecutor:
      capturedMainConfig?.executor ?? selection.effectiveExecutor,
    selectedVariant: capturedMainConfig
      ? (capturedMainConfig.variant ?? null)
      : selection.selectedVariant,
    presetOptions: capturedMainConfig ?? selection.presetOptions,
    configurationLocked: !!capturedMainConfig,
    // ModelSelector already renders an empty options list as a disabled current preset.
    variantOptions: existingSession && restored ? [] : selection.variantOptions,
    executorConfig: isConfigReady
      ? (capturedMainConfig ?? selection.executorConfig)
      : null,
    // Cached data remains visible during a background refresh. Only the
    // submission config above may authorize a send or configuration edit.
    displayExecutorConfig:
      isConfigInitialized && !configError
        ? (capturedMainConfig ?? selection.executorConfig)
        : null,
    isConfigLoading,
    configError,
    refetchConfig,
    needsExecutorSelection: isNewSessionMode || (!boundExecutor && !restored),
  };
}
