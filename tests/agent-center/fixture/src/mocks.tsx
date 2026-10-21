import * as React from 'react';
import {
  AgentProviderCapability,
  AgentProviderReadiness,
  type AgentSettingsProvider,
  type AgentCommandInventoryView,
  type AgentToolInventoryView,
  type AgentToolView,
  type CreateAgentCommandRequest,
  type CreateAgentToolRequest,
  type ExecutorConfig,
  type ModelSelectorConfig,
  type SettingsSnapshot,
  NativeConfigFormat,
  NativeParseStatus,
  SettingActivation,
  SettingControl,
  SettingScope,
  SettingSection,
  SettingValueType,
  BaseCodingAgent,
} from 'shared/types';
import {
  AGENT_PROVIDERS,
  AGENT_PROVIDER_BY_EXECUTOR,
  agentProviderForSettings,
} from '../../../../packages/web-core/src/shared/lib/agentProviders';
import type { ExecutorDiscoveryContext } from '../../../../packages/web-core/src/shared/lib/executorDiscovery';

const providers = AGENT_PROVIDERS.map((provider) => provider.executor);

const params = new URLSearchParams(window.location.search);
export class ApiError extends Error {}
const calls = { relay: 0, paired: 0, garage: 0 };
Object.assign(window, { agentCenterCalls: calls });
const modelDiscoveryProbe: {
  executor: BaseCodingAgent | null | undefined;
  hostId: string | null;
  context: ExecutorDiscoveryContext | undefined;
  refreshCount: number;
}[] = [];
Object.assign(window, { modelDiscoveryProbe });
const installProbe = {
  starts: 0,
  registry: null as string | null,
  installed: false,
};
Object.assign(window, { agentInstallProbe: installProbe });
const installJobs = new Map<
  string,
  { executor: BaseCodingAgent; logs: string }
>();
const installJobProbe = {
  startedExecutors: [] as BaseCodingAgent[],
  polledExecutors: [] as BaseCodingAgent[],
};
Object.assign(window, { agentInstallJobProbe: installJobProbe });
const missingProviders = (params.get('missing') ?? '').split(',');
export const useAppRuntime = () => params.get('runtime') ?? 'local';
export const useAuth = () => ({
  isLoaded: true,
  isSignedIn: params.get('signedOut') !== 'true',
});
export const useHostId = () =>
  new URLSearchParams(window.location.search).get('host');
const pairedHosts = [
  { host_id: 'remote-fixture', host_name: 'Remote fixture', paired_at: '' },
];
export const relayApi = {
  listPairedRelayHosts: async () => {
    calls.paired++;
    return pairedHosts;
  },
};
export const listPairedRelayHosts = async () => pairedHosts;
export const subscribeRelayPairingChanges = () => () => undefined;
export async function listRelayHosts() {
  calls.relay++;
  if (params.has('cloudError')) throw new Error('PRIVATE_CLOUD_ERROR');
  return [{ id: 'remote-fixture', name: 'Remote fixture', status: 'online' }];
}
export const createMachineClient = (_runtime: unknown, target: unknown) => ({
  ...machineClient,
  target,
});

const settingsProviderFor = (executor: BaseCodingAgent) => {
  return AGENT_PROVIDER_BY_EXECUTOR[executor].settingsProvider;
};

const garage = providers.map((executor) => ({
  executor,
  availability: { type: 'INSTALLATION_FOUND' as const },
  capabilities: [
    AgentProviderCapability.INITIAL_RUN,
    AgentProviderCapability.FOLLOW_UP,
    AgentProviderCapability.MCP,
    AgentProviderCapability.WORKFLOW_AGENT_STEP,
  ],
  policy: {
    executor,
    readiness: AgentProviderReadiness.READY,
    capabilities: [
      AgentProviderCapability.INITIAL_RUN,
      AgentProviderCapability.FOLLOW_UP,
      AgentProviderCapability.MCP,
      AgentProviderCapability.WORKFLOW_AGENT_STEP,
    ],
    legacy: false,
    disabled: false,
    diagnostics: [],
  },
}));

