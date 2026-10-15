import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ScratchType,
  type DraftFollowUpData,
  type ExecutorConfig,
} from 'shared/types';
import { useScratch } from '@/shared/hooks/useScratch';
import { useDebouncedCallback } from '@/shared/hooks/useDebouncedCallback';
import { useHostId } from '@/shared/providers/HostIdProvider';

// Only in-flight writes live here. A remount can recover the original draft
// before its PUT and the replacement scratch stream have finished their race.
const pendingDraftWrites = new Map<
  string,
  { data: DraftFollowUpData | null; completion: Promise<void> }
>();

function writeDraftInOrder(
  scope: string,
  data: DraftFollowUpData | null,
  write: () => Promise<void>
): Promise<void> {
  const previous = pendingDraftWrites.get(scope);
  const completion = previous
    ? previous.completion.then(write, write)
    : write();
  const pending = { data, completion };
  pendingDraftWrites.set(scope, pending);
  const release = () => {
    if (pendingDraftWrites.get(scope) === pending) {
      pendingDraftWrites.delete(scope);
    }
  };
  void completion.then(release, release);
  return completion;
}

interface UseSessionMessageEditorOptions {
  /** Scratch ID (workspaceId for new session, sessionId for existing) */
  scratchId: string | undefined;
}

interface UseSessionMessageEditorResult {
  /** Current message value */
  localMessage: string;
  /** Set local message directly */
  setLocalMessage: (value: string) => void;
  /** Scratch data (message and variant) */
  scratchData: DraftFollowUpData | undefined;
  /** Whether scratch is loading */
  isScratchLoading: boolean;
  /** Whether the initial value has been applied from scratch */
  hasInitialValue: boolean;
  /** Save message and executor config to scratch */
  saveToScratch: (
    message: string,
    executorConfig: ExecutorConfig
  ) => Promise<void>;
  /** Delete the draft scratch */
  clearDraft: () => Promise<void>;
  /** Cancel pending debounced save */
  cancelDebouncedSave: () => void;
  /** Handle message change with debounced save */
  handleMessageChange: (value: string, executorConfig: ExecutorConfig) => void;
}

/**
 * Hook to manage message editing with draft persistence.
 * Handles local state, debounced saves to scratch, and sync on load.
 */
export function useSessionMessageEditor({
  scratchId,
}: UseSessionMessageEditorOptions): UseSessionMessageEditorResult {
  const hostId = useHostId();
  const scope = JSON.stringify([hostId, scratchId]);
  const initialHandoff = useMemo(
    () => pendingDraftWrites.get(scope)?.data,
    [scope]
  );
  const {
    scratch,
    updateScratch,
    deleteScratch,
    isLoading: isScratchLoading,
  } = useScratch(ScratchType.DRAFT_FOLLOW_UP, scratchId ?? '');

  const persistedScratchData: DraftFollowUpData | undefined =
    scratch?.payload?.type === 'DRAFT_FOLLOW_UP'
      ? scratch.payload.data
      : undefined;

  const [handoffDraft, setHandoffDraft] = useState(initialHandoff);
  const scratchData =
    handoffDraft !== undefined
      ? (handoffDraft ?? undefined)
      : persistedScratchData;
  const [localMessage, setLocalMessage] = useState(
    initialHandoff?.message ?? ''
  );
  const [hasInitialValue, setHasInitialValue] = useState(
    initialHandoff !== undefined
  );
  const pendingSaveRef = useRef<(() => Promise<void>) | null>(null);

  useEffect(() => {
    if (
      handoffDraft !== undefined &&
      !isScratchLoading &&
      (handoffDraft === null
        ? !persistedScratchData
        : persistedScratchData?.message === handoffDraft.message &&
          JSON.stringify(persistedScratchData.executor_config) ===
            JSON.stringify(handoffDraft.executor_config))
    ) {
      setHandoffDraft(undefined);
    }
  }, [handoffDraft, persistedScratchData, isScratchLoading]);

  const saveToScratch = useCallback(
    async (message: string, executorConfig: ExecutorConfig) => {
      if (!scratchId) return;
      const data: DraftFollowUpData = {
        message,
        executor_config: executorConfig,
      };
      // A config edit or explicit save supersedes an older debounced snapshot.
      pendingSaveRef.current = null;
      try {
        await writeDraftInOrder(scope, data, () =>
          updateScratch({
            payload: {
              type: 'DRAFT_FOLLOW_UP',
              data,
            },
          })
        );
      } catch (e) {
        console.error('Failed to save follow-up draft', e);
      }
    },
    [scratchId, scope, updateScratch]
  );

  const clearDraft = useCallback(async () => {
    if (!scratchId) return;
    pendingSaveRef.current = null;
    // Delete after previous PUTs so a slow save cannot revive sent content.
    await writeDraftInOrder(scope, null, deleteScratch);
  }, [scratchId, scope, deleteScratch]);

  // The existing draft owner also flushes a rapid close/switch. Capture the
  // immutable save function here so a late flush cannot target another Host.
  const { debounced: debouncedSave, cancel: cancelTimer } =
    useDebouncedCallback(() => {
      const save = pendingSaveRef.current;
      pendingSaveRef.current = null;
      void save?.();
    }, 500);
  const cancelDebouncedSave = useCallback(() => {
    pendingSaveRef.current = null;
    cancelTimer();
  }, [cancelTimer]);
  useEffect(
    () => () => {
      const save = pendingSaveRef.current;
      pendingSaveRef.current = null;
      cancelTimer();
      void save?.();
    },
    [scratchId, hostId, cancelTimer]
  );

  // Track whether initial load has happened to avoid re-syncing during typing
  const hasLoadedRef = useRef(initialHandoff !== undefined);

  // Reset load state and clear message when scratchId changes (e.g., switching to approval mode)
  useEffect(() => {
    // A replacement may render before the old instance's unmount flush.
    // Passive cleanups run first, so capture that write again at commit.
    const pending = pendingDraftWrites.get(scope);
    const handoff = pending ? pending.data : initialHandoff;
    hasLoadedRef.current = handoff !== undefined;
    setHandoffDraft(handoff);
    setHasInitialValue(handoff !== undefined);
    setLocalMessage(handoff?.message ?? '');
  }, [initialHandoff, scope]);

  // Sync local message from scratch only on initial load
  useEffect(() => {
    if (isScratchLoading) return;
    if (hasLoadedRef.current) return;
    hasLoadedRef.current = true;
    setLocalMessage(scratchData?.message ?? '');
    setHasInitialValue(true);
  }, [isScratchLoading, scratchData?.message]);

  // Handle message change with debounced save
  // Pass executor profile at call-time to avoid stale closure
  const handleMessageChange = useCallback(
    (value: string, executorConfig: ExecutorConfig) => {
      setLocalMessage(value);
      pendingSaveRef.current = () => saveToScratch(value, executorConfig);
      debouncedSave();
    },
    [debouncedSave, saveToScratch]
  );

  return {
    localMessage,
    setLocalMessage,
    scratchData,
    isScratchLoading,
    hasInitialValue,
    saveToScratch,
    clearDraft,
    cancelDebouncedSave,
    handleMessageChange,
  };
}
