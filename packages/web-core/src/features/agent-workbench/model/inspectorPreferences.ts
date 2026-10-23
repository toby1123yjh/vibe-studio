export const AGENT_WORKBENCH_INSPECTOR_TABS = [
  'changes',
  'files',
  'git',
  'terminal',
  'preview',
  'logs',
] as const;

export type AgentWorkbenchInspectorTab =
  (typeof AGENT_WORKBENCH_INSPECTOR_TABS)[number];

export interface AgentWorkbenchInspectorPreferences {
  readonly visible: boolean;
  readonly activeTab: AgentWorkbenchInspectorTab;
  readonly width: number;
}

export const AGENT_WORKBENCH_INSPECTOR_MIN_WIDTH = 360;
export const AGENT_WORKBENCH_INSPECTOR_MAX_WIDTH = 480;
export const AGENT_WORKBENCH_CONVERSATION_MIN_WIDTH = 480;

export const DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES: AgentWorkbenchInspectorPreferences =
  { visible: false, activeTab: 'files', width: 380 };

export function normalizeAgentWorkbenchInspectorPreferences(
  value: Partial<AgentWorkbenchInspectorPreferences> | null | undefined
): AgentWorkbenchInspectorPreferences {
  const activeTab = AGENT_WORKBENCH_INSPECTOR_TABS.includes(
    value?.activeTab as AgentWorkbenchInspectorTab
  )
    ? (value?.activeTab as AgentWorkbenchInspectorTab)
    : DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES.activeTab;
  const width = Number.isFinite(value?.width)
    ? Math.min(
        AGENT_WORKBENCH_INSPECTOR_MAX_WIDTH,
        Math.max(
          AGENT_WORKBENCH_INSPECTOR_MIN_WIDTH,
          value?.width ?? DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES.width
        )
      )
    : DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES.width;
  return {
    visible:
      value?.visible ?? DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES.visible,
    activeTab,
    width,
  };
}

export function setAgentWorkbenchInspectorVisibility(
  state: AgentWorkbenchInspectorPreferences,
  visible: boolean
): AgentWorkbenchInspectorPreferences {
  return { ...state, visible };
}
