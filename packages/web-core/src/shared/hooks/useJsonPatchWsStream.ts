import { useCallback, useEffect, useState, useRef } from 'react';
import { produce } from 'immer';
import type { Operation } from 'rfc6902';
import { applyUpsertPatch } from '@/shared/lib/jsonPatch';
import {
  openLocalApiWebSocket,
  type LocalApiWebSocketOptions,
} from '@/shared/lib/localApiTransport';

type WsJsonPatchMsg = { JsonPatch: Operation[] };
type WsReadyMsg = { Ready: true };
type WsFinishedMsg = { finished: boolean };
type WsMsg = WsJsonPatchMsg | WsReadyMsg | WsFinishedMsg;

interface UseJsonPatchStreamOptions<T> {
  /**
   * Called once when the stream starts to inject initial data
   */
  injectInitialEntry?: (data: T) => void;
  /**
   * Filter/deduplicate patches before applying them
   */
  deduplicatePatches?: (patches: Operation[]) => Operation[];
  /** Bind the stream to an immutable Host instead of mutable route state. */
  transport?: LocalApiWebSocketOptions;
}

interface UseJsonPatchStreamResult<T> {
  data: T | undefined;
  isConnected: boolean;
  isInitialized: boolean;
  isRetrying: boolean;
  error: string | null;
  retry: () => void;
}

/**
 * Generic hook for consuming WebSocket streams that send JSON messages with patches
 */