const toolProviders: AgentToolInventoryView['providers'] = AGENT_PROVIDERS.map(
  ({ toolProvider: provider }) => {
    return {
      provider,
      installed: true,
      mcp_scopes:
        provider === 'deepseek_harness' ? ['user'] : ['user', 'project'],
      skill_scopes: ['user', 'project'],
      items: [],
      limitations: [],
      errors: [],
    };
  }
);

const commandProviders: AgentCommandInventoryView['providers'] =
  toolProviders.map((entry) => ({
    ...entry,
    items: [],
    capabilities: {
      discoverable: entry.provider !== 'deepseek_harness',
      creatable: entry.provider !== 'deepseek_harness',
      supported_scopes:
        entry.provider === 'deepseek_harness' ? [] : ['user', 'project'],
      writable_formats:
        entry.provider === 'opencode' ? ['opencode_markdown'] : [],
    },
    limitations:
      entry.provider === 'deepseek_harness'
        ? ['Native prompt commands are not supported']
        : [],
  }));

const modelConfigByExecutor: Partial<
  Record<BaseCodingAgent, ModelSelectorConfig>
> = {
  [BaseCodingAgent.CODEX]: {
    providers: [],
    models: [
      {
        id: 'gpt-6.1-sol',
        name: 'GPT-6.1 Sol',
        reasoning_options: [
          { id: 'high', label: 'High', is_default: true },
          { id: 'max', label: 'Max', is_default: false },
        ],
      },
      {
        id: 'gpt-6.0',
        name: 'GPT-6.0',
        reasoning_options: [
          { id: 'high', label: 'High', is_default: true },
          { id: 'max', label: 'Max', is_default: false },
        ],
      },
    ],
    default_model: 'gpt-6.0',
    native_defaults: {
      model_id: 'gpt-6.1-sol',
      reasoning_id: 'max',
      model_reasoning: {},
    },
    catalog_source: 'native',
    agents: [],
    permissions: [],
  },
  [BaseCodingAgent.CLAUDE_CODE]: {
    providers: [],
    models: [
      {
        id: 'opus',
        name: 'Opus',
        reasoning_options: [
          { id: 'high', label: 'High', is_default: true },
          { id: 'xhigh', label: 'Extra High', is_default: false },
        ],
        capabilities: {
          reasoning: 'supported',
          input_modalities: ['text'],
          fast_mode: 'unknown',
          auto_mode: 'unknown',
          resolved_model: 'claude-opus-4-6',
          is_default: true,
          service_tiers: [],
        },
      },
      { id: 'sonnet', name: 'Sonnet', reasoning_options: [] },
    ],
    default_model: 'opus',
    native_defaults: {
      model_id: 'opus',
      reasoning_id: null,
      model_reasoning: { 'claude-opus-4-6': 'xhigh' },
    },
    catalog_source: 'native',
    agents: [],
    permissions: [],
  },
  [BaseCodingAgent.GEMINI]: {
    providers: [],
    models: [],
    default_model: null,
    agents: [],
    permissions: [],
  },
  [BaseCodingAgent.OPENCODE]: {
    providers: [{ id: 'FixtureProvider', name: 'Fixture Provider' }],
    models: [
      {
        id: 'FixtureModel',
        name: 'OpenCode Fixture Model',
        provider_id: 'FixtureProvider',
        reasoning_options: [
          { id: '', label: 'No variant', is_default: true },
          { id: 'high', label: 'High', is_default: false },
        ],
      },
    ],
    default_model: 'FixtureProvider/FixtureModel',
    native_defaults: {
      model_id: 'FixtureProvider/FixtureModel',
      reasoning_id: null,
      model_reasoning: {},
    },
    catalog_source: 'native',
    agents: [],
    permissions: [],
  },
  [BaseCodingAgent.DEEPSEEK_HARNESS]: {
    providers: [],
    models: [
      {
        id: 'FixtureDshModel',
        name: 'DeepSeek Harness Fixture Model',
        reasoning_options: [],
      },
    ],
    default_model: 'FixtureDshModel',
    native_defaults: {
      model_id: 'FixtureDshModel',
      reasoning_id: null,
      model_reasoning: {},
    },
    catalog_source: 'configured',
    agents: [],
    permissions: [],
  },
};

