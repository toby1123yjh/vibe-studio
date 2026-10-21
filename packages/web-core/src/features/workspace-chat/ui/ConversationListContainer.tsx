import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
} from 'react';
import { SpinnerIcon } from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';

import {
  findPreviousUserMessageIndex,
  type ConversationRow,
} from '../model/conversation-row-model';
import { deriveConversationEntries } from '../model/deriveConversationEntries';
import { deriveConversationTimeline } from '../model/deriveConversationTimeline';
import { useConversationVirtualizer } from '../model/useConversationVirtualizer';
import { useScrollCommandExecutor } from '../model/useScrollCommandExecutor';

import DisplayConversationEntry from './DisplayConversationEntry';
import {
  ConversationBlockEntry,
  ConversationBlockGroup,
} from './ConversationBlockEntry';
import { useAgentRunSession } from '@/features/agent-runtime/model/AgentRunSessionContext';
import { ApprovalFormProvider } from '@/shared/hooks/ApprovalForm';
import { useEntriesActions } from '../model/contexts/EntriesContext';
import {
  useResetProcess,
  type UseResetProcessResult,
} from '../model/hooks/useResetProcess';
import type {
  AddEntryType,
  ConversationTimelineSource,
  DisplayEntry,
} from '@/shared/hooks/useConversationHistory/types';
import {
  isAggregatedGroup,
  isAggregatedDiffGroup,
  isAggregatedFileChangeGroup,
  isAggregatedThinkingGroup,
} from '@/shared/hooks/useConversationHistory/types';
import { useConversationHistory } from '../model/hooks/useConversationHistory';
import { useSetTokenUsageInfo } from '../model/contexts/EntriesContext';
import type { WorkspaceWithSession } from '@/shared/types/attempt';
import type { RepoWithTargetBranch } from 'shared/types';
import { ChatEmptyState } from '@vibe/ui/components/ChatEmptyState';
import { ChatScriptPlaceholder } from '@vibe/ui/components/ChatScriptPlaceholder';
import { ScriptFixerDialog } from '@/shared/dialogs/scripts/ScriptFixerDialog';
import { useWorkflowNotifications } from '@/shared/hooks/useWorkflowManagement';
import { workflowNotificationEntries } from '@/features/workflow/model/workflowNotifications';
import { Button } from '@vibe/ui/components/Button';
import type { WorkspaceConversationViewState } from '../model/conversationViewState';

interface ConversationListProps {
  attempt: WorkspaceWithSession;
  repos?: RepoWithTargetBranch[];
  onAtBottomChange?: (atBottom: boolean) => void;
  sessionScopeId?: string;
  initialViewState?: WorkspaceConversationViewState;
  onViewStateChange?: (state: WorkspaceConversationViewState) => void;
  showScriptSetup?: boolean;
  readOnly?: boolean;
}

export interface ConversationListHandle {
  scrollToPreviousUserMessage: () => void;
  scrollToBottom: (behavior?: 'auto' | 'smooth') => void;
  adjustScrollBy: (delta: number) => void;
  getScrollElement: () => HTMLDivElement | null;
  scrollToEntryByPatchKey: (patchKey: string) => void;
  getVisibleUserMessagePatchKey: () => string | null;
}

const ALWAYS_UNVIRTUALIZED_TAIL_ROWS = 8;
const STREAMING_UNVIRTUALIZED_BUFFER_ROWS = 24;

function renderRowContent(
  entry: DisplayEntry,
  attempt: WorkspaceWithSession,
  resetAction: UseResetProcessResult,
  repos: RepoWithTargetBranch[],
  readOnly: boolean
): React.ReactNode {
  if (isAggregatedGroup(entry)) {
    const blockIds = entry.entries.map((item) => item.conversationBlockId);
    if (blockIds.every((id): id is string => Boolean(id))) {
      return <ConversationBlockGroup ids={blockIds} workspaceId={attempt.id} />;
    }
    return (
      <DisplayConversationEntry
        expansionKey={entry.patchKey}
        aggregatedGroup={entry}
        aggregatedDiffGroup={null}
        aggregatedFileChangeGroup={null}
        aggregatedThinkingGroup={null}
        entry={null}
        executionProcessId={entry.executionProcessId}
        workspaceWithSession={attempt}
        resetAction={resetAction}
        repos={repos}
        readOnly={readOnly}
      />
    );
  }

  if (isAggregatedDiffGroup(entry)) {
    return (
      <DisplayConversationEntry
        expansionKey={entry.patchKey}
        aggregatedGroup={null}
        aggregatedDiffGroup={entry}
        aggregatedFileChangeGroup={null}
        aggregatedThinkingGroup={null}
        entry={null}
        executionProcessId={entry.executionProcessId}
        workspaceWithSession={attempt}
        resetAction={resetAction}
        repos={repos}
        readOnly={readOnly}
      />
    );
  }

  if (isAggregatedThinkingGroup(entry)) {
    return (
      <DisplayConversationEntry
        expansionKey={entry.patchKey}
        aggregatedGroup={null}
        aggregatedDiffGroup={null}
        aggregatedFileChangeGroup={null}
        aggregatedThinkingGroup={entry}
        entry={null}
        executionProcessId={entry.executionProcessId}
        workspaceWithSession={attempt}
        resetAction={resetAction}
        repos={repos}
        readOnly={readOnly}
      />
    );
  }

  if (isAggregatedFileChangeGroup(entry)) {
    return (
      <DisplayConversationEntry
        expansionKey={entry.patchKey}
        aggregatedGroup={null}
        aggregatedDiffGroup={null}
        aggregatedFileChangeGroup={entry}
        aggregatedThinkingGroup={null}
        entry={null}
        executionProcessId={entry.executionProcessId}
        workspaceWithSession={attempt}
        resetAction={resetAction}
        repos={repos}
        readOnly={readOnly}
      />
    );
  }

  if (entry.type === 'STDOUT') {
    return <p>{entry.content}</p>;
  }
  if (entry.type === 'STDERR') {
    return <p>{entry.content}</p>;
  }

  if (entry.type === 'NORMALIZED_ENTRY') {
    if (entry.conversationBlockId) {
      return (
        <ConversationBlockEntry
          blockId={entry.conversationBlockId}
          workspaceId={attempt.id}
        />
      );
    }
    return (
      <DisplayConversationEntry
        expansionKey={entry.patchKey}
        entry={entry.content}
        aggregatedGroup={null}
        aggregatedDiffGroup={null}
        aggregatedFileChangeGroup={null}
        aggregatedThinkingGroup={null}
        executionProcessId={entry.executionProcessId}
        canonicalActive={entry.canonical?.active}
        workspaceWithSession={attempt}
        resetAction={resetAction}
        repos={repos}
        readOnly={readOnly}
      />
    );
  }

  return null;
}

