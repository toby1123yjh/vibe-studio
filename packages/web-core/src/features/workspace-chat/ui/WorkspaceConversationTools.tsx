import {
  lazy,
  Suspense,
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { useTranslation } from 'react-i18next';
import { ArrowLeftIcon } from '@phosphor-icons/react';
import type { RepoWithTargetBranch, Workspace } from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import { LoadingState } from '@vibe/ui/components/StateSurface';
import { useDiffStream } from '@/shared/hooks/useDiffStream';
import { ChangesViewProvider } from '@/shared/hooks/ChangesViewProvider';
import { ReviewProvider } from '@/shared/hooks/ReviewProvider';
import { LogsPanelProvider } from '@/shared/providers/LogsPanelProvider';
import { WorkspaceDiffDataProvider } from '@/shared/stores/useWorkspaceDiffStore';
import {
  RIGHT_MAIN_PANEL_MODES,
  type RightMainPanelMode,
} from '@/shared/stores/useUiPreferencesStore';
import { WorkspaceFilesSelectionProvider } from '@/features/workspace-files/model/WorkspaceFilesSelectionProvider';
import { AgentWorkbenchInspectorVisibilityProvider } from '@/features/agent-workbench/ui/AgentWorkbenchInspectorVisibility';
import { ConversationToolsContext } from '../model/ConversationToolsContext';

const AgentWorkbenchInspector = lazy(() =>
  import('@/features/agent-workbench/ui/AgentWorkbenchInspector').then(
    (module) => ({ default: module.AgentWorkbenchInspector })
  )
);

/** The existing workbench tools, scoped to a float instead of the page route. */
export function WorkspaceConversationTools({
  workspace,
  repos,
  diffPaths,
  readOnly,
  children,
}: {
  workspace: Workspace | null;
  repos: RepoWithTargetBranch[];
  diffPaths: Set<string>;
  readOnly: boolean;
  children: ReactNode;
}) {
  const { t } = useTranslation('common');
  const [visible, setVisible] = useState(false);
  const [activated, setActivated] = useState(false);
  const [changesActivated, setChangesActivated] = useState(false);
  const [mode, setMode] = useState<RightMainPanelMode | null>(
    RIGHT_MAIN_PANEL_MODES.FILES
  );
  const [pendingComponentMarkdown, setPendingComponentMarkdown] = useState<
    string | null
  >(null);
  const conversationRef = useRef<HTMLDivElement | null>(null);
  const toolsRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLElement | null>(null);
  const fullDiff = useDiffStream(workspace?.id ?? null, changesActivated);
  const diffData = useMemo(
    () => ({ diffs: fullDiff.diffs, diffError: fullDiff.error, diffPaths }),
    [diffPaths, fullDiff.diffs, fullDiff.error]
  );

  const openTools = useCallback(
    (nextMode?: RightMainPanelMode) => {
      if (!workspace) return;
      if (
        document.activeElement instanceof HTMLElement &&
        conversationRef.current?.contains(document.activeElement)
      ) {
        triggerRef.current = document.activeElement;
      } else if (document.activeElement?.closest('[role="menu"]')) {
        triggerRef.current =
          conversationRef.current?.querySelector<HTMLElement>(
            '[data-conversation-actions-trigger]'
          ) ?? null;
      }
      if (nextMode) setMode(nextMode);
      if (nextMode === RIGHT_MAIN_PANEL_MODES.CHANGES) {
        setChangesActivated(true);
      }
      setActivated(true);
      setVisible(true);
    },
    [workspace]
  );
  const closeTools = useCallback(() => {
    setVisible(false);
    requestAnimationFrame(() => {
      if (triggerRef.current?.isConnected) {
        triggerRef.current.focus({ preventScroll: true });
      } else {
        conversationRef.current
          ?.querySelector<HTMLElement>('[data-lexical-editor="true"], button')
          ?.focus({ preventScroll: true });
      }
    });
  }, []);
  const onModeChange = useCallback((nextMode: RightMainPanelMode | null) => {
    setMode(nextMode);
    if (nextMode === RIGHT_MAIN_PANEL_MODES.CHANGES) setChangesActivated(true);
  }, []);
  const openChanges = useCallback(
    () => openTools(RIGHT_MAIN_PANEL_MODES.CHANGES),
    [openTools]
  );
  const openLogs = useCallback(
    (nextMode: RightMainPanelMode | null) => {
      if (nextMode) openTools(nextMode);
    },
    [openTools]
  );
  const insertComponentContext = useCallback(
    (markdown: string) => {
      setPendingComponentMarkdown(markdown);
      closeTools();
    },
    [closeTools]
  );
  const clearComponentContext = useCallback(
    () => setPendingComponentMarkdown(null),
    []
  );
  const actions = useMemo(
    () => ({
      openTools,
      pendingComponentMarkdown,
      insertComponentContext,
      clearComponentContext,
    }),
    [
      openTools,
      pendingComponentMarkdown,
      insertComponentContext,
      clearComponentContext,
    ]
  );

  useLayoutEffect(() => {
    if (!visible) return;
    const frame = requestAnimationFrame(() => {
      toolsRef.current
        ?.querySelector<HTMLButtonElement>('button')
        ?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [visible]);

  const changesLoading = !fullDiff.isInitialized && !fullDiff.error;
  return (
    <ConversationToolsContext.Provider value={actions}>
      <WorkspaceDiffDataProvider value={diffData}>
        <ReviewProvider workspaceId={workspace?.id}>
          <ChangesViewProvider
            workspaceId={workspace?.id}
            onViewChanges={openChanges}
            isolated
          >
            <WorkspaceFilesSelectionProvider>
              <LogsPanelProvider
                workspaceId={workspace?.id}
                rightMainPanelMode={mode}
                onRightMainPanelModeChange={openLogs}
              >
                <div className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
                  <div
                    className="flex min-h-0 min-w-0 flex-1 flex-col"
                    style={visible ? { visibility: 'hidden' } : undefined}
                    aria-hidden={visible || undefined}
                    ref={(element) => {
                      conversationRef.current = element;
                      element?.toggleAttribute('inert', visible);
                    }}
                    onFocusCapture={(event) => {
                      if (conversationRef.current?.contains(event.target)) {
                        triggerRef.current = event.target;
                      }
                    }}
                  >
                    {children}
                  </div>
                  {activated && (
                    <div
                      ref={toolsRef}
                      className="absolute inset-0 z-30 flex min-h-0 min-w-0 flex-col bg-primary"
                      hidden={!visible}
                      style={!visible ? { display: 'none' } : undefined}
                      aria-label={t('agentWorkbench.inspector.label')}
                      data-testid="conversation-tools"
                      data-workspace-id={workspace?.id}
                      onKeyDown={(event) => {
                        if (event.key !== 'Escape' || event.defaultPrevented)
                          return;
                        event.preventDefault();
                        event.stopPropagation();
                        closeTools();
                      }}
                    >
                      <div className="flex shrink-0 items-center gap-half border-b border-secondary p-half">
                        <Button variant="ghost" size="xs" onClick={closeTools}>
                          <span className="inline-flex items-center gap-half">
                            <ArrowLeftIcon
                              className="size-icon-sm shrink-0"
                              aria-hidden
                            />
                            {t('workflow.conversation.backToConversation', {
                              defaultValue: 'Back to conversation',
                            })}
                          </span>
                        </Button>
                        <span className="min-w-0 truncate text-sm font-medium text-low">
                          {t('agentWorkbench.inspector.label')}
                        </span>
                      </div>
                      <div className="min-h-0 flex-1">
                        <Suspense
                          fallback={
                            <LoadingState
                              className="h-full"
                              title={t('states.loading', {
                                defaultValue: 'Loading…',
                              })}
                            />
                          }
                        >
                          <AgentWorkbenchInspectorVisibilityProvider
                            visible={visible}
                          >
                            <AgentWorkbenchInspector
                              workspace={workspace ?? undefined}
                              repos={repos}
                              compact
                              readOnly={readOnly}
                              rightMainPanelMode={mode}
                              onRightMainPanelModeChange={onModeChange}
                              panelOverrides={{
                                changes: changesLoading ? (
                                  <LoadingState
                                    className="h-full"
                                    title={t('states.loading', {
                                      defaultValue: 'Loading…',
                                    })}
                                  />
                                ) : fullDiff.error ? (
                                  <div
                                    role="alert"
                                    className="p-base text-sm text-error"
                                  >
                                    {fullDiff.error}
                                  </div>
                                ) : undefined,
                              }}
                            />
                          </AgentWorkbenchInspectorVisibilityProvider>
                        </Suspense>
                      </div>
                    </div>
                  )}
                </div>
              </LogsPanelProvider>
            </WorkspaceFilesSelectionProvider>
          </ChangesViewProvider>
        </ReviewProvider>
      </WorkspaceDiffDataProvider>
    </ConversationToolsContext.Provider>
  );
}