const managementProbe = {
  createdTools: [] as CreateAgentToolRequest[],
  createdCommands: [] as CreateAgentCommandRequest[],
  discoveredSettings: [] as AgentSettingsProvider[],
  savedProfiles: [] as string[],
};
Object.assign(window, { agentManagementProbe: managementProbe });

function settingsSnapshot(provider: AgentSettingsProvider): SettingsSnapshot {
  const fileId = `${provider}/fixture-config`;
  const nativeModel =
    modelConfigByExecutor[agentProviderForSettings(provider).executor]
      ?.default_model ??
    `${agentProviderForSettings(provider).label} fixture-model`;
  const keys = ['model', 'api_address'].map((name) => ({
    namespace: 'common',
    name,
  }));
  return {
    provider,
    installed: true,
    provider_version: 'fixture-1.0',
    schema_revision: 'fixture-revision',
    capabilities: {
      readable: true,
      native_writable: true,
      profile_storage: true,
      per_run_overrides: true,
    },
    descriptors: keys.map((key) => ({
      key,
      section: SettingSection.general,
      label:
        key.name === 'model' ? 'Fixture native model' : 'Fixture API address',
      description: 'Native configuration fixture',
      value_type: SettingValueType.string,
      control: SettingControl.text,
      options: [],
      validation: {},
      supported_scopes: [SettingScope.user, SettingScope.project],
      capabilities: {
        readable: true,
        writable: true,
        resettable: true,
        profile_storable: true,
        run_override: true,
      },
      native_locations: [
        { file_id: fileId, scope: SettingScope.user, native_path: [key.name] },
      ],
      activation: SettingActivation.next_session,
      sensitive: false,
    })),
    native_files: [
      {
        file_id: fileId,
        format: NativeConfigFormat.json,
        scope: SettingScope.user,
        exists: true,
        parse_status: NativeParseStatus.parsed,
        revision: 'fixture-native-revision',
        writable: true,
        managed_setting_keys: keys,
      },
    ],
    effective_settings: [
      {
        key: { namespace: 'common', name: 'model' },
        sources: [
          {
            source: 'native_user',
            scope: SettingScope.user,
            file_id: fileId,
            value: nativeModel,
            configured: true,
            revision: 'fixture-native-revision',
          },
        ],
        effective_value: nativeModel,
        effective_source: 'native_user',
        configured: true,
        warnings: [],
      },
      {
        key: { namespace: 'common', name: 'api_address' },
        sources: [],
        effective_value:
          'https://api.fixture.example.com/v1/agent-runtime/configuration',
        effective_source: 'native_user',
        configured: true,
        warnings: [],
      },
    ],
    unknown_native_nodes: [],
    limitations: [],
    errors: [],
  };
}

const settingsInventory = {
  providers: providers.map((executor) =>
    settingsSnapshot(settingsProviderFor(executor))
  ),
  errors: [],
};