export const ConversationList = forwardRef<
  ConversationListHandle,
  ConversationListProps
>(function ConversationList(
  {
    attempt,
    repos: reposProp = [],
    onAtBottomChange,
    sessionScopeId,
    initialViewState,
    onViewStateChange,
    showScriptSetup = true,
    readOnly = false,
  },
  ref
) {
  const { t } = useTranslation('common');
  const sessionConversation = useAgentRunSession();
  const savedViewRef = useRef(initialViewState);
  const reportViewRef = useRef(onViewStateChange);
  reportViewRef.current = onViewStateChange;
  const viewRestoredRef = useRef(!initialViewState);
  const { context: workflowContext, notifications: workflowNotifications } =
    useWorkflowNotifications(attempt.session?.id);
  const workflowSystemEntries = useMemo(
    () =>
      workflowNotificationEntries(
        workflowNotifications.data ?? [],
        attempt.session?.id ?? '',
        {
          heading: t('workflow.management.notification'),
          resolved: t('workflow.management.resolved'),
          pending: t('workflow.management.pending'),
          status: (status) =>
            t(`workflow.management.status.${status}`, { defaultValue: status }),
        }
      ),
    [workflowNotifications.data, attempt.session?.id, t]
  );
  const workflowSystemEntriesRef = useRef(workflowSystemEntries);
  workflowSystemEntriesRef.current = workflowSystemEntries;
  const lastTimelineSourceRef = useRef<ConversationTimelineSource | null>(null);
  const flushTimelineRef = useRef<(() => void) | null>(null);
  const agentWorkbenchTimelineCopy = useMemo(
    () => ({
      fileChanges: t('agentWorkbench.timeline.fileChanges', {
        defaultValue: 'File changes',
      }),
      working: t('agentWorkbench.timeline.working', {
        defaultValue: 'Agent is working',
      }),
      terminalStatuses: {
        cancelled: t('agentWorkbench.timeline.cancelled', {
          defaultValue: 'Agent run cancelled',
        }),
        failed: t('agentWorkbench.timeline.failed', {
          defaultValue: 'Agent run failed',
        }),
        crashed: t('agentWorkbench.timeline.crashed', {
          defaultValue: 'Agent run crashed',
        }),
        audit_failed: t('agentWorkbench.timeline.auditFailed', {
          defaultValue: 'Agent run audit failed',
        }),
      },
    }),
    [t]
  );
  const repos = reposProp;
  const resetAction = useResetProcess(attempt.id, attempt.session?.id);
  const conversationScopeKey = `${attempt.id}:${sessionScopeId ?? attempt.session?.id ?? 'new'}`;
  const [dataVersion, setDataVersion] = useState(0);
  const [loading, setLoading] = useState(true);
  const [hasSetupScriptRun, setHasSetupScriptRun] = useState(false);
  const [hasCleanupScriptRun, setHasCleanupScriptRun] = useState(false);
  const [hasRunningProcess, setHasRunningProcess] = useState(false);
  const { setEntries, reset } = useEntriesActions();
  const setTokenUsageInfo = useSetTokenUsageInfo();
  const scriptOutputCacheRef = useRef<
    Map<string, { count: number; output: string }>
  >(new Map());
  const scrollOnEntriesChangedRef = useRef<
    ((addType: AddEntryType, isInitialLoad: boolean) => void) | null
  >(null);
  const pendingUpdateRef = useRef<{
    source: ConversationTimelineSource;
    addType: AddEntryType;
    loading: boolean;
    isInitialLoad: boolean;
  } | null>(null);
  // rAF throttle: at most one state update per animation frame.
  // Replaces the previous 100ms trailing debounce which never fired during
  // continuous streaming (upstream rAF in streamJsonPatchEntries reset the
  // timer every ~16ms). TanStack Virtual has no internal batching — unlike
  // Virtuoso — so we need to drive renders explicitly via React state.
  // rAF naturally limits updates to the display refresh rate (~60fps) while
  // ensuring every frame reflects the latest data.
  const rafIdRef = useRef<number | null>(null);
  const planRevealSpacerRef = useRef<HTMLDivElement | null>(null);
  const pendingInteractionAnchorRef = useRef<{
    element: HTMLElement;
    top: number;
  } | null>(null);
  const pendingInteractionAnchorFrameRef = useRef<number | null>(null);
  const pendingInteractionAnchorDeadlineRef = useRef(0);

  // Use ref to access current repos without causing callback recreation
  const reposRef = useRef(repos);
  reposRef.current = repos;

  // Check if any repo has setup or cleanup scripts configured
  const hasSetupScript = repos.some((repo) => repo.setup_script);
  const hasCleanupScript = repos.some((repo) => repo.cleanup_script);

  // Handlers to open script fixer dialog for setup/cleanup scripts
  const handleConfigureSetup = useCallback(() => {
    const currentRepos = reposRef.current;
    if (readOnly || currentRepos.length === 0) return;

    ScriptFixerDialog.show({
      scriptType: 'setup',
      repos: currentRepos,
      workspaceId: attempt.id,
      sessionId: attempt.session?.id,
    });
  }, [attempt.id, attempt.session?.id, readOnly]);

  const handleConfigureCleanup = useCallback(() => {
    const currentRepos = reposRef.current;
    if (readOnly || currentRepos.length === 0) return;

    ScriptFixerDialog.show({
      scriptType: 'cleanup',
      repos: currentRepos,
      workspaceId: attempt.id,
      sessionId: attempt.session?.id,
    });
  }, [attempt.id, attempt.session?.id, readOnly]);

  // Determine if configure buttons should be shown
  const canConfigure = !readOnly && repos.length > 0;

  useEffect(() => {
    if (rafIdRef.current !== null) {
      cancelAnimationFrame(rafIdRef.current);
      rafIdRef.current = null;
    }
    pendingUpdateRef.current = null;
    lastTimelineSourceRef.current = null;
    scriptOutputCacheRef.current.clear();
    if (planRevealSpacerRef.current) {
      planRevealSpacerRef.current.style.height = '0px';
    }
    setLoading(true);
    setHasSetupScriptRun(false);
    setHasCleanupScriptRun(false);
    setHasRunningProcess(false);
    setDataVersion(0);
    reset();
  }, [conversationScopeKey, reset]);

  useEffect(() => {
    return () => {
      if (rafIdRef.current !== null) {
        cancelAnimationFrame(rafIdRef.current);
      }
    };
  }, []);

  // ---- TanStack Virtual plumbing ----
  const tanstackScrollRef = useRef<HTMLDivElement | null>(null);
  const historyAnchorRef = useRef<{
    key: string;
    top: number;
    ready: boolean;
    committed: boolean;
    corrections: number;
  } | null>(null);
  const returnToLatestRef = useRef<{
    behavior: 'auto' | 'smooth';
    ready: boolean;
    committed: boolean;
  } | null>(null);

  useEffect(() => {
    historyAnchorRef.current = null;
    returnToLatestRef.current = null;
  }, [conversationScopeKey]);

  useEffect(() => {
    const element = tanstackScrollRef.current;
    if (!element) return;
    const cancelWindowScroll = () => {
      historyAnchorRef.current = null;
      returnToLatestRef.current = null;
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (
        [
          'ArrowUp',
          'ArrowDown',
          'PageUp',
          'PageDown',
          'Home',
          'End',
          ' ',
        ].includes(event.key)
      )
        cancelWindowScroll();
    };
    element.addEventListener('wheel', cancelWindowScroll, { passive: true });
    element.addEventListener('touchstart', cancelWindowScroll, {
      passive: true,
    });
    element.addEventListener('pointerdown', cancelWindowScroll, {
      passive: true,
    });
    element.addEventListener('keydown', handleKeyDown);
    return () => {
      cancelWindowScroll();
      element.removeEventListener('wheel', cancelWindowScroll);
      element.removeEventListener('touchstart', cancelWindowScroll);
      element.removeEventListener('pointerdown', cancelWindowScroll);
      element.removeEventListener('keydown', handleKeyDown);
    };
  }, []);

  const clearPendingInteractionAnchor = useCallback(() => {
    if (pendingInteractionAnchorFrameRef.current !== null) {
      cancelAnimationFrame(pendingInteractionAnchorFrameRef.current);
      pendingInteractionAnchorFrameRef.current = null;
    }
    pendingInteractionAnchorDeadlineRef.current = 0;
    pendingInteractionAnchorRef.current = null;
  }, []);

  const programmaticScrollDeadlineRef = useRef(0);

  const shouldSuppressInteractionDrivenSizeAdjustment = useCallback(
    () =>
      historyAnchorRef.current !== null ||
      returnToLatestRef.current !== null ||
      performance.now() < programmaticScrollDeadlineRef.current ||
      (pendingInteractionAnchorRef.current !== null &&
        performance.now() < pendingInteractionAnchorDeadlineRef.current),
    []
  );

  const runInteractionAnchorCorrection = useCallback(() => {
    pendingInteractionAnchorFrameRef.current = null;

    const anchor = pendingInteractionAnchorRef.current;
    const activeScrollContainer = tanstackScrollRef.current;
    if (!anchor || !activeScrollContainer || !anchor.element.isConnected) {
      clearPendingInteractionAnchor();
      return;
    }

    const currentTop = anchor.element.getBoundingClientRect().top;
    const delta = currentTop - anchor.top;
    if (Math.abs(delta) >= 0.5) {
      activeScrollContainer.scrollTop += delta;
    }

    if (performance.now() < pendingInteractionAnchorDeadlineRef.current) {
      pendingInteractionAnchorFrameRef.current = requestAnimationFrame(
        runInteractionAnchorCorrection
      );
      return;
    }

    clearPendingInteractionAnchor();
  }, [clearPendingInteractionAnchor]);

  const handleConversationClickCapture = useCallback(
    (event: MouseEvent<HTMLDivElement>) => {
      const target = event.target;
      if (!(target instanceof Element)) return;

      const trigger = target.closest<HTMLElement>(
        'button, summary, [role="button"], [data-scroll-anchor-target]'
      );
      if (!trigger || trigger.closest('[data-scroll-anchor-ignore]')) return;

      const scrollContainer = tanstackScrollRef.current;
      if (!scrollContainer || !scrollContainer.contains(trigger)) return;

      clearPendingInteractionAnchor();
      pendingInteractionAnchorRef.current = {
        element: trigger,
        top: trigger.getBoundingClientRect().top,
      };

      pendingInteractionAnchorDeadlineRef.current = performance.now() + 250;
      pendingInteractionAnchorFrameRef.current = requestAnimationFrame(
        runInteractionAnchorCorrection
      );
    },
    [clearPendingInteractionAnchor, runInteractionAnchorCorrection]
  );

  const flushPendingUpdate = () => {
    rafIdRef.current = null;
    const pending = pendingUpdateRef.current;
    if (!pending) return;

    const derivedEntries = deriveConversationEntries({
      source: pending.source,
      scriptOutputCache: scriptOutputCacheRef.current,
      workflowSystemEntries: workflowSystemEntriesRef.current,
    });

    setHasSetupScriptRun(derivedEntries.hasSetupScriptRun);
    setHasCleanupScriptRun(derivedEntries.hasCleanupScriptRun);
    setHasRunningProcess(derivedEntries.hasRunningProcess);
    setTokenUsageInfo(derivedEntries.latestTokenUsageInfo);

    const derivedTimeline = deriveConversationTimeline(
      derivedEntries.entries,
      pending.source,
      prevEntriesRef.current,
      prevRowsRef.current,
      agentWorkbenchTimelineCopy
    );

    prevEntriesRef.current = derivedTimeline.displayEntries;
    prevRowsRef.current = derivedTimeline.rows;

    // Page completion and list derivation are separate React updates. Apply
    // their scroll transition only after the actual new window is committed.
    if (
      pending.source.canonical ===
      sessionConversation.store?.getSnapshot().conversation
    ) {
      if (historyAnchorRef.current?.ready)
        historyAnchorRef.current.committed = true;
      if (returnToLatestRef.current?.ready)
        returnToLatestRef.current.committed = true;
    }

    setDataVersion((current) => current + 1);
    setEntries(derivedEntries.entries);

    scrollOnEntriesChangedRef.current?.(pending.addType, pending.isInitialLoad);

    if (loading) {
      setLoading(pending.loading);
    }
  };
  flushTimelineRef.current = flushPendingUpdate;

  useEffect(() => {
    const source = lastTimelineSourceRef.current;
    if (!source) return;
    // Notification-only updates reuse the current conversation source and its
    // existing frame batch. They do not start a turn or touch the send queue.
    pendingUpdateRef.current ??= {
      source,
      addType: 'historic',
      loading: false,
      isInitialLoad: false,
    };
    if (rafIdRef.current === null) {
      rafIdRef.current = requestAnimationFrame(() =>
        flushTimelineRef.current?.()
      );
    }
  }, [workflowSystemEntries]);

  const onTimelineUpdated = (
    source: ConversationTimelineSource,
    addType: AddEntryType,
    newLoading: boolean
  ) => {
    lastTimelineSourceRef.current = source;
    pendingUpdateRef.current = {
      source,
      addType,
      loading: newLoading,
      isInitialLoad: addType === 'initial',
    };

    if (rafIdRef.current === null) {
      rafIdRef.current = requestAnimationFrame(flushPendingUpdate);
    }
  };

  const { isFirstTurn, isLoadingHistory } = useConversationHistory({
    attempt,
    onTimelineUpdated,
    scopeKey: conversationScopeKey,
  });

  const prevEntriesRef = useRef<DisplayEntry[]>([]);
  const prevRowsRef = useRef<ConversationRow[]>([]);
  const conversationRows = prevRowsRef.current;

  const hasActiveStreamingTurn = useMemo(
    () =>
      hasRunningProcess ||
      conversationRows.some((row) => row.rowFamily === 'loading'),
    [conversationRows, hasRunningProcess]
  );

  // A single long agent turn can contain thousands of tools. Retaining every
  // row since its user message defeats virtualization, so keep a bounded tail.
  const firstUnvirtualizedRowIndex = Math.max(
    0,
    conversationRows.length -
      ALWAYS_UNVIRTUALIZED_TAIL_ROWS -
      (hasActiveStreamingTurn ? STREAMING_UNVIRTUALIZED_BUFFER_ROWS : 0)
  );

  const virtualizedRows = useMemo(
    () => conversationRows.slice(0, firstUnvirtualizedRowIndex),
    [conversationRows, firstUnvirtualizedRowIndex]
  );

  const unvirtualizedTailRows = useMemo(
    () => conversationRows.slice(firstUnvirtualizedRowIndex),
    [conversationRows, firstUnvirtualizedRowIndex]
  );

  const conversationVirtualizer = useConversationVirtualizer({
    rows: virtualizedRows,
    totalRowCount: conversationRows.length,
    scrollContainerRef: tanstackScrollRef,
    onAtBottomChange,
    shouldSuppressSizeAdjustment: shouldSuppressInteractionDrivenSizeAdjustment,
  });

  const loadPrevious = useCallback(() => {
    const element = tanstackScrollRef.current;
    const store = sessionConversation.store;
    if (!element || !store || store.getSnapshot().isLoadingHistory) return;
    const top = element.getBoundingClientRect().top;
    const visible = [
      ...element.querySelectorAll<HTMLElement>('[data-semantic-key]'),
    ].find((row) => row.getBoundingClientRect().bottom > top);
    if (visible?.dataset.semanticKey)
      historyAnchorRef.current = {
        key: visible.dataset.semanticKey,
        top: visible.getBoundingClientRect().top - top,
        ready: false,
        committed: false,
        corrections: 0,
      };
    const anchor = historyAnchorRef.current;
    returnToLatestRef.current = null;
    conversationVirtualizer.releaseBottomLock();
    void store.loadPrevious().then(() => {
      if (anchor && historyAnchorRef.current === anchor) anchor.ready = true;
    });
  }, [sessionConversation.store, conversationVirtualizer]);

  // NOTE: Do NOT call conversationVirtualizer.virtualizer.measure() when
  // firstUnvirtualizedRowIndex changes. measure() wipes ALL cached item sizes,
  // triggering a massive re-measurement storm and multi-second jitter.
  // TanStack Virtual handles count changes automatically via getItemKey.

  const scrollToAbsoluteIndex = useCallback(
    (
      index: number,
      align: 'start' | 'center' | 'end' = 'start',
      behavior: 'auto' | 'smooth' = 'smooth'
    ): boolean => {
      if (index < 0 || index >= conversationRows.length) return false;

      const scrollEl = tanstackScrollRef.current;
      if (!scrollEl) return false;

      const targetNode = scrollEl.querySelector<HTMLElement>(
        `[data-row-index="${index}"]`
      );

      if (targetNode) {
        let top = targetNode.offsetTop;

        if (align === 'center') {
          top =
            targetNode.offsetTop -
            scrollEl.clientHeight / 2 +
            targetNode.offsetHeight / 2;
        } else if (align === 'end') {
          top =
            targetNode.offsetTop -
            scrollEl.clientHeight +
            targetNode.offsetHeight;
        }

        const requestedTop = Math.max(0, top);
        let maxScrollable = scrollEl.scrollHeight - scrollEl.clientHeight;
        const deficit = requestedTop - maxScrollable;

        if (deficit > 1 && align === 'start' && planRevealSpacerRef.current) {
          conversationVirtualizer.releaseBottomLock();
          planRevealSpacerRef.current.style.height = `${Math.ceil(deficit)}px`;
          maxScrollable = scrollEl.scrollHeight - scrollEl.clientHeight;
        }

        scrollEl.scrollTo({
          top: Math.min(requestedTop, maxScrollable),
          behavior,
        });
        return true;
      }

      if (index < virtualizedRows.length) {
        conversationVirtualizer.scrollToIndex(index, { align, behavior });
        return true;
      }

      return false;
    },
    [conversationRows.length, conversationVirtualizer, virtualizedRows.length]
  );

  const scrollToBottomAndClearSpacer = useCallback(
    (behavior?: 'auto' | 'smooth') => {
      historyAnchorRef.current = null;
      if (planRevealSpacerRef.current) {
        planRevealSpacerRef.current.style.height = '0px';
      }
      if (sessionConversation.hasNewer && sessionConversation.store) {
        if (returnToLatestRef.current) return;
        const transition = {
          behavior: behavior ?? 'auto',
          ready: false,
          committed: false,
        };
        returnToLatestRef.current = transition;
        void sessionConversation.store
          .loadLatest()
          .then(() => {
            if (returnToLatestRef.current === transition)
              transition.ready = true;
          })
          .catch(() => {
            if (returnToLatestRef.current === transition)
              returnToLatestRef.current = null;
          });
        return;
      }
      returnToLatestRef.current = null;
      conversationVirtualizer.scrollToBottom(behavior);
    },
    [
      conversationVirtualizer,
      sessionConversation.hasNewer,
      sessionConversation.store,
    ]
  );

  const restoreInitialPosition = useCallback(() => {
    const saved = savedViewRef.current;
    const element = tanstackScrollRef.current;
    if (!saved || !element) return false;
    if (!viewRestoredRef.current) {
      if (saved.atBottom) {
        conversationVirtualizer.scrollToBottom('auto');
      } else {
        conversationVirtualizer.releaseBottomLock();
        element.scrollTop = saved.scrollTop;
      }
      viewRestoredRef.current = true;
    }
    return true;
  }, [conversationVirtualizer]);

  const scrollExecutor = useScrollCommandExecutor({
    virtualizer: conversationVirtualizer.virtualizer,
    itemCount: conversationRows.length,
    dataVersion,
    checkIsAtBottom: () =>
      !historyAnchorRef.current &&
      !returnToLatestRef.current &&
      conversationVirtualizer.checkIsAtBottom(),
    scrollToBottom: scrollToBottomAndClearSpacer,
    scrollToAbsoluteIndex,
    restoreInitialPosition,
  });
  scrollOnEntriesChangedRef.current = scrollExecutor.onEntriesChanged;

  const windowScrollRef = useRef({ conversationRows, conversationVirtualizer });
  windowScrollRef.current = { conversationRows, conversationVirtualizer };
  useLayoutEffect(() => {
    const element = tanstackScrollRef.current;
    if (!element) return;
    const latest = returnToLatestRef.current;
    if (latest?.committed) {
      returnToLatestRef.current = null;
      windowScrollRef.current.conversationVirtualizer.scrollToBottom(
        latest.behavior
      );
      return;
    }
    const anchor = historyAnchorRef.current;
    if (!anchor?.committed) return;
    const index = windowScrollRef.current.conversationRows.findIndex(
      (row) => row.semanticKey === anchor.key
    );
    if (index < 0) {
      historyAnchorRef.current = null;
      return;
    }
    const correct = () => {
      const row = [
        ...element.querySelectorAll<HTMLElement>('[data-semantic-key]'),
      ].find((item) => item.dataset.semanticKey === anchor.key);
      if (!row) return false;
      element.scrollTop +=
        row.getBoundingClientRect().top -
        element.getBoundingClientRect().top -
        anchor.top;
      return true;
    };
    let frame: number | null = null;
    const correctAfterMeasurement = () => {
      if (historyAnchorRef.current !== anchor) return;
      anchor.corrections += 1;
      if (!correct() && anchor.corrections === 1)
        windowScrollRef.current.conversationVirtualizer.scrollToIndex(index, {
          align: 'start',
          behavior: 'auto',
        });
      if (anchor.corrections < 5) {
        frame = requestAnimationFrame(correctAfterMeasurement);
      } else {
        historyAnchorRef.current = null;
      }
    };
    correctAfterMeasurement();
    return () => {
      if (frame !== null) cancelAnimationFrame(frame);
    };
  }, [dataVersion]);

  // Determine if there are entries to show placeholders
  const hasEntries = conversationRows.length > 0;

  // Show placeholders only if script not configured AND not already run AND first turn
  const showSetupPlaceholder =
    showScriptSetup && !hasSetupScript && !hasSetupScriptRun && hasEntries;
  const showCleanupPlaceholder =
    showScriptSetup &&
    !hasCleanupScript &&
    !hasCleanupScriptRun &&
    !hasRunningProcess &&
    hasEntries &&
    isFirstTurn;

  // Expose scroll functionality via ref — delegates to TanStack Virtual
  const scrollToPreviousUserMessage = useCallback(() => {
    conversationVirtualizer.releaseBottomLock();

    const scrollEl = tanstackScrollRef.current;
    if (!scrollEl || conversationRows.length === 0) return;

    const containerTop = scrollEl.getBoundingClientRect().top;
    const rowNodes = Array.from(
      scrollEl.querySelectorAll<HTMLElement>('[data-row-index]')
    );

    let firstVisibleIndex = conversationRows.length - 1;

    for (const node of rowNodes) {
      const rect = node.getBoundingClientRect();
      if (rect.bottom <= containerTop + 1) continue;
      const indexAttr = node.dataset.rowIndex;
      if (!indexAttr) continue;
      const parsedIndex = Number.parseInt(indexAttr, 10);
      if (!Number.isFinite(parsedIndex)) continue;
      firstVisibleIndex = parsedIndex;
      break;
    }

    const targetIndex = findPreviousUserMessageIndex(
      conversationRows,
      firstVisibleIndex
    );

    if (targetIndex < 0) return;

    programmaticScrollDeadlineRef.current = performance.now() + 1000;

    let attempts = 0;
    const maxAttempts = 6;

    const correctScroll = () => {
      if (attempts >= maxAttempts) return;
      attempts++;

      programmaticScrollDeadlineRef.current = performance.now() + 500;

      const node = scrollEl.querySelector<HTMLElement>(
        `[data-row-index="${targetIndex}"]`
      );
      if (!node) {
        if (attempts === 1) {
          conversationVirtualizer.scrollToIndex(targetIndex, {
            align: 'start',
            behavior: 'auto',
          });
        }
        requestAnimationFrame(correctScroll);
        return;
      }

      const nodeRect = node.getBoundingClientRect();
      const contRect = scrollEl.getBoundingClientRect();
      const delta = nodeRect.top - contRect.top;

      if (Math.abs(delta) < 2) return;

      scrollEl.scrollTop += delta;
      requestAnimationFrame(correctScroll);
    };

    correctScroll();
  }, [conversationRows, conversationVirtualizer]);

  useImperativeHandle(
    ref,
    () => ({
      scrollToPreviousUserMessage: () => {
        scrollToPreviousUserMessage();
      },
      scrollToBottom: (behavior = 'smooth') => {
        scrollToBottomAndClearSpacer(behavior);
      },
      adjustScrollBy: (delta) => {
        if (Math.abs(delta) < 0.5) return;
        const scrollElement = tanstackScrollRef.current;
        if (!scrollElement) return;
        scrollElement.scrollTop += delta;
      },
      getScrollElement: () => tanstackScrollRef.current,
      scrollToEntryByPatchKey: (patchKey: string) => {
        const targetIndex = conversationRows.findIndex(
          (row) => row.entry.patchKey === patchKey
        );
        if (targetIndex < 0) return;

        const scrollEl = tanstackScrollRef.current;
        if (!scrollEl) return;

        conversationVirtualizer.releaseBottomLock();
        programmaticScrollDeadlineRef.current = performance.now() + 1000;

        // Initial scroll via scrollToAbsoluteIndex which handles both
        // virtualized and unvirtualized (tail) rows correctly.
        scrollToAbsoluteIndex(targetIndex, 'start', 'auto');

        // Correction loop: after the virtualizer lays out the target
        // row, its actual size may differ from the estimate, so we
        // iteratively adjust until the row is at the container top.
        let attempts = 0;
        const maxAttempts = 5;

        const correctScroll = () => {
          if (attempts >= maxAttempts) return;
          attempts++;

          programmaticScrollDeadlineRef.current = performance.now() + 500;

          const node = scrollEl.querySelector<HTMLElement>(
            `[data-row-index="${targetIndex}"]`
          );
          if (!node) {
            requestAnimationFrame(correctScroll);
            return;
          }

          const nodeRect = node.getBoundingClientRect();
          const contRect = scrollEl.getBoundingClientRect();
          const delta = nodeRect.top - contRect.top;

          if (Math.abs(delta) < 2) return;

          scrollEl.scrollTop += delta;
          requestAnimationFrame(correctScroll);
        };

        requestAnimationFrame(correctScroll);
      },
      getVisibleUserMessagePatchKey: () => {
        const scrollEl = tanstackScrollRef.current;
        if (!scrollEl || conversationRows.length === 0) return null;

        const containerTop = scrollEl.getBoundingClientRect().top;
        const rowNodes = Array.from(
          scrollEl.querySelectorAll<HTMLElement>('[data-row-index]')
        );

        let firstVisibleIndex = conversationRows.length - 1;

        for (const node of rowNodes) {
          const rect = node.getBoundingClientRect();
          if (rect.bottom <= containerTop + 1) continue;
          const indexAttr = node.dataset.rowIndex;
          if (!indexAttr) continue;
          const parsedIndex = Number.parseInt(indexAttr, 10);
          if (!Number.isFinite(parsedIndex)) continue;
          firstVisibleIndex = parsedIndex;
          break;
        }

        // Find the nearest user message at or before the first visible index
        for (let i = firstVisibleIndex; i >= 0; i--) {
          if (conversationRows[i].isUserMessage) {
            return conversationRows[i].entry.patchKey;
          }
        }
        return null;
      },
    }),
    [
      conversationRows,
      conversationVirtualizer,
      scrollToAbsoluteIndex,
      scrollToBottomAndClearSpacer,
      scrollToPreviousUserMessage,
    ]
  );

  const showLoader = loading && conversationRows.length === 0;
  const showEmptyState = !loading && conversationRows.length === 0;

  const { virtualItems, totalSize, measureElement } = conversationVirtualizer;

  const canReportViewRef = useRef(false);
  canReportViewRef.current =
    !loading && !isLoadingHistory && viewRestoredRef.current;
  useEffect(() => {
    const element = tanstackScrollRef.current;
    if (!element) return;
    const report = () => {
      if (!canReportViewRef.current) return;
      reportViewRef.current?.({
        scrollTop: element.scrollTop,
        atBottom:
          element.scrollHeight - element.clientHeight - element.scrollTop < 48,
      });
    };
    element.addEventListener('scroll', report, { passive: true });
    return () => {
      report();
      element.removeEventListener('scroll', report);
    };
  }, []);

  useEffect(() => {
    return () => {
      clearPendingInteractionAnchor();
    };
  }, [clearPendingInteractionAnchor]);

  return (
    <ApprovalFormProvider>
      <div className="relative h-full overflow-hidden">
        {showLoader && (
          <div className="absolute inset-0 flex items-center justify-center z-10">
            <SpinnerIcon className="size-6 animate-spin text-low" />
          </div>
        )}
        <div
          ref={tanstackScrollRef}
          className="h-full overflow-y-auto scrollbar-none"
          style={{ overflowAnchor: 'none', contain: 'strict' }}
          onClickCapture={handleConversationClickCapture}
        >
          <div className="pt-2">
            {sessionConversation.error && (
              <div
                role="status"
                className="mx-double mb-base flex items-center gap-base text-sm text-low"
              >
                <span className="min-w-0 flex-1 break-words">
                  {t('conversation.connectionError', {
                    defaultValue: 'Conversation updates unavailable',
                  })}
                  : {sessionConversation.error}
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() =>
                    void sessionConversation.store?.loadLatest().catch(() => {})
                  }
                >
                  {t('buttons.retry')}
                </Button>
              </div>
            )}
            {sessionConversation.hasMore && (
              <div className="flex justify-center py-2">
                <Button
                  variant="ghost"
                  size="sm"
                  data-scroll-anchor-ignore
                  disabled={sessionConversation.isLoadingHistory}
                  onClick={loadPrevious}
                >
                  {t('conversation.loadEarlierMessages', {
                    defaultValue: 'Load earlier messages',
                  })}
                </Button>
              </div>
            )}
            {sessionConversation.hasNewer && (
              <div className="sticky top-0 z-10 flex justify-center bg-primary/90 py-2">
                <Button
                  variant="ghost"
                  size="sm"
                  data-scroll-anchor-ignore
                  onClick={() => scrollToBottomAndClearSpacer('auto')}
                >
                  {t('conversation.returnToLatest', {
                    defaultValue: 'Return to latest messages',
                  })}
                </Button>
              </div>
            )}
            {workflowContext.error || workflowNotifications.error ? (
              <div
                role="status"
                className="mx-double mb-base flex items-center gap-base text-base text-low"
              >
                <span>{t('workflow.management.notificationsFailed')}</span>
                <Button
                  variant="ghost"
                  size="sm"
                  loading={
                    workflowContext.isFetching ||
                    workflowNotifications.isFetching
                  }
                  onClick={() => {
                    void workflowContext.refetch();
                    if (workflowContext.data)
                      void workflowNotifications.refetch();
                  }}
                >
                  {t('buttons.retry')}
                </Button>
              </div>
            ) : null}
            {showSetupPlaceholder && (
              <div className="my-base px-double">
                <ChatScriptPlaceholder
                  type="setup"
                  onConfigure={canConfigure ? handleConfigureSetup : undefined}
                />
              </div>
            )}
          </div>

          {isLoadingHistory &&
            !sessionConversation.isLoadingHistory &&
            !showLoader && (
              <div className="flex flex-col items-center gap-2 px-double py-3">
                <div className="flex w-full max-w-md flex-col gap-1.5">
                  <div className="flex items-center gap-2">
                    <div className="h-2.5 w-16 animate-pulse rounded-full bg-foreground/10" />
                    <div className="h-2.5 flex-1 animate-pulse rounded-full bg-foreground/[0.06]" />
                  </div>
                  <div className="flex items-center gap-2">
                    <div
                      className="h-2.5 w-24 animate-pulse rounded-full bg-foreground/[0.07]"
                      style={{ animationDelay: '150ms' }}
                    />
                    <div
                      className="h-2.5 w-32 animate-pulse rounded-full bg-foreground/[0.05]"
                      style={{ animationDelay: '150ms' }}
                    />
                  </div>
                </div>
                <span className="text-xs text-low">
                  {t('conversation.loadingEarlierMessages')}
                </span>
              </div>
            )}

          {showEmptyState && (
            <div className="flex min-h-full items-center justify-center px-double py-12">
              <ChatEmptyState
                title={t('conversation.emptyTitle', {
                  defaultValue: 'Send a message to start the conversation.',
                })}
                description={t('conversation.emptyDescription', {
                  defaultValue:
                    'Your workspace conversation will appear here once a new turn starts.',
                })}
              />
            </div>
          )}

          {virtualizedRows.length > 0 && (
            <div
              style={{
                height: `${totalSize}px`,
                width: '100%',
                position: 'relative',
              }}
            >
              {virtualItems.map((virtualItem) => {
                const row = virtualizedRows[virtualItem.index];
                if (!row) return null;
                return (
                  <div
                    key={row.semanticKey}
                    data-index={virtualItem.index}
                    data-row-index={virtualItem.index}
                    data-semantic-key={row.semanticKey}
                    ref={measureElement}
                    style={{
                      position: 'absolute',
                      top: 0,
                      left: 0,
                      width: '100%',
                      transform: `translateY(${virtualItem.start}px)`,
                    }}
                  >
                    {renderRowContent(
                      row.entry,
                      attempt,
                      resetAction,
                      repos,
                      readOnly
                    )}
                  </div>
                );
              })}
            </div>
          )}

          {unvirtualizedTailRows.map((row, tailIndex) => {
            const rowIndex = firstUnvirtualizedRowIndex + tailIndex;
            return (
              <div
                key={row.semanticKey}
                data-row-index={rowIndex}
                data-semantic-key={row.semanticKey}
              >
                {renderRowContent(
                  row.entry,
                  attempt,
                  resetAction,
                  repos,
                  readOnly
                )}
              </div>
            );
          })}

          {/* Plan-reveal spacer: provides extra scroll room so plan-reveal
              can align the plan entry to the top of the viewport. Height is set
              imperatively in scrollToAbsoluteIndex and cleared on scrollToBottom. */}
          <div ref={planRevealSpacerRef} style={{ height: 0 }} />

          {/* Footer placeholder */}
          <div className="pb-2">
            {showCleanupPlaceholder && (
              <div className="my-base px-double">
                <ChatScriptPlaceholder
                  type="cleanup"
                  onConfigure={
                    canConfigure ? handleConfigureCleanup : undefined
                  }
                />
              </div>
            )}
          </div>
        </div>
      </div>
    </ApprovalFormProvider>
  );
});

export default ConversationList;