export const useJsonPatchWsStream = <T extends object>(
  endpoint: string | undefined,
  enabled: boolean,
  initialData: () => T,
  options?: UseJsonPatchStreamOptions<T>
): UseJsonPatchStreamResult<T> => {
  const [data, setData] = useState<T | undefined>(undefined);
  const [isConnected, setIsConnected] = useState(false);
  const [isInitialized, setIsInitialized] = useState(false);
  const initializedForEndpointRef = useRef<string | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [isRetrying, setIsRetrying] = useState(false);
  const wsRef = useRef<WebSocket | null>(null);
  const dataRef = useRef<T | undefined>(undefined);
  const activeStreamIdentityRef = useRef<string | undefined>(undefined);
  const retryTimerRef = useRef<number | null>(null);
  const retryAttemptsRef = useRef<number>(0);
  const explicitRetryRef = useRef(false);
  const [retryNonce, setRetryNonce] = useState(0);
  const finishedRef = useRef<boolean>(false);

  const injectInitialEntry = options?.injectInitialEntry;
  const deduplicatePatches = options?.deduplicatePatches;
  const transportHostScope = options?.transport?.hostScope;
  const transportHostId = options?.transport?.hostId;
  const transportRelayHostId = options?.transport?.relayHostId;
  const streamIdentity = endpoint
    ? JSON.stringify([
        endpoint,
        transportHostScope ?? 'current',
        transportHostId ?? null,
        transportRelayHostId ?? null,
      ])
    : undefined;

  const retry = useCallback(() => {
    if (!enabled || !endpoint || explicitRetryRef.current) return;
    explicitRetryRef.current = true;
    setIsRetrying(true);
    setError(null);
    setIsConnected(false);
    if (retryTimerRef.current !== null) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
    if (wsRef.current) {
      const ws = wsRef.current;
      ws.onopen = null;
      ws.onmessage = null;
      ws.onerror = null;
      ws.onclose = null;
      ws.close();
      wsRef.current = null;
    }
    setRetryNonce((nonce) => nonce + 1);
  }, [enabled, endpoint, streamIdentity]);

  useEffect(() => {
    if (!enabled || !endpoint) {
      // Close connection and reset state
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
      if (retryTimerRef.current !== null) {
        window.clearTimeout(retryTimerRef.current);
        retryTimerRef.current = null;
      }
      retryAttemptsRef.current = 0;
      explicitRetryRef.current = false;
      finishedRef.current = false;
      activeStreamIdentityRef.current = undefined;
      initializedForEndpointRef.current = undefined;
      setData(undefined);
      setIsConnected(false);
      setIsInitialized(false);
      setIsRetrying(false);
      setError(null);
      dataRef.current = undefined;
      return;
    }

    const streamChanged = activeStreamIdentityRef.current !== streamIdentity;
    if (streamChanged) {
      activeStreamIdentityRef.current = streamIdentity;
      initializedForEndpointRef.current = undefined;
      retryAttemptsRef.current = 0;
      explicitRetryRef.current = false;
      setData(undefined);
      setIsConnected(false);
      setIsInitialized(false);
      setIsRetrying(false);
      setError(null);
      dataRef.current = initialData();

      if (injectInitialEntry) {
        injectInitialEntry(dataRef.current);
      }
    } else if (!dataRef.current) {
      dataRef.current = initialData();
    }

    let cancelled = false;

    const scheduleReconnect = () => {
      if (retryTimerRef.current !== null) return;
      const attempt = retryAttemptsRef.current;
      const delay = Math.min(8000, 1000 * Math.pow(2, attempt));
      retryTimerRef.current = window.setTimeout(() => {
        retryTimerRef.current = null;
        setRetryNonce((nonce) => nonce + 1);
      }, delay);
    };

    const finishExplicitRetry = () => {
      explicitRetryRef.current = false;
      setIsRetrying(false);
    };

    // Create WebSocket if it doesn't exist
    if (!wsRef.current) {
      // Reset finished flag for new connection
      finishedRef.current = false;

      void (async () => {
        try {
          const ws = await openLocalApiWebSocket(endpoint, {
            hostScope: transportHostScope,
            hostId: transportHostId,
            relayHostId: transportRelayHostId,
          });

          if (cancelled) {
            ws.close();
            return;
          }

          ws.onopen = () => {
            setIsConnected(true);
            // Reset backoff on successful connection
            retryAttemptsRef.current = 0;
            if (retryTimerRef.current !== null) {
              window.clearTimeout(retryTimerRef.current);
              retryTimerRef.current = null;
            }
          };

          ws.onmessage = (event) => {
            try {
              const msg: WsMsg = JSON.parse(event.data);

              // Handle JsonPatch messages (same as SSE json_patch event)
              if ('JsonPatch' in msg) {
                const patches: Operation[] = msg.JsonPatch;
                const filtered = deduplicatePatches
                  ? deduplicatePatches(patches)
                  : patches;

                const current = dataRef.current;
                if (!filtered.length || !current) return;

                // Use Immer for structural sharing - only modified parts get new references
                const next = produce(current, (draft) => {
                  applyUpsertPatch(draft, filtered);
                });

                dataRef.current = next;
                setData(next);
              }

              // Handle Ready messages (initial data has been sent)
              if ('Ready' in msg) {
                initializedForEndpointRef.current = streamIdentity;
                setData(dataRef.current);
                setIsInitialized(true);
                setError(null);
                finishExplicitRetry();
              }

              // Handle finished messages ({finished: true})
              // Treat finished as terminal - do NOT reconnect
              if ('finished' in msg) {
                finishedRef.current = true;
                ws.close(1000, 'finished');
                wsRef.current = null;
                setIsConnected(false);
              }
            } catch (err) {
              console.error('Failed to process WebSocket message:', err);
              setError('Failed to process stream update');
              finishExplicitRetry();
            }
          };

          ws.onerror = () => {
            // Don't set error here — onclose always fires after onerror
            // and handles retry logic. Setting error eagerly hides data
            // that was already received.
          };

          ws.onclose = (evt) => {
            setIsConnected(false);
            wsRef.current = null;

            // Do not reconnect if we received a finished message or clean close
            if (
              cancelled ||
              finishedRef.current ||
              (evt?.code === 1000 && evt?.wasClean)
            ) {
              return;
            }

            // Otherwise, reconnect on unexpected/error closures
            retryAttemptsRef.current += 1;
            setError('Connection failed');
            finishExplicitRetry();
            scheduleReconnect();
          };

          wsRef.current = ws;
        } catch (error) {
          if (cancelled) {
            return;
          }

          console.error('Failed to open WebSocket stream:', error);
          retryAttemptsRef.current += 1;
          setError('Connection failed');
          finishExplicitRetry();
          scheduleReconnect();
        }
      })();
    }

    return () => {
      cancelled = true;
      if (wsRef.current) {
        const ws = wsRef.current;

        // Clear all event handlers first to prevent callbacks after cleanup
        ws.onopen = null;
        ws.onmessage = null;
        ws.onerror = null;
        ws.onclose = null;

        // Close regardless of state
        ws.close();
        wsRef.current = null;
      }
      if (retryTimerRef.current !== null) {
        window.clearTimeout(retryTimerRef.current);
        retryTimerRef.current = null;
      }
      finishedRef.current = false;
    };
  }, [
    endpoint,
    enabled,
    initialData,
    injectInitialEntry,
    deduplicatePatches,
    retryNonce,
    streamIdentity,
    transportHostId,
    transportHostScope,
    transportRelayHostId,
  ]);

  const isInitializedForCurrentEndpoint =
    isInitialized && initializedForEndpointRef.current === streamIdentity;
  const dataForCurrentEndpoint =
    activeStreamIdentityRef.current === streamIdentity ? data : undefined;

  return {
    data: dataForCurrentEndpoint,
    isConnected,
    isInitialized: isInitializedForCurrentEndpoint,
    isRetrying,
    error: activeStreamIdentityRef.current === streamIdentity ? error : null,
    retry,
  };
};
