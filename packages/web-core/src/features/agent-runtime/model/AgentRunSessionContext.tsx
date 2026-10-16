import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { useQuery } from '@tanstack/react-query';

import { agentRunsApi } from '@/shared/lib/agentRunApi';
import {
  buildCanonicalAgentSessionTimeline,
  type CanonicalAgentSessionTimeline,
} from './canonicalAgentSessionTimeline';
import type { CanonicalAgentTimeline } from './canonicalAgentTimeline';
import { projectCanonicalAgentConversation } from './canonicalAgentConversation';
import { useAgentRunCanonicalStream } from './useAgentRunCanonicalStream';
import type { CanonicalConversationProjection } from '@/shared/hooks/useConversationHistory/types';
import { useHostId } from '@/shared/providers/HostIdProvider';

const AGENT_RUN_DISCOVERY_INTERVAL_MS = 2_000;

export interface AgentRunSessionContextValue {
  readonly timeline: CanonicalAgentSessionTimeline | null;
  readonly conversation: CanonicalConversationProjection;
  readonly isLoading: boolean;
  readonly isConnected: boolean;
  readonly error: string | null;
}

const AgentRunSessionContext =
  createContext<AgentRunSessionContextValue | null>(null);

function AgentRunStreamBridge({
  agentRunId,
  onTimeline,
  onConnection,
}: {
  agentRunId: string;
  onTimeline: (agentRunId: string, timeline: CanonicalAgentTimeline) => void;
  onConnection: (
    agentRunId: string,
    connected: boolean,
    initialized: boolean,
    error: string | null
  ) => void;
}) {
  const stream = useAgentRunCanonicalStream(agentRunId);

  useEffect(() => {
    if (stream.timeline) onTimeline(agentRunId, stream.timeline);
  }, [agentRunId, onTimeline, stream.timeline]);

  useEffect(() => {
    onConnection(
      agentRunId,
      stream.isConnected,
      stream.isInitialized,
      stream.error
    );
  }, [
    agentRunId,
    onConnection,
    stream.error,
    stream.isConnected,
    stream.isInitialized,
  ]);

  return null;
}

function ScopedAgentRunSessionProvider({
  sessionId,
  children,
}: {
  sessionId?: string;
  children: ReactNode;
}) {
  const hostId = useHostId();
  const [timelines, setTimelines] = useState<
    ReadonlyMap<string, CanonicalAgentTimeline>
  >(new Map());
  const [connections, setConnections] = useState<
    ReadonlyMap<
      string,
      { connected: boolean; initialized: boolean; error: string | null }
    >
  >(new Map());
  const runsQuery = useQuery({
    queryKey: ['agent-runs', 'session', sessionId, hostId],
    queryFn: () => agentRunsApi.listForSession(sessionId!, hostId),
    enabled: Boolean(sessionId),
    refetchInterval: sessionId ? AGENT_RUN_DISCOVERY_INTERVAL_MS : false,
  });

  useEffect(() => {
    setTimelines(new Map());
    setConnections(new Map());
  }, [sessionId]);

  const updateTimeline = useCallback(
    (agentRunId: string, timeline: CanonicalAgentTimeline) => {
      setTimelines((current) => {
        if (current.get(agentRunId) === timeline) return current;
        const next = new Map(current);
        next.set(agentRunId, timeline);
        return next;
      });
    },
    []
  );

  const updateConnection = useCallback(
    (
      agentRunId: string,
      connected: boolean,
      initialized: boolean,
      error: string | null
    ) => {
      setConnections((current) => {
        const previous = current.get(agentRunId);
        if (
          previous?.connected === connected &&
          previous.initialized === initialized &&
          previous.error === error
        ) {
          return current;
        }
        const next = new Map(current);
        next.set(agentRunId, { connected, initialized, error });
        return next;
      });
    },
    []
  );

  const summaries = useMemo(() => runsQuery.data ?? [], [runsQuery.data]);
  const timeline = useMemo(
    () =>
      sessionId
        ? buildCanonicalAgentSessionTimeline(sessionId, summaries, timelines)
        : null,
    [sessionId, summaries, timelines]
  );
  const streamsInitialized = summaries.every(
    (summary) => connections.get(summary.agent_run_id)?.initialized
  );
  const streamErrors = useMemo(
    () =>
      summaries
        .map((summary) => connections.get(summary.agent_run_id)?.error)
        .filter((error): error is string => Boolean(error)),
    [connections, summaries]
  );
  const isLoading =
    Boolean(sessionId) && (runsQuery.isLoading || !streamsInitialized);
  const conversation = useMemo(
    () => projectCanonicalAgentConversation(timeline, isLoading),
    [isLoading, timeline]
  );
  const value = useMemo<AgentRunSessionContextValue>(
    () => ({
      timeline,
      conversation,
      isLoading,
      isConnected:
        summaries.length === 0 ||
        summaries.every(
          (summary) => connections.get(summary.agent_run_id)?.connected
        ),
      error:
        streamErrors[0] ??
        (runsQuery.error instanceof Error ? runsQuery.error.message : null),
    }),
    [
      connections,
      runsQuery.error,
      conversation,
      isLoading,
      streamErrors,
      summaries,
      timeline,
    ]
  );

  return (
    <AgentRunSessionContext.Provider value={value}>
      {summaries.map((summary) => (
        <AgentRunStreamBridge
          key={summary.agent_run_id}
          agentRunId={summary.agent_run_id}
          onTimeline={updateTimeline}
          onConnection={updateConnection}
        />
      ))}
      {children}
    </AgentRunSessionContext.Provider>
  );
}

export function AgentRunSessionProvider(props: {
  sessionId?: string;
  children: ReactNode;
}) {
  const hostId = useHostId();
  return (
    <ScopedAgentRunSessionProvider
      key={JSON.stringify([hostId, props.sessionId])}
      {...props}
    />
  );
}

export function useAgentRunSession(): AgentRunSessionContextValue {
  const context = useContext(AgentRunSessionContext);
  if (!context) {
    throw new Error(
      'useAgentRunSession must be used within AgentRunSessionProvider'
    );
  }
  return context;
}

/** Shared selectors also render outside a running conversation. */
export function useOptionalAgentRunSession(): AgentRunSessionContextValue | null {
  return useContext(AgentRunSessionContext);
}