export const machineClient = {
  target: {
    kind: 'local' as const,
    id: 'local' as const,
    apiHostId: null,
    label: 'This machine',
  },
  queryScopeKey: ['machine', 'local'] as const,
  getAgentGarage: async () => {
    calls.garage++;
    if (params.has('scanError') && calls.garage > 1)
      throw new Error('PRIVATE_SCAN_ERROR');
    return garage.map((entry) =>
      missingProviders.includes(entry.executor) && !installProbe.installed
        ? {
            ...entry,
            availability: { type: 'NOT_FOUND' as const },
            policy: {
              ...entry.policy,
              readiness: AgentProviderReadiness.MISSING_EXECUTABLE,
            },
          }
        : entry
    );
  },
  startAgentInstall: async (request: {
    executor: BaseCodingAgent;
    npm_registry?: string | null;
  }) => {
    installProbe.starts++;
    installProbe.registry = request.npm_registry ?? null;
    installJobProbe.startedExecutors.push(request.executor);
    const id = `install-fixture-${request.executor}-${installProbe.starts}`;
    const logs = `${request.executor}: downloading fixture installer`;
    installJobs.set(id, { executor: request.executor, logs });
    return {
      id,
      executor: request.executor,
      status: 'running',
      logs,
      error: null,
    };
  },
  getAgentInstall: async (id: string) => {
    const job = installJobs.get(id);
    if (!job) throw new Error('Unknown fixture installation job');
    installJobProbe.polledExecutors.push(job.executor);
    if (params.has('installPending')) {
      return { id, ...job, status: 'running', error: null };
    }
    const failed = params.has('installFails');
    if (!failed) installProbe.installed = true;
    return {
      id,
      executor: job.executor,
      status: failed ? 'failed' : 'succeeded',
      logs: 'Fixture installation log',
      error: failed ? 'Fixture download failed' : null,
    };
  },
  listAgentTools: async () => ({ providers: toolProviders, errors: [] }),
  listAgentCommands: async () => ({ providers: commandProviders, errors: [] }),
  discoverAgentSettings: async (request?: {
    provider?: AgentSettingsProvider;
  }) => {
    if (request?.provider)
      managementProbe.discoveredSettings.push(request.provider);
    return settingsInventory;
  },
  createAgentTool: async (request: CreateAgentToolRequest) => {
    managementProbe.createdTools.push(request);
    const item: AgentToolView = {
      ...request.target,
      installation_id: `fixture-${request.target.name}`,
      state: 'enabled',
      revision: 'fixture-revision',
      capabilities: {
        editable: true,
        removable: true,
        toggleable: true,
        exportable: true,
        installable: true,
      },
      definition:
        request.definition.type === 'skill'
          ? {
              type: 'skill',
              data: {
                contract_configured: true,
                file_count: 1,
                has_assets: false,
              },
            }
          : {
              type: 'mcp_server',
              data: {
                transport: request.definition.data.transport,
                command_configured: true,
                args_count: 0,
                cwd_configured: false,
                url_configured: false,
                env_count: 0,
                header_count: 0,
                has_provider_extensions: false,
              },
            },
    };
    toolProviders
      .find((entry) => entry.provider === request.target.provider)
      ?.items.push(item);
    return item;
  },
  createAgentCommand: async (request: CreateAgentCommandRequest) => {
    managementProbe.createdCommands.push(request);
    return undefined;
  },
  listAgentSettingsProfiles: async () => [],
  getConfig: async () => ({ config: configValue }),
  saveConfig: async (config: unknown) => config,
  // The remaining methods are only exercised by the detail tabs. Keeping
  // them available makes the mock a complete MachineClient boundary.
  updateAndSaveConfig: async () => true,
};

export function formatAgentSettingOperationError(error: unknown): string {
  return error instanceof Error ? error.message : 'Fixture operation failed';
}

export const profilesApi = {
  save: async (value: string) => {
    managementProbe.savedProfiles.push(value);
  },
};

export const agentsApi = {
  getGarage: machineClient.getAgentGarage,
  getPresetOptions: async ({
    executor,
    variant,
  }: {
    executor: BaseCodingAgent;
    variant: string | null;
  }): Promise<ExecutorConfig> => ({ executor, variant }),
};

