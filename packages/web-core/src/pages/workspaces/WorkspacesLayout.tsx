import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from 'react';
import { useTranslation } from 'react-i18next';
import type { CreateModeInitialState } from '@/shared/types/createMode';
import { useWorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { CreateModeProvider } from '@/features/create-mode/model/CreateModeProvider';
import {
  consumeCreateModeSeedState,
  getCreateModeSeedVersion,
  subscribeCreateModeSeedState,
} from '@/features/create-mode/model/createModeSeedStore';
import { ReviewProvider } from '@/shared/hooks/ReviewProvider';
import { ChangesViewProvider } from '@/shared/hooks/ChangesViewProvider';
import {
  WorkspacesMainContainer,
  type WorkspacesMainContainerHandle,
} from './WorkspacesMainContainer';
import { CreateChatBoxContainer } from '@/shared/components/CreateChatBoxContainer';
import { WorkspacesGuideDialog } from '@/shared/dialogs/shared/WorkspacesGuideDialog';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import {
  WorkspaceFilePreviewActionsProvider,
  WorkspaceFilesSelectionProvider,
  useWorkspaceFilesSelection,
  type WorkspaceFilePreviewTarget,
} from '@/features/workspace-files';

import {
  useUiPreferencesStore,
  useWorkspacePanelState,
  RIGHT_MAIN_PANEL_MODES,
  type RightMainPanelMode,
} from '@/shared/stores/useUiPreferencesStore';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import {
  findAppShellRecentSession,
  useAppShellRecentSessions,
} from '@/shared/hooks/useAppShellRecentSessions';
import {
  AgentWorkbenchContainer,
  AgentWorkbenchInspector,
  deriveAgentWorkbenchHeader,
} from '@/features/agent-workbench';
import { deriveWorkspaceDetailState } from '@/shared/lib/workspaceDetailState';

const WORKSPACES_GUIDE_ID = 'workspaces-guide';

export function WorkspacesLayout() {
  const appNavigation = useAppNavigation();
  const {
    workspaceId,
    workspace: selectedWorkspace,
    isLoading,
    isWorkspaceLoading,
    workspaceError,
    retryWorkspace,
    isCreateMode,
    selectedSession,
    selectedSessionId,
    sessions,
    isSessionsLoading,
    sessionsError,
    retrySessions,
    selectSession,
    repos,
    isNewSessionMode,
    startNewSession,
  } = useWorkspaceContext();

  const { t } = useTranslation('common');
  const workspaceRetryLockRef = useRef(false);
  const workspaceRetryEpochRef = useRef(0);
  const [isRetryingWorkspace, setIsRetryingWorkspace] = useState(false);

  useEffect(() => {
    workspaceRetryEpochRef.current += 1;
    workspaceRetryLockRef.current = false;
    setIsRetryingWorkspace(false);
  }, [workspaceId]);

  const handleRetryWorkspace = useCallback(async () => {
    if (workspaceRetryLockRef.current) return;
    workspaceRetryLockRef.current = true;
    const epoch = workspaceRetryEpochRef.current;
    setIsRetryingWorkspace(true);
    try {
      await retryWorkspace();
    } finally {
      if (workspaceRetryEpochRef.current === epoch) {
        workspaceRetryLockRef.current = false;
        setIsRetryingWorkspace(false);
      }
    }
  }, [retryWorkspace]);
  const recentSessions = useAppShellRecentSessions();
  const canonicalSession = useMemo(
    () => findAppShellRecentSession(recentSessions, selectedSessionId),
    [recentSessions, selectedSessionId]
  );
  const workbenchHeader = useMemo(() => {
    const fallbackTitle = isCreateMode
      ? t('agentWorkbench.header.newWorkspace', {
          defaultValue: 'New workspace',
        })
      : isNewSessionMode && sessions.length > 0
        ? t('agentWorkbench.header.newSession', {
            defaultValue: 'New session',
          })
        : selectedSession?.name?.trim() ||
          selectedWorkspace?.name?.trim() ||
          t('agentWorkbench.header.untitledTask', {
            defaultValue: 'Untitled task',
          });
    const taskId = canonicalSession?.task_id?.trim();
    return deriveAgentWorkbenchHeader({
      canonicalSession,
      fallbackTitle,
      taskLabel: taskId
        ? t('agentWorkbench.header.task', {
            taskId,
            defaultValue: 'Task {{taskId}}',
          })
        : undefined,
      workspaceContext: selectedWorkspace
        ? {
            containerRef: selectedWorkspace.container_ref,
            workingDir: selectedSession?.agent_working_dir,
            fallbackRepoName: repos.length === 1 ? repos[0].name : undefined,
            branch: selectedWorkspace.branch,
            workspaceKind: selectedWorkspace.workspace_kind,
            worktreeLabel: t('agentWorkbench.header.worktree', {
              defaultValue: 'Worktree',
            }),
          }
        : undefined,
    });
  }, [
    canonicalSession,
    isCreateMode,
    isNewSessionMode,
    repos,
    selectedSession,
    selectedWorkspace,
    sessions.length,
    t,
  ]);
  usePageTitle(
    isCreateMode ? t('workspaces.newWorkspace') : selectedWorkspace?.name
  );

  const seedVersion = useSyncExternalStore(
    subscribeCreateModeSeedState,
    getCreateModeSeedVersion,
    getCreateModeSeedVersion
  );
  const consumedSeedVersionRef = useRef(0);
  const [createModeSeed, setCreateModeSeed] = useState<{
    version: number;
    state: CreateModeInitialState | null;
  }>({
    version: 0,
    state: null,
  });

  useEffect(() => {
    if (!isCreateMode) {
      consumedSeedVersionRef.current = 0;
      setCreateModeSeed((current) =>
        current.version === 0 && current.state === null
          ? current
          : { version: 0, state: null }
      );
      return;
    }

    if (seedVersion === 0 || seedVersion === consumedSeedVersionRef.current) {
      return;
    }

    consumedSeedVersionRef.current = seedVersion;
    setCreateModeSeed({
      version: seedVersion,
      state: consumeCreateModeSeedState(),
    });
  }, [isCreateMode, seedVersion]);

  const createModeProviderKey =
    createModeSeed.version > 0
      ? `create-mode-seed-${createModeSeed.version}`
      : 'create-mode-seed-default';

  const toggleRightSidebar = useUiPreferencesStore(
    (state) => state.toggleRightSidebar
  );
  const mainContainerRef = useRef<WorkspacesMainContainerHandle>(null);

  const handleWorkspaceCreated = useCallback(
    (workspaceId: string) => {
      appNavigation.goToWorkspace(workspaceId);
    },
    [appNavigation]
  );

  // Use workspace-specific panel state (pass undefined when in create mode)
  const { isRightSidebarVisible, rightMainPanelMode, setRightMainPanelMode } =
    useWorkspacePanelState(isCreateMode ? undefined : workspaceId);

  const {
    config,
    updateAndSaveConfig,
    loading: configLoading,
  } = useUserSystem();
  const hasAutoShownWorkspacesGuide = useRef(false);

  // Auto-show Workspaces Guide on first visit
  useEffect(() => {
    if (hasAutoShownWorkspacesGuide.current) return;
    if (configLoading || !config) return;

    const seenFeatures = config.showcases?.seen_features ?? [];
    if (seenFeatures.includes(WORKSPACES_GUIDE_ID)) return;

    hasAutoShownWorkspacesGuide.current = true;

    void updateAndSaveConfig({
      showcases: { seen_features: [...seenFeatures, WORKSPACES_GUIDE_ID] },
    });
    WorkspacesGuideDialog.show().finally(() => WorkspacesGuideDialog.hide());
  }, [configLoading, config, updateAndSaveConfig]);

  const setInspectorVisible = useCallback(
    (visible: boolean) => {
      if (visible !== isRightSidebarVisible) toggleRightSidebar();
    },
    [isRightSidebarVisible, toggleRightSidebar]
  );

  const mainContent = (
    <ReviewProvider workspaceId={selectedWorkspace?.id}>
      <ChangesViewProvider workspaceId={selectedWorkspace?.id}>
        <WorkspaceFilesSelectionProvider>
          <MainWorkspaceFilePreviewActionsBridge
            workspaceId={selectedWorkspace?.id}
            setRightMainPanelMode={setRightMainPanelMode}
          >
            <AgentWorkbenchContainer
              title={workbenchHeader.title}
              subtitle={workbenchHeader.subtitle}
              conversation={
                isCreateMode ? (
                  <CreateChatBoxContainer
                    onWorkspaceCreated={handleWorkspaceCreated}
                  />
                ) : (
                  <WorkspacesMainContainer
                    ref={mainContainerRef}
                    selectedWorkspace={selectedWorkspace ?? null}
                    selectedSession={selectedSession}
                    selectedSessionId={selectedSessionId}
                    sessions={sessions}
                    repos={repos}
                    onSelectSession={selectSession}
                    isLoading={isLoading}
                    workspaceState={deriveWorkspaceDetailState({
                      hasWorkspaceId: Boolean(workspaceId),
                      isLoading: isWorkspaceLoading,
                      hasWorkspace: Boolean(selectedWorkspace),
                      error: workspaceError,
                    })}
                    isRetryingWorkspace={isRetryingWorkspace}
                    onRetryWorkspace={handleRetryWorkspace}
                    isSessionsLoading={isSessionsLoading}
                    sessionsError={sessionsError}
                    onRetrySessions={retrySessions}
                    isNewSessionMode={isNewSessionMode}
                    onStartNewSession={startNewSession}
                  />
                )
              }
              inspector={
                <AgentWorkbenchInspector
                  rightMainPanelMode={rightMainPanelMode}
                  workspace={selectedWorkspace}
                  repos={repos}
                  onRightMainPanelModeChange={setRightMainPanelMode}
                />
              }
              inspectorVisible={isRightSidebarVisible && !isCreateMode}
              inspectorAvailable={!isCreateMode && Boolean(selectedWorkspace)}
              onInspectorVisibleChange={setInspectorVisible}
            />
          </MainWorkspaceFilePreviewActionsBridge>
        </WorkspaceFilesSelectionProvider>
      </ChangesViewProvider>
    </ReviewProvider>
  );

  return (
    <div className="flex flex-1 min-h-0 h-full">
      <div className="flex-1 min-w-0 h-full">
        {isCreateMode ? (
          <CreateModeProvider
            key={createModeProviderKey}
            initialState={createModeSeed.state}
          >
            {mainContent}
          </CreateModeProvider>
        ) : (
          mainContent
        )}
      </div>
    </div>
  );
}

function MainWorkspaceFilePreviewActionsBridge({
  children,
  workspaceId,
  setRightMainPanelMode,
}: {
  children: ReactNode;
  workspaceId: string | undefined;
  setRightMainPanelMode: (mode: RightMainPanelMode | null) => void;
}) {
  const { openTarget } = useWorkspaceFilesSelection(workspaceId);

  const handleOpenWorkspaceFilePreview = useCallback(
    (target: WorkspaceFilePreviewTarget) => {
      openTarget(target);
      setRightMainPanelMode(RIGHT_MAIN_PANEL_MODES.FILES);
    },
    [openTarget, setRightMainPanelMode]
  );

  return (
    <WorkspaceFilePreviewActionsProvider
      enabled={Boolean(workspaceId)}
      onOpenWorkspaceFilePreview={handleOpenWorkspaceFilePreview}
    >
      {children}
    </WorkspaceFilePreviewActionsProvider>
  );
}
