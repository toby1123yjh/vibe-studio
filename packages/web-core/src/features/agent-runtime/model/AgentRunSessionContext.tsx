import {
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useSyncExternalStore,
  type ReactNode,
} from 'react';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { agentConversationApi } from '@/shared/lib/agentConversationApi';
import { createHmrContext } from '@/shared/lib/hmrContext';
import {
  AgentConversationStore,
  getAgentConversationStore,
  retainAgentConversationStore,
  type ConversationSessionSnapshot,
} from './agentConversationStore';

export interface AgentRunSessionContextValue
  extends ConversationSessionSnapshot {
  readonly store: AgentConversationStore | null;
}

const StoreContext = createHmrContext<AgentConversationStore | null>(
  'AgentConversationStore',
  null
);
const EMPTY_STORE = new AgentConversationStore('', null);

/** A host/session owns one connection, including when two business hosts show it.
 * Context identity stays fixed during streaming; blocks and the low-frequency
 * session metadata have separate external subscriptions.
 */
export function AgentRunSessionProvider({
  sessionId,
  children,
}: {
  sessionId?: string;
  children: ReactNode;
}) {
  const hostId = useHostId();
  const store = useMemo(
    () =>
      sessionId
        ? getAgentConversationStore(
            sessionId,
            hostId ?? null,
            agentConversationApi
          )
        : null,
    [sessionId, hostId]
  );
  useEffect(
    () => (store ? retainAgentConversationStore(store) : undefined),
    [store]
  );
  return (
    <StoreContext.Provider value={store}>{children}</StoreContext.Provider>
  );
}

export function useAgentConversationStore() {
  return useContext(StoreContext);
}

export function useAgentRunSession(): AgentRunSessionContextValue {
  const store = useContext(StoreContext);
  const source = store ?? EMPTY_STORE;
  const snapshot = useSyncExternalStore(
    source.subscribe,
    source.getSnapshot,
    source.getSnapshot
  );
  return useMemo(() => ({ ...snapshot, store }), [snapshot, store]);
}

export function useConversationBlock(blockId: string) {
  const store = useContext(StoreContext);
  const subscribe = useCallback(
    (listener: () => void) =>
      store?.subscribeBlock(blockId, listener) ?? (() => {}),
    [store, blockId]
  );
  const get = useCallback(() => store?.getBlock(blockId), [store, blockId]);
  return useSyncExternalStore(subscribe, get, get);
}

/** Shared selectors also render outside a conversation. */
export function useOptionalAgentRunSession(): AgentRunSessionContextValue | null {
  const value = useAgentRunSession();
  return value.store ? value : null;
}