export function useModelSelectorConfig(
  executor?: BaseCodingAgent | null,
  context?: ExecutorDiscoveryContext
) {
  const [refreshCount, setRefreshCount] = React.useState(0);
  const hostId = useHostId();
  React.useEffect(() => {
    modelDiscoveryProbe.push({ executor, hostId, context, refreshCount });
  }, [executor, hostId, context, refreshCount]);
  let config = executor
    ? (modelConfigByExecutor[executor] ?? {
        providers: [],
        models: [],
        default_model: null,
        agents: [],
        permissions: [],
      })
    : null;
  const params = new URLSearchParams(window.location.search);
  if (config && executor === BaseCodingAgent.GEMINI) {
    if (params.has('geminiConfigured')) {
      config = {
        ...config,
        catalog_source: 'configured',
        native_defaults: {
          model_id: refreshCount > 0 ? 'gemini-refreshed' : 'gemini-configured',
          reasoning_id: null,
          model_reasoning: {},
        },
      };
    }
    if (params.has('geminiCatalog')) {
      config = {
        ...config,
        catalog_source: 'observed',
        models: [
          {
            id: 'gemini-native-choice',
            name: 'Native model choice',
            reasoning_options: [],
            capabilities: {
              reasoning: 'unknown',
              input_modalities: ['text', 'image'],
              fast_mode: 'unknown',
              auto_mode: 'unknown',
              resolved_model: null,
              is_default: false,
              service_tiers: [],
            },
          },
        ],
      };
    }
  }
  if (config && executor === BaseCodingAgent.CLAUDE_CODE) {
    if (params.has('claudeAliasOnly') || params.has('claudeNativeName')) {
      config = {
        ...config,
        native_defaults: {
          model_id: 'opus',
          reasoning_id: null,
          model_reasoning: {},
        },
        models: config.models.map((model) => ({
          ...model,
          name:
            model.id === 'opus' && params.has('claudeNativeName')
              ? 'Opus 4.6'
              : model.name,
          capabilities: model.capabilities && {
            ...model.capabilities,
            resolved_model: null,
          },
        })),
      };
    }
    if (params.has('claudeSearch')) {
      config = {
        ...config,
        providers: [
          { id: 'native', name: 'Native' },
          { id: 'other', name: 'Other' },
        ],
        native_defaults: {
          model_id: 'native/opus',
          reasoning_id: null,
          model_reasoning: {},
        },
        models: config.models.map((model) => ({
          ...model,
          provider_id: 'native',
        })),
      };
    }
  }
  if (
    config &&
    executor === BaseCodingAgent.CLAUDE_CODE &&
    params.has('nativeAlias')
  ) {
    config = {
      ...config,
      native_defaults: {
        model_id: null,
        reasoning_id: null,
        model_reasoning: { 'claude-opus-4-6': 'xhigh' },
      },
    };
  }
  if (config && executor === BaseCodingAgent.CODEX) {
    if (params.has('codexConfigError')) {
      config = {
        ...config,
        models: config.models.map((model) => ({
          ...model,
          reasoning_options: [
            { id: 'high', label: 'High', is_default: true },
            { id: 'xhigh', label: 'Extra High', is_default: false },
          ],
          capabilities: {
            reasoning: 'supported',
            input_modalities:
              model.id === 'gpt-6.1-sol' ? ['text', 'image'] : ['text'],
            fast_mode: 'unknown',
            auto_mode: 'unknown',
            resolved_model: null,
            is_default: false,
            service_tiers: [],
          },
        })),
      };
    }
    if (params.has('builtin')) {
      config = {
        ...config,
        native_defaults: undefined,
        catalog_source: 'builtin',
      };
    } else if (params.has('nativeMissing')) {
      config = {
        ...config,
        models: config.models.filter((model) => model.id !== 'gpt-6.1-sol'),
        catalog_source: 'configured',
      };
    } else if (params.has('refreshNative') && refreshCount > 0) {
      config = {
        ...config,
        native_defaults: {
          model_id: 'gpt-6.1-sol',
          reasoning_id: 'high',
          model_reasoning: {},
        },
      };
    }
  }
  const discoveryError =
    refreshCount === 0
      ? executor === BaseCodingAgent.CODEX && params.has('codexConfigError')
        ? 'Codex native configuration discovery failed: I/O error: config/read request failed: invalid configuration: unknown variant `max`, expected one of `none`, `minimal`, `low`, `medium`, `high`, `xhigh` in `model_reasoning_effort`'
        : executor === BaseCodingAgent.CODEX && params.has('codexError')
          ? 'Codex model discovery failed: native request timed out.'
          : executor === BaseCodingAgent.GEMINI && params.has('geminiError')
            ? 'Gemini model discovery failed: host connection unavailable.'
            : null
      : null;
  if (discoveryError && params.has('noDiscoveryConfig')) config = null;
  return {
    config,
    refresh: () => setRefreshCount((count) => count + 1),
    loadingModels: Boolean(
      discoveryError && params.has('discoveryLoadingError')
    ),
    loadingAgents: false,
    error: discoveryError,
    isConnected: true,
    isInitialized: true,
  };
}

export function useSettingsNavigation() {
  return { openAgentCenter: () => undefined };
}

