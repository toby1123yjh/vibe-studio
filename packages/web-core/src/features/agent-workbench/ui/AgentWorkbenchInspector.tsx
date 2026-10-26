import {
  useEffect,
  useId,
  useMemo,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import { useTranslation } from 'react-i18next';
import type { RepoWithTargetBranch, Workspace } from 'shared/types';

import { TerminalPanelContainer } from '@/shared/components/TerminalPanelContainer';
import { cn } from '@/shared/lib/utils';
import {
  RIGHT_MAIN_PANEL_MODES,
  type RightMainPanelMode,
} from '@/shared/stores/useUiPreferencesStore';
import { ChangesPanelContainer } from '@/pages/workspaces/ChangesPanelContainer';
import { GitPanelContainer } from '@/pages/workspaces/GitPanelContainer';
import { LogsContentContainer } from '@/pages/workspaces/LogsContentContainer';
import { PreviewBrowserContainer } from '@/pages/workspaces/PreviewBrowserContainer';
import { WorkspaceFilesSurfaceContainer } from '@/pages/workspaces/WorkspaceFilesSurfaceContainer';
import {
  AGENT_WORKBENCH_INSPECTOR_TABS,
  DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES,
  type AgentWorkbenchInspectorTab,
} from '../model';
import { useAgentWorkbenchInspectorVisibility } from './AgentWorkbenchInspectorVisibility';

function modeToTab(
  mode: RightMainPanelMode | null
): AgentWorkbenchInspectorTab | null {
  switch (mode) {
    case RIGHT_MAIN_PANEL_MODES.CHANGES:
      return 'changes';
    case RIGHT_MAIN_PANEL_MODES.FILES:
      return 'files';
    case RIGHT_MAIN_PANEL_MODES.LOGS:
      return 'logs';
    case RIGHT_MAIN_PANEL_MODES.PREVIEW:
      return 'preview';
    case null:
      return null;
  }
}

function tabToMode(tab: AgentWorkbenchInspectorTab): RightMainPanelMode | null {
  switch (tab) {
    case 'changes':
      return RIGHT_MAIN_PANEL_MODES.CHANGES;
    case 'files':
      return RIGHT_MAIN_PANEL_MODES.FILES;
    case 'logs':
      return RIGHT_MAIN_PANEL_MODES.LOGS;
    case 'preview':
      return RIGHT_MAIN_PANEL_MODES.PREVIEW;
    case 'git':
    case 'terminal':
      return null;
  }
}

function readInspectorTab(
  storageKey: string,
  mode: RightMainPanelMode | null,
  override: AgentWorkbenchInspectorTab | undefined
): AgentWorkbenchInspectorTab {
  const explicitTab = override ?? modeToTab(mode);
  if (explicitTab) return explicitTab;
  try {
    const saved = window.localStorage.getItem(storageKey);
    if (
      AGENT_WORKBENCH_INSPECTOR_TABS.includes(
        saved as AgentWorkbenchInspectorTab
      )
    ) {
      return saved as AgentWorkbenchInspectorTab;
    }
  } catch {
    // The Inspector remains usable when host storage is unavailable.
  }
  return DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES.activeTab;
}

export interface AgentWorkbenchInspectorProps {
  workspace: Workspace | undefined;
  repos: RepoWithTargetBranch[];
  rightMainPanelMode: RightMainPanelMode | null;
  onRightMainPanelModeChange: (mode: RightMainPanelMode | null) => void;
  activeTabOverride?: AgentWorkbenchInspectorTab;
  compact?: boolean;
  readOnly?: boolean;
  panelOverrides?: Partial<Record<AgentWorkbenchInspectorTab, ReactNode>>;
}

export function AgentWorkbenchInspector({
  workspace,
  repos,
  rightMainPanelMode,
  onRightMainPanelModeChange,
  activeTabOverride,
  compact = false,
  readOnly = false,
  panelOverrides,
}: AgentWorkbenchInspectorProps) {
  const { t } = useTranslation('common');
  const tabsId = useId();
  const inspectorVisible = useAgentWorkbenchInspectorVisibility();
  const tabs = useMemo(
    () =>
      readOnly
        ? AGENT_WORKBENCH_INSPECTOR_TABS.filter((tab) =>
            ['changes', 'files', 'logs'].includes(tab)
          )
        : AGENT_WORKBENCH_INSPECTOR_TABS,
    [readOnly]
  );
  const storageKey = `vk-agent-workbench-inspector-tab:${workspace?.id ?? 'none'}`;
  const [selection, setSelection] = useState(() => ({
    scope: storageKey,
    tab: readInspectorTab(storageKey, rightMainPanelMode, activeTabOverride),
  }));
  const requestedTab =
    activeTabOverride ??
    modeToTab(rightMainPanelMode) ??
    (selection.scope === storageKey
      ? selection.tab
      : readInspectorTab(storageKey, rightMainPanelMode, activeTabOverride));
  const activeTab = tabs.includes(requestedTab) ? requestedTab : 'files';
  const [mountedPanels, setMountedPanels] = useState<{
    scope: string;
    tabs: AgentWorkbenchInspectorTab[];
  }>({ scope: storageKey, tabs: [] });

  useEffect(() => {
    setSelection((current) =>
      current.scope === storageKey && current.tab === activeTab
        ? current
        : { scope: storageKey, tab: activeTab }
    );
  }, [activeTab, storageKey]);

  useEffect(() => {
    if (!inspectorVisible) return;
    setMountedPanels((current) => {
      const tabs = current.scope === storageKey ? current.tabs : [];
      return tabs.includes(activeTab)
        ? current
        : { scope: storageKey, tabs: [...tabs, activeTab] };
    });
  }, [activeTab, inspectorVisible, storageKey]);

  const panels = useMemo(
    () => ({
      changes: workspace?.id ? (
        <ChangesPanelContainer className="" workspaceId={workspace.id} />
      ) : null,
      files: workspace?.id ? (
        <WorkspaceFilesSurfaceContainer
          className=""
          workspaceId={workspace.id}
          mobile={compact}
        />
      ) : null,
      git: <GitPanelContainer selectedWorkspace={workspace} repos={repos} />,
      terminal: <TerminalPanelContainer workspace={workspace} />,
      preview: workspace?.id ? (
        <PreviewBrowserContainer
          workspaceId={workspace.id}
          repos={repos}
          className=""
        />
      ) : null,
      logs: <LogsContentContainer className="" workspace={workspace} />,
    }),
    [compact, repos, workspace]
  );
  const tabLabels = useMemo<Record<AgentWorkbenchInspectorTab, string>>(
    () => ({
      changes: t('agentWorkbench.inspector.tabs.changes', {
        defaultValue: 'Changes',
      }),
      files: t('agentWorkbench.inspector.tabs.files', {
        defaultValue: 'Files',
      }),
      git: t('agentWorkbench.inspector.tabs.git', { defaultValue: 'Git' }),
      terminal: t('agentWorkbench.inspector.tabs.terminal', {
        defaultValue: 'Terminal',
      }),
      preview: t('agentWorkbench.inspector.tabs.preview', {
        defaultValue: 'Preview',
      }),
      logs: t('agentWorkbench.inspector.tabs.logs', { defaultValue: 'Logs' }),
    }),
    [t]
  );

  const selectTab = (tab: AgentWorkbenchInspectorTab) => {
    setSelection({ scope: storageKey, tab });
    onRightMainPanelModeChange(tabToMode(tab));
    try {
      window.localStorage.setItem(storageKey, tab);
    } catch {
      // Preference persistence is best effort.
    }
  };

  const handleTabKeyDown = (
    event: KeyboardEvent<HTMLButtonElement>,
    tab: AgentWorkbenchInspectorTab
  ) => {
    const currentIndex = tabs.indexOf(tab);
    let nextIndex: number | null = null;
    switch (event.key) {
      case 'ArrowLeft':
        nextIndex = (currentIndex - 1 + tabs.length) % tabs.length;
        break;
      case 'ArrowRight':
        nextIndex = (currentIndex + 1) % tabs.length;
        break;
      case 'Home':
        nextIndex = 0;
        break;
      case 'End':
        nextIndex = tabs.length - 1;
        break;
      default:
        return;
    }

    event.preventDefault();
    const nextTab = tabs[nextIndex];
    selectTab(nextTab);
    document.getElementById(`${tabsId}-tab-${nextTab}`)?.focus();
  };

  return (
    <div className="vk-agent-workbench-inspector flex h-full min-h-0 min-w-0 flex-col bg-secondary">
      <div
        className="flex shrink-0 flex-wrap gap-1 border-b p-1"
        role="tablist"
        aria-label={t('agentWorkbench.inspector.label', {
          defaultValue: 'Inspector',
        })}
      >
        {tabs.map((tab) => (
          <button
            key={tab}
            id={`${tabsId}-tab-${tab}`}
            type="button"
            role="tab"
            aria-selected={activeTab === tab}
            aria-controls={`${tabsId}-panel-${tab}`}
            tabIndex={activeTab === tab ? 0 : -1}
            className={cn(
              'min-h-9 min-w-0 rounded px-2 text-xs text-muted-foreground',
              'hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand',
              activeTab === tab && 'bg-accent text-foreground'
            )}
            onClick={() => selectTab(tab)}
            onKeyDown={(event) => handleTabKeyDown(event, tab)}
          >
            {tabLabels[tab]}
          </button>
        ))}
      </div>
      <div className="relative min-h-0 flex-1">
        {tabs.map((tab) => (
          <div
            key={tab}
            id={`${tabsId}-panel-${tab}`}
            role="tabpanel"
            aria-labelledby={`${tabsId}-tab-${tab}`}
            hidden={activeTab !== tab}
            className="absolute inset-0 min-h-0 min-w-0 overflow-hidden"
          >
            {mountedPanels.scope === storageKey &&
            mountedPanels.tabs.includes(tab)
              ? (panelOverrides?.[tab] ?? panels[tab])
              : null}
          </div>
        ))}
      </div>
    </div>
  );
}
