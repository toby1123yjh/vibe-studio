import {
  forwardRef,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from 'react';
import { useTranslation } from 'react-i18next';
import type { Workspace, Session, RepoWithTargetBranch } from 'shared/types';
import { createWorkspaceWithSession } from '@/shared/types/attempt';
import { WorkspacesMain } from '@vibe/ui/components/WorkspacesMain';
import { Button } from '@vibe/ui/components/Button';
import { cn } from '@vibe/ui/lib/cn';
import { ErrorState, LoadingState } from '@vibe/ui/components/StateSurface';
import {
  ConversationList,
  type ConversationListHandle,
} from '@/features/workspace-chat/ui/ConversationListContainer';
import { SessionChatBoxContainer } from '@/features/workspace-chat/ui/SessionChatBoxContainer';
import { ConversationInteractionPanel } from './ConversationInteractionPanel';
import { EntriesProvider } from '@/features/workspace-chat/model/contexts/EntriesContext';
import { MessageEditProvider } from '@/features/workspace-chat/model/contexts/MessageEditContext';
import { RetryUiProvider } from '@/features/workspace-chat/model/contexts/RetryUiContext';
import { ApprovalFeedbackProvider } from '@/features/workspace-chat/model/contexts/ApprovalFeedbackContext';
import { forwardWheelToScroller } from '@/features/workspace-chat/ui/forwardWheelToScroller';
import {
  useDiffError,
  useDiffStats,
  useDiffPaths,
  WorkspaceDiffPathsProvider,
} from '@/shared/stores/useWorkspaceDiffStore';
import type { WorkspaceDetailState } from '@/shared/lib/workspaceDetailState';
import { useDiffSummary } from '@/shared/hooks/useDiffSummary';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { LogsPanelActionsContext } from '@/shared/hooks/useLogsPanel';
import type { WorkspaceConversationViewState } from '../model/conversationViewState';
import {
  WorkspaceConversationLogs,
  type ConversationLogPreview,
} from './WorkspaceConversationLogs';

/**
 * Isolated component that reads diffStats from WorkspaceContext.
 * By pushing the context subscription down to this leaf, the parent
 * WorkspacesMainContainer (and its ConversationList child) no longer
 * rerenders when diffs/comments/repos stream in.
 */
function ConversationComposer({
  session,
  workspaceId,
  isNewSessionMode,
  isSessionsLoading,
  sessionsError,
  onRetrySessions,
  sessions,
  onSelectSession,
  onStartNewSession,
  onScrollToPreviousMessage,
  onScrollToBottom,
  onScrollToUserMessage,
  getActiveTurnPatchKey,
  workspace,
  presentation,
  showSessionControls,
  diffStats,
  diffError,
  diffPaths,
}: {
  session: Session | undefined;
  workspaceId: string | undefined;
  isNewSessionMode: boolean;
  isSessionsLoading: boolean;
  sessionsError: unknown;
  onRetrySessions?: () => Promise<void>;
  sessions: Session[];
  onSelectSession: (sessionId: string) => void;
  onStartNewSession: () => void;
  onScrollToPreviousMessage: () => void;
  onScrollToBottom: (behavior?: 'auto' | 'smooth') => void;
  onScrollToUserMessage: (patchKey: string) => void;
  getActiveTurnPatchKey: () => string | null;
  workspace: Workspace | null;
  presentation: 'full-page' | 'embedded';
  showSessionControls?: boolean;
  diffStats: {
    files_changed: number;
    lines_added: number;
    lines_removed: number;
  };
  diffError: unknown;
  diffPaths: Set<string>;
}) {
  const { t } = useTranslation('common');

  if (isSessionsLoading) {
    return (
      <LoadingState
        className={cn(
          'max-w-full min-h-24',
          presentation === 'embedded' ? 'w-full' : 'w-chat'
        )}
        title={t('agentSessionResume.loading', {
          defaultValue: 'Loading sessions...',
        })}
      />
    );
  }

  if (sessionsError && sessions.length === 0) {
    return (
      <ErrorState
        className={cn(
          'max-w-full min-h-24',
          presentation === 'embedded' ? 'w-full' : 'w-chat'
        )}
        title={t('agentSessionResume.error', {
          defaultValue: 'Failed to load sessions',
        })}
        description={t('workspaces.errorDescription', {
          defaultValue: 'Check the connection and try again.',
        })}
        action={
          onRetrySessions ? (
            <Button className="min-h-10" onClick={() => void onRetrySessions()}>
              {t('buttons.retry', { defaultValue: 'Retry' })}
            </Button>
          ) : undefined
        }
      />
    );
  }

  return (
    <SessionChatBoxContainer
      {...(isNewSessionMode && workspaceId
        ? {
            mode: 'new-session' as const,
            workspaceId,
            onSelectSession,
          }
        : session
          ? {
              mode: 'existing-session' as const,
              session,
              onSelectSession,
              onStartNewSession,
            }
          : {
              mode: 'placeholder' as const,
            })}
      sessions={sessions}
      filesChanged={diffStats.files_changed}
      linesAdded={diffStats.lines_added}
      linesRemoved={diffStats.lines_removed}
      diffStatsUnavailable={Boolean(diffError)}
      disableViewCode={presentation === 'embedded'}
      showOpenWorkspaceButton={false}
      presentation={presentation}
      showSessionControls={showSessionControls}
      workspace={workspace}
      diffPaths={diffPaths}
      onScrollToPreviousMessage={onScrollToPreviousMessage}
      onScrollToBottom={onScrollToBottom}
      onScrollToUserMessage={onScrollToUserMessage}
      getActiveTurnPatchKey={getActiveTurnPatchKey}
    />
  );
}

type ConversationComposerProps = Omit<
  Parameters<typeof ConversationComposer>[0],
  'diffStats' | 'diffError' | 'diffPaths'
>;

const EmbeddedConversationDiffContext = createContext<ReturnType<
  typeof useDiffSummary
> | null>(null);

function EmbeddedConversationDiffScope({
  workspaceId,
  children,
}: {
  workspaceId: string | null;
  children: ReactNode;
}) {
  const summary = useDiffSummary(workspaceId);
  return (
    <EmbeddedConversationDiffContext.Provider value={summary}>
      <WorkspaceDiffPathsProvider value={summary.paths}>
        {children}
      </WorkspaceDiffPathsProvider>
    </EmbeddedConversationDiffContext.Provider>
  );
}

function FullPageConversationComposer(props: ConversationComposerProps) {
  const diffStats = useDiffStats();
  const diffError = useDiffError();
  const diffPaths = useDiffPaths();
  return (
    <ConversationComposer {...props} {...{ diffStats, diffError, diffPaths }} />
  );
}

function EmbeddedConversationComposer(props: ConversationComposerProps) {
  const summary = useContext(EmbeddedConversationDiffContext);
  if (!summary)
    throw new Error('Embedded composer requires its workspace diff scope');
  return (
    <ConversationComposer
      {...props}
      diffStats={{
        files_changed: summary.fileCount,
        lines_added: summary.added,
        lines_removed: summary.deleted,
      }}
      diffError={summary.error}
      diffPaths={summary.paths}
    />
  );
}

export interface WorkspaceConversationContentHandle {
  scrollToBottom: (behavior?: 'auto' | 'smooth') => void;
}

export interface WorkspaceConversationContentProps {
  selectedWorkspace: Workspace | null;
  selectedSession: Session | undefined;
  selectedSessionId: string | undefined;
  sessions: Session[];
  repos: RepoWithTargetBranch[];
  onSelectSession: (sessionId: string) => void;
  isLoading: boolean;
  workspaceState?: WorkspaceDetailState;
  isRetryingWorkspace?: boolean;
  onRetryWorkspace?: () => Promise<void>;
  isSessionsLoading?: boolean;
  sessionsError?: unknown;
  onRetrySessions?: () => Promise<void>;
  isNewSessionMode: boolean;
  onStartNewSession: () => void;
  presentation?: 'full-page' | 'embedded';
  /** Allow workspace session selection when the embedding surface has no selector. */
  showSessionControls?: boolean;
  readOnly?: boolean;
  beforeConversation?: ReactNode;
  previewContent?: ReactNode;
  renderContextBar?: (containerRef: RefObject<HTMLElement>) => ReactNode;
  viewState?: WorkspaceConversationViewState;
  onViewStateChange?: (state: WorkspaceConversationViewState) => void;
}

export const WorkspaceConversationContent = forwardRef<
  WorkspaceConversationContentHandle,
  WorkspaceConversationContentProps
>(function WorkspaceConversationContent(
  {
    selectedWorkspace,
    selectedSession,
    selectedSessionId,
    sessions,
    repos,
    onSelectSession,
    isLoading,
    workspaceState = 'ready',
    isRetryingWorkspace = false,
    onRetryWorkspace,
    isSessionsLoading = false,
    sessionsError,
    onRetrySessions,
    isNewSessionMode,
    onStartNewSession,
    presentation = 'full-page',
    showSessionControls,
    readOnly = false,
    beforeConversation,
    previewContent,
    renderContextBar,
    viewState,
    onViewStateChange,
  },
  ref
) {
  const hostId = useHostId();
  const containerRef = useRef<HTMLElement>(null);
  const conversationListRef = useRef<ConversationListHandle>(null);
  const previewContainerRef = useRef<HTMLDivElement>(null);
  const conversationFocusRef = useRef<HTMLElement | null>(null);
  const previewWasVisibleRef = useRef(false);
  const [logPreview, setLogPreview] = useState<ConversationLogPreview | null>(
    null
  );
  const closeLogPreview = useCallback(() => setLogPreview(null), []);
  const embeddedLogActions = useMemo(
    () => ({
      viewProcessInPanel: (processId: string) => {
        if (processId) setLogPreview({ type: 'process', processId });
      },
      viewToolContentInPanel: (
        toolName: string,
        content: string,
        command?: string
      ) => setLogPreview({ type: 'tool', toolName, content, command }),
      expandTerminal: () => {},
      collapseTerminal: closeLogPreview,
    }),
    [closeLogPreview]
  );
  const visiblePreview =
    previewContent ??
    (presentation === 'embedded' && logPreview ? (
      <WorkspaceConversationLogs
        key={logPreview.type === 'process' ? logPreview.processId : 'tool'}
        content={logPreview}
        onBack={closeLogPreview}
      />
    ) : null);
  const previewIsVisible = Boolean(visiblePreview);

  useLayoutEffect(() => {
    if (previewIsVisible) {
      previewContainerRef.current
        ?.querySelector<HTMLElement>('button, a[href], [tabindex="0"]')
        ?.focus({ preventScroll: true });
    } else if (previewWasVisibleRef.current) {
      const trigger = conversationFocusRef.current;
      if (trigger?.isConnected) trigger.focus({ preventScroll: true });
    }
    previewWasVisibleRef.current = previewIsVisible;
  }, [previewIsVisible]);

  const workspaceWithSession = useMemo(() => {
    if (!selectedWorkspace) return undefined;
    return createWorkspaceWithSession(selectedWorkspace, selectedSession);
  }, [selectedWorkspace, selectedSession]);

  const handleScrollToPreviousMessage = useCallback(() => {
    conversationListRef.current?.scrollToPreviousUserMessage();
  }, []);

  const handleScrollToUserMessage = useCallback((patchKey: string) => {
    conversationListRef.current?.scrollToEntryByPatchKey(patchKey);
  }, []);

  const handleGetActiveTurnPatchKey = useCallback(() => {
    return conversationListRef.current?.getVisibleUserMessagePatchKey() ?? null;
  }, []);

  const [isAtBottom, setIsAtBottom] = useState(true);
  const isAtBottomRef = useRef(isAtBottom);
  const handleAtBottomChange = useCallback((atBottom: boolean) => {
    isAtBottomRef.current = atBottom;
    setIsAtBottom(atBottom);
  }, []);

  const handleScrollToBottom = useCallback(
    (behavior: 'auto' | 'smooth' = 'smooth') => {
      conversationListRef.current?.scrollToBottom(behavior);
    },
    []
  );

  const { session } = workspaceWithSession ?? {};

  useEffect(() => {
    isAtBottomRef.current = isAtBottom;
  }, [isAtBottom]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || typeof ResizeObserver === 'undefined') return;

    const chatBoxContainer = container.querySelector<HTMLElement>(
      '[data-chatbox-container="true"]'
    );
    if (!chatBoxContainer) return;

    let previousHeight = chatBoxContainer.getBoundingClientRect().height;

    const observer = new ResizeObserver(() => {
      const nextHeight = chatBoxContainer.getBoundingClientRect().height;

      if (Math.abs(nextHeight - previousHeight) < 0.5) return;
      const heightDelta = nextHeight - previousHeight;
      previousHeight = nextHeight;

      if (!isAtBottomRef.current) return;

      requestAnimationFrame(() => {
        if (!isAtBottomRef.current) return;
        conversationListRef.current?.adjustScrollBy(heightDelta);
      });
    });

    observer.observe(chatBoxContainer, { box: 'border-box' });

    return () => {
      observer.disconnect();
    };
  }, [workspaceWithSession?.id, session?.id]);

  const entriesProviderKey = workspaceWithSession
    ? `${hostId ?? 'local'}:${workspaceWithSession.id}:${selectedSessionId ?? 'new'}`
    : 'empty';

  const conversationContent =
    workspaceWithSession &&
    !isSessionsLoading &&
    (!sessionsError || sessions.length > 0) &&
    (Boolean(selectedSessionId) || isNewSessionMode) ? (
      <div
        className="flex-1 min-h-0 overflow-hidden flex justify-center"
        onWheel={(e) => forwardWheelToScroller(e, conversationListRef)}
      >
        <div
          className={cn(
            'max-w-full h-full',
            presentation === 'embedded' ? 'w-full' : 'w-chat'
          )}
        >
          <RetryUiProvider workspaceId={workspaceWithSession.id}>
            <ConversationList
              key={entriesProviderKey}
              ref={conversationListRef}
              attempt={workspaceWithSession}
              repos={repos}
              onAtBottomChange={handleAtBottomChange}
              sessionScopeId={selectedSessionId}
              initialViewState={viewState}
              onViewStateChange={onViewStateChange}
              readOnly={readOnly}
            />
          </RetryUiProvider>
        </div>
      </div>
    ) : null;

  const Composer =
    presentation === 'embedded'
      ? EmbeddedConversationComposer
      : FullPageConversationComposer;
  const chatBoxContent = readOnly ? null : (
    <Composer
      session={session}
      workspaceId={workspaceWithSession?.id}
      isNewSessionMode={isNewSessionMode}
      isSessionsLoading={isSessionsLoading}
      sessionsError={sessionsError}
      onRetrySessions={onRetrySessions}
      sessions={sessions}
      onSelectSession={onSelectSession}
      onStartNewSession={onStartNewSession}
      onScrollToPreviousMessage={handleScrollToPreviousMessage}
      onScrollToBottom={handleScrollToBottom}
      onScrollToUserMessage={handleScrollToUserMessage}
      getActiveTurnPatchKey={handleGetActiveTurnPatchKey}
      workspace={selectedWorkspace}
      presentation={presentation}
      showSessionControls={showSessionControls}
    />
  );

  const contextBarContent =
    workspaceWithSession && presentation === 'full-page'
      ? renderContextBar?.(containerRef)
      : null;

  const approvalBannerContent =
    workspaceWithSession && !readOnly ? (
      <ConversationInteractionPanel compact={presentation === 'embedded'} />
    ) : null;

  useImperativeHandle(
    ref,
    () => ({
      scrollToBottom: (behavior = 'smooth') => {
        conversationListRef.current?.scrollToBottom(behavior);
      },
    }),
    []
  );

  const content = (
    <ApprovalFeedbackProvider>
      <EntriesProvider key={entriesProviderKey} sessionId={selectedSessionId}>
        <MessageEditProvider>
          <div className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
            <div
              className="flex min-h-0 min-w-0 flex-1 flex-col"
              onFocusCapture={(event) => {
                if (event.target instanceof HTMLElement) {
                  conversationFocusRef.current = event.target;
                }
              }}
              style={visiblePreview ? { visibility: 'hidden' } : undefined}
              aria-hidden={visiblePreview ? true : undefined}
              ref={(element) =>
                element?.toggleAttribute('inert', Boolean(visiblePreview))
              }
            >
              {beforeConversation}
              <WorkspacesMain
                workspaceWithSession={
                  workspaceWithSession
                    ? { id: workspaceWithSession.id }
                    : undefined
                }
                isLoading={isLoading}
                workspaceState={workspaceState}
                isRetryingWorkspace={isRetryingWorkspace}
                onRetryWorkspace={onRetryWorkspace}
                containerRef={containerRef}
                conversationContent={conversationContent}
                chatBoxContent={chatBoxContent}
                contextBarContent={contextBarContent}
                isAtBottom={isAtBottom}
                onAtBottomChange={handleAtBottomChange}
                onScrollToBottom={handleScrollToBottom}
                approvalBannerContent={approvalBannerContent}
                compact={presentation === 'embedded'}
              />
            </div>
            {visiblePreview && (
              <div
                ref={previewContainerRef}
                className="absolute inset-0 z-20 flex min-h-0 min-w-0 flex-col overflow-hidden bg-primary"
              >
                {visiblePreview}
              </div>
            )}
          </div>
        </MessageEditProvider>
      </EntriesProvider>
    </ApprovalFeedbackProvider>
  );

  return presentation === 'embedded' ? (
    <LogsPanelActionsContext.Provider value={embeddedLogActions}>
      <EmbeddedConversationDiffScope
        workspaceId={selectedWorkspace?.id ?? null}
      >
        {content}
      </EmbeddedConversationDiffScope>
    </LogsPanelActionsContext.Provider>
  ) : (
    content
  );
});
