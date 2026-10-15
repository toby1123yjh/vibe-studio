import { useEffect, useRef, useState } from 'react';
import type {
  AgentEventCursor,
  AgentEventEnvelope,
  RunState,
} from 'shared/types';
import { openLocalApiWebSocket } from '@/shared/lib/localApiTransport';
import { useHostId } from '@/shared/providers/HostIdProvider';
import {
  emptyCanonicalAgentTimeline,
  isCanonicalAgentRunTerminal,
  mergeCanonicalAgentTimeline,
  type CanonicalAgentTimeline,
} from './canonicalAgentTimeline';

type AgentRunStreamMessage =
  | { type: 'event'; data: { event: AgentEventEnvelope; replay: boolean } }
  | {
      type: 'ready';
      data: { state: RunState; cursor?: AgentEventCursor | null };
    }
  | {
      type: 'state';
      data: { state: RunState; cursor?: AgentEventCursor | null };
    }
  | { type: 'error'; data: { message: string } };

export interface UseAgentRunCanonicalStreamResult {
  timeline: CanonicalAgentTimeline | null;
  isConnected: boolean;
  isInitialized: boolean;
  error: string | null;
}

/**
 * Consume the canonical AgentRun stream. Reconnects resume from the last
 * attempt/sequence cursor; the reducer drops duplicate replay events.
 */
export function useAgentRunCanonicalStream(
  agentRunId: string | undefined,
  enabled = true
): UseAgentRunCanonicalStreamResult {
  const hostId = useHostId();
  const [timeline, setTimeline] = useState<CanonicalAgentTimeline | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const [isInitialized, setIsInitialized] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const timelineRef = useRef<CanonicalAgentTimeline | null>(null);
  const retryTimerRef = useRef<number | null>(null);
  const retryAttemptRef = useRef(0);
  const socketRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    if (!enabled || !agentRunId) {
      socketRef.current?.close();
      socketRef.current = null;
      if (retryTimerRef.current !== null) {
        window.clearTimeout(retryTimerRef.current);
        retryTimerRef.current = null;
      }
      timelineRef.current = null;
      setTimeline(null);
      setIsConnected(false);
      setIsInitialized(false);
      setError(null);
      return;
    }

    let cancelled = false;
    let frame: number | null = null;
    let fallbackTimer: number | null = null;
    let pendingEvents: AgentEventEnvelope[] = [];
    let pendingSnapshots: Array<{
      state: RunState;
      cursor?: AgentEventCursor | null;
    }> = [];
    let pendingReady = false;
    timelineRef.current = emptyCanonicalAgentTimeline();
    setTimeline(timelineRef.current);
    setIsInitialized(false);
    setIsConnected(false);
    setError(null);
    retryAttemptRef.current = 0;

    const clearScheduledFlush = () => {
      if (frame !== null) window.cancelAnimationFrame(frame);
      if (fallbackTimer !== null) window.clearTimeout(fallbackTimer);
      frame = null;
      fallbackTimer = null;
    };
    const flush = () => {
      clearScheduledFlush();
      if (cancelled) return;
      let next = timelineRef.current ?? emptyCanonicalAgentTimeline();
      next = mergeCanonicalAgentTimeline(next, pendingEvents);
      for (const snapshot of pendingSnapshots) {
        next = mergeCanonicalAgentTimeline(
          next,
          [],
          snapshot.state,
          snapshot.cursor
        );
      }
      pendingEvents = [];
      pendingSnapshots = [];
      timelineRef.current = next;
      setTimeline(next);
      if (pendingReady) setIsInitialized(true);
      pendingReady = false;
    };
    const scheduleFlush = () => {
      // Bound the pending queue even when animation frames are suspended.
      // Received events are never dropped; reconnect uses the applied cursor.
      if (pendingEvents.length + pendingSnapshots.length >= 2048) {
        flush();
      } else if (frame === null) {
        frame = window.requestAnimationFrame(flush);
        fallbackTimer = window.setTimeout(flush, 100);
      }
    };

    const scheduleReconnect = () => {
      if (cancelled || retryTimerRef.current !== null) return;
      const delay = Math.min(8_000, 500 * 2 ** retryAttemptRef.current);
      retryAttemptRef.current += 1;
      retryTimerRef.current = window.setTimeout(() => {
        retryTimerRef.current = null;
        void connect();
      }, delay);
    };

    const connect = async () => {
      const cursor = timelineRef.current?.cursor;
      const params = new URLSearchParams();
      if (cursor) {
        params.set('after_attempt_number', String(cursor.run_attempt_number));
        params.set('after_sequence', String(cursor.sequence));
      }
      const endpoint = `/api/agent-runs/${agentRunId}/events/ws${
        params.toString() ? `?${params.toString()}` : ''
      }`;

      try {
        const socket = await openLocalApiWebSocket(endpoint, {
          hostScope: 'explicit',
          hostId,
        });
        if (cancelled) {
          socket.close();
          return;
        }
        socketRef.current = socket;
        socket.onopen = () => {
          if (cancelled || socketRef.current !== socket) return;
          retryAttemptRef.current = 0;
          setIsConnected(true);
          setError(null);
        };
        socket.onmessage = (message) => {
          if (cancelled || socketRef.current !== socket) return;
          try {
            const parsed = JSON.parse(message.data) as AgentRunStreamMessage;
            switch (parsed.type) {
              case 'event':
                pendingEvents.push(parsed.data.event);
                break;
              case 'ready':
              case 'state':
                pendingSnapshots.push(parsed.data);
                if (parsed.type === 'ready') pendingReady = true;
                break;
              case 'error':
                flush();
                setError(parsed.data.message);
                return;
            }
            if (
              parsed.type === 'ready' ||
              (parsed.type === 'state' &&
                isCanonicalAgentRunTerminal(parsed.data.state))
            ) {
              // Publish the final text and terminal state atomically.
              flush();
            } else {
              scheduleFlush();
            }
          } catch (parseError) {
            setError(
              parseError instanceof Error
                ? parseError.message
                : 'Invalid AgentRun event'
            );
          }
        };
        socket.onerror = () => {
          if (cancelled || socketRef.current !== socket) return;
          setError('AgentRun stream connection failed');
        };
        socket.onclose = () => {
          if (cancelled || socketRef.current !== socket) return;
          flush();
          socketRef.current = null;
          setIsConnected(false);
          scheduleReconnect();
        };
      } catch (connectError) {
        if (cancelled) return;
        setIsConnected(false);
        setError(
          connectError instanceof Error
            ? connectError.message
            : 'AgentRun stream unavailable'
        );
        scheduleReconnect();
      }
    };

    void connect();
    return () => {
      cancelled = true;
      clearScheduledFlush();
      pendingEvents = [];
      pendingSnapshots = [];
      socketRef.current?.close();
      socketRef.current = null;
      if (retryTimerRef.current !== null) {
        window.clearTimeout(retryTimerRef.current);
        retryTimerRef.current = null;
      }
    };
  }, [agentRunId, enabled, hostId]);

  return { timeline, isConnected, isInitialized, error };
}