const configValue = {
  config_version: 'fixture',
  theme: 'system',
  executor_profile: { executor: BaseCodingAgent.CODEX, variant: null },
  disclaimer_acknowledged: true,
  onboarding_acknowledged: true,
  remote_onboarding_acknowledged: true,
  notifications: {},
  editor: {},
  github: {},
  analytics_enabled: false,
  workspace_dir: null,
  last_app_version: null,
  show_release_notes: false,
  language: 'en',
  git_branch_prefix: '',
  showcases: {},
  pr_auto_description_enabled: false,
  pr_auto_description_prompt: null,
  commit_reminder_enabled: false,
  commit_reminder_prompt: null,
  send_message_shortcut: 'enter',
  relay_enabled: false,
  host_nickname: null,
  hidden_agents: [],
};

export function useSettingsHost() {
  return {
    availableHosts: [
      machineClient.target,
      {
        kind: 'remote' as const,
        id: 'remote-fixture',
        apiHostId: 'remote-fixture',
        label: 'Remote fixture',
        status: 'online' as const,
      },
    ],
    hostsResolved: true,
    hostDiscovery: {
      hasCanonicalData: true,
      isLoading: false,
      isRetrying: false,
      error: null,
      canRetry: true,
      retry: async () => undefined,
    },
    selectedHostId: 'local',
    selectedHost: machineClient.target,
    setSelectedHostId: () => undefined,
  };
}

export function useSettingsMachineClient() {
  return machineClient as never;
}

export function useSettingsDirty() {
  return {
    isDirty: false,
    setDirty: () => undefined,
    clearAll: () => undefined,
  };
}

export function useSettingsMachineState() {
  return {
    hasCanonicalData: true,
    isLoading: false,
    isRetrying: false,
    error: null,
    canMutate: true,
    retry: async () => undefined,
  };
}

const fixtureProfiles = Object.fromEntries(
  providers.map((executor) => [
    executor,
    {
      DEFAULT: { [executor]: {} },
      ...(executor === BaseCodingAgent.CODEX
        ? {
            recently_used_models: {
              models: ['gpt-6.1-sol'],
              reasoning_by_model: { 'gpt-6.1-sol': 'high' },
            },
          }
        : {}),
    },
  ])
);

export function useUserSystem() {
  return {
    config: configValue,
    updateAndSaveConfig: async () => true,
    profiles: fixtureProfiles,
    setProfiles: () => undefined,
    reloadSystem: async () => undefined,
  };
}

export function useBlocker() {
  return {
    status: 'unblocked' as const,
    proceed: () => undefined,
    reset: () => undefined,
  };
}

export function useLocation() {
  return { pathname: '/' };
}

export function AgentIcon({ agent }: { agent: BaseCodingAgent }) {
  return <span aria-hidden="true" data-agent-icon={agent} />;
}

export function getAgentName(agent?: BaseCodingAgent | null) {
  return agent ? AGENT_PROVIDER_BY_EXECUTOR[agent].label : 'Agent';
}

export function useOptionalAgentRunSession() {
  return null;
}

export function LoadingState({ title }: { title: React.ReactNode }) {
  return <div role="status">{title}</div>;
}

export function EmptyState({
  title,
  description,
}: {
  title: React.ReactNode;
  description?: React.ReactNode;
}) {
  return (
    <div role="status">
      <strong>{title}</strong>
      {description}
    </div>
  );
}

export function ErrorState({
  title,
  description,
  action,
}: {
  title: React.ReactNode;
  description?: React.ReactNode;
  action?: React.ReactNode;
}) {
  return (
    <div role="alert">
      <strong>{title}</strong>
      {description}
      {action}
    </div>
  );
}

export function OfflineState({
  title,
  description,
}: {
  title: React.ReactNode;
  description?: React.ReactNode;
}) {
  return (
    <div role="status">
      <strong>{title}</strong>
      {description}
    </div>
  );
}

export function DegradedState({
  title,
  description,
  action,
}: {
  title: React.ReactNode;
  description?: React.ReactNode;
  action?: React.ReactNode;
}) {
  return (
    <div role="status">
      <strong>{title}</strong>
      {description}
      {action}
    </div>
  );
}

export const ConfirmDialog = { show: async () => 'cancelled' as const };
