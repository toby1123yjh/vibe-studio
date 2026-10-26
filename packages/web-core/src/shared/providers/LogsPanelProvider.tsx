import {
  useState,
  useCallback,
  useEffect,
  useMemo,
  useContext,
  type ReactNode,
} from 'react';
import type { LogsPanelContent } from '@/shared/types/actions';
import {
  useWorkspacePanelState,
  RIGHT_MAIN_PANEL_MODES,
  type RightMainPanelMode,
} from '@/shared/stores/useUiPreferencesStore';
import { WorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import {
  LogsPanelActionsContext,
  LogsPanelContext,
} from '@/shared/hooks/useLogsPanel';

interface LogsPanelProviderProps {
  children: ReactNode;
  workspaceId?: string;
  rightMainPanelMode?: RightMainPanelMode | null;
  onRightMainPanelModeChange?: (mode: RightMainPanelMode | null) => void;
}

export function LogsPanelProvider({
  children,
  workspaceId: boundWorkspaceId,
  rightMainPanelMode: boundMode,
  onRightMainPanelModeChange,
}: LogsPanelProviderProps) {
  const workspaceContext = useContext(WorkspaceContext);
  const workspaceId = boundWorkspaceId ?? workspaceContext?.workspaceId;
  const pagePanel = useWorkspacePanelState(
    workspaceContext?.isCreateMode ? undefined : workspaceId
  );
  const rightMainPanelMode =
    boundMode === undefined ? pagePanel.rightMainPanelMode : boundMode;
  const setRightMainPanelMode =
    onRightMainPanelModeChange ?? pagePanel.setRightMainPanelMode;
  const [logsPanelContent, setLogsPanelContent] =
    useState<LogsPanelContent | null>(null);
  const [logSearchQuery, setLogSearchQuery] = useState('');
  const [logMatchIndices, setLogMatchIndices] = useState<number[]>([]);
  const [logCurrentMatchIdx, setLogCurrentMatchIdx] = useState(0);

  const isTerminalExpanded = logsPanelContent?.type === 'terminal';

  const logContentId =
    logsPanelContent?.type === 'process'
      ? logsPanelContent.processId
      : logsPanelContent?.type === 'tool'
        ? logsPanelContent.toolName
        : null;

  useEffect(() => {
    setLogSearchQuery('');
    setLogCurrentMatchIdx(0);
  }, [logContentId]);

  useEffect(() => {
    setLogCurrentMatchIdx(0);
  }, [logSearchQuery]);

  // Collapse terminal when switching away from Logs panel mode
  useEffect(() => {
    if (
      rightMainPanelMode !== RIGHT_MAIN_PANEL_MODES.LOGS &&
      isTerminalExpanded
    ) {
      setLogsPanelContent(null);
    }
  }, [rightMainPanelMode, isTerminalExpanded]);

  const handleLogPrevMatch = useCallback(() => {
    if (logMatchIndices.length === 0) return;
    setLogCurrentMatchIdx((prev) =>
      prev > 0 ? prev - 1 : logMatchIndices.length - 1
    );
  }, [logMatchIndices.length]);

  const handleLogNextMatch = useCallback(() => {
    if (logMatchIndices.length === 0) return;
    setLogCurrentMatchIdx((prev) =>
      prev < logMatchIndices.length - 1 ? prev + 1 : 0
    );
  }, [logMatchIndices.length]);

  const viewProcessInPanel = useCallback(
    (processId: string) => {
      setRightMainPanelMode(RIGHT_MAIN_PANEL_MODES.LOGS);
      setLogsPanelContent({ type: 'process', processId });
    },
    [setRightMainPanelMode]
  );

  const viewToolContentInPanel = useCallback(
    (toolName: string, content: string, command?: string) => {
      setRightMainPanelMode(RIGHT_MAIN_PANEL_MODES.LOGS);
      setLogsPanelContent({ type: 'tool', toolName, content, command });
    },
    [setRightMainPanelMode]
  );

  const expandTerminal = useCallback(() => {
    setRightMainPanelMode(RIGHT_MAIN_PANEL_MODES.LOGS);
    setLogsPanelContent({ type: 'terminal' });
  }, [setRightMainPanelMode]);

  const collapseTerminal = useCallback(() => {
    setLogsPanelContent(null);
  }, []);

  const actionsValue = useMemo(
    () => ({
      viewProcessInPanel,
      viewToolContentInPanel,
      expandTerminal,
      collapseTerminal,
    }),
    [
      viewProcessInPanel,
      viewToolContentInPanel,
      expandTerminal,
      collapseTerminal,
    ]
  );

  const value = useMemo(
    () => ({
      logsPanelContent,
      logSearchQuery,
      logMatchIndices,
      logCurrentMatchIdx,
      setLogSearchQuery,
      setLogMatchIndices,
      handleLogPrevMatch,
      handleLogNextMatch,
      viewProcessInPanel,
      viewToolContentInPanel,
      expandTerminal,
      collapseTerminal,
      isTerminalExpanded,
    }),
    [
      logsPanelContent,
      logSearchQuery,
      logMatchIndices,
      logCurrentMatchIdx,
      handleLogPrevMatch,
      handleLogNextMatch,
      viewProcessInPanel,
      viewToolContentInPanel,
      expandTerminal,
      collapseTerminal,
      isTerminalExpanded,
    ]
  );

  return (
    <LogsPanelActionsContext.Provider value={actionsValue}>
      <LogsPanelContext.Provider value={value}>
        {children}
      </LogsPanelContext.Provider>
    </LogsPanelActionsContext.Provider>
  );
}
