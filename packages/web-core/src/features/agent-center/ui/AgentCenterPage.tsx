import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useBlocker } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import {
  ArrowClockwiseIcon,
  CheckCircleIcon,
  SpinnerIcon,
  WarningCircleIcon,
} from '@phosphor-icons/react';
import {
  AgentProviderDiagnosticKind,
  AgentProviderReadiness,
  AgentSettingsProvider,
  BaseCodingAgent,
  type AgentGarageEntry,
  type AgentProviderCapability,
} from 'shared/types';
import { ConfirmDialog } from '@vibe/ui/components/ConfirmDialog';
import {
  DegradedState,
  EmptyState,
  ErrorState,
  LoadingState,
  OfflineState,
} from '@vibe/ui/components/StateSurface';
import { AgentIcon } from '@/shared/components/AgentIcon';
import { AgentConfigurationSettingsPanel } from '@/shared/dialogs/settings/settings/AgentConfigurationSettingsPanel';
import { AgentToolsSettingsSection } from '@/shared/dialogs/settings/settings/AgentToolsSettingsSection';
import { AgentCommandsSettingsSection } from '../AgentCommandsSettingsSection';
import {
  advanceAgentCenterScope,
  agentCenterScopeIdentity,
  canPublishAgentCenterOperation,
  isAgentCenterScopeCurrent,
  projectAgentCenterSource,
  type AgentCenterScopeEpoch,
  type AgentCenterSourceProjection,
  type AgentCenterSourceState,
} from '../model/agentCenterState';
import {
  useSettingsHost,
  useSettingsMachineClient,
} from '@/shared/dialogs/settings/settings/SettingsHostContext';
import { useSettingsDirty } from '@/shared/dialogs/settings/settings/SettingsDirtyContext';
import { useSettingsMachineState } from '@/shared/dialogs/settings/settings/SettingsMachineUserSystemProvider';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { isAgentProviderReady } from '@/shared/lib/agentProviderOptions';
import { effectiveStringSetting } from '@/shared/lib/agentSettingsModel';
import {
  AGENT_PROVIDERS as PROVIDERS,
  getAgentBrandAttributes,
  type AgentProviderDefinition as ProviderDefinition,
} from '@/shared/lib/agentProviders';
import './agent-center.css';
import { AgentInstallPanel } from './AgentInstallPanel';

type AgentCenterTab = 'providers' | 'mcp' | 'skills' | 'commands' | 'profiles';

type SummaryState = AgentCenterSourceState;

type RefreshSource = 'garage' | 'tools' | 'commands' | 'settings' | 'config';

type RefreshDiagnostic = {
  source: RefreshSource;
  message: string;
};

type RefreshStatus = {
  scope: AgentCenterScopeEpoch;
  pending: boolean;
  diagnostics: RefreshDiagnostic[];
};

type DefaultMutationOwner = {
  client: ReturnType<typeof useSettingsMachineClient>;
  scope: AgentCenterScopeEpoch;
  executor: BaseCodingAgent;
  canMutate: boolean;
};

const TABS: AgentCenterTab[] = [
  'providers',
  'mcp',
  'skills',
  'commands',
  'profiles',
];

function garageEntryFor(
  garage: AgentGarageEntry[] | undefined,
  executor: BaseCodingAgent
) {
  return garage?.find((entry) => entry.executor === executor) ?? null;
}

function readinessFor(entry: AgentGarageEntry | null): AgentProviderReadiness {
  if (entry?.policy) return entry.policy.readiness;
  if (entry?.availability.type === 'NOT_FOUND') {
    return AgentProviderReadiness.MISSING_EXECUTABLE;
  }
  if (entry) return AgentProviderReadiness.INSTALLED;
  return AgentProviderReadiness.UNKNOWN;
}

function capabilityKey(capability: AgentProviderCapability): string {
  return `agentCenter.capability.${capability.toLowerCase()}`;
}

function readinessTone(readiness: AgentProviderReadiness) {
  if (isAgentProviderReady(readiness)) return 'ready';
  if (readiness === AgentProviderReadiness.DEGRADED) return 'warning';
  return 'unavailable';
}

function readinessKey(readiness: AgentProviderReadiness): string {
  return `agentCenter.readiness.${readiness.toLowerCase()}`;
}

function inventoryPayloadErrors(
  inventory:
    | {
        errors: Array<{ provider: string; message: string }>;
        providers: Array<{ provider: string; errors: string[] }>;
      }
    | undefined,
  provider: string
): string[] {
  if (!inventory) return [];
  return [
    ...inventory.errors
      .filter((error) => error.provider === provider)
      .map((error) => error.message),
    ...(inventory.providers.find((entry) => entry.provider === provider)
      ?.errors ?? []),
  ];
}

function settingsPayloadErrors(
  inventory:
    | {
        errors: Array<{ provider: AgentSettingsProvider; message: string }>;
        providers: Array<{
          provider: AgentSettingsProvider;
          errors: Array<{ message: string }>;
        }>;
      }
    | undefined,
  provider: AgentSettingsProvider
): string[] {
  if (!inventory) return [];
  return [
    ...inventory.errors
      .filter((error) => error.provider === provider)
      .map((error) => error.message),
    ...(inventory.providers
      .find((entry) => entry.provider === provider)
      ?.errors.map((error) => error.message) ?? []),
  ];
}

export function AgentCenterPage({
  initialExecutor,
}: { initialExecutor?: BaseCodingAgent } = {}) {
  const { t } = useTranslation('common');
  const {
    availableHosts,
    hostDiscovery,
    remoteHostDiscovery,
    selectedHost,
    selectedHostId,
    setSelectedHostId,
  } = useSettingsHost();
  const { clearAll: clearDirty, isDirty } = useSettingsDirty();
  const machineClient = useSettingsMachineClient();
  const machineState = useSettingsMachineState();
  const { config, updateAndSaveConfig } = useUserSystem();
  const [activeTab, setActiveTab] = useState<AgentCenterTab>('providers');
  const [selectedExecutor, setSelectedExecutor] = useState<BaseCodingAgent>(
    initialExecutor ?? BaseCodingAgent.CODEX
  );
  useEffect(() => {
    if (initialExecutor) setSelectedExecutor(initialExecutor);
  }, [initialExecutor]);
  const [refreshStatus, setRefreshStatus] = useState<RefreshStatus | null>(
    null
  );
  const [defaultMutation, setDefaultMutation] = useState<{
    sequence: number;
    owner: DefaultMutationOwner;
  } | null>(null);
  const [actionError, setActionError] = useState<{
    scope: AgentCenterScopeEpoch;
    message: string;
  } | null>(null);
  const navigationConfirmationPending = useRef(false);
  const allowNavigationRef = useRef(false);
  const refreshPendingRef = useRef<AgentCenterScopeEpoch | null>(null);
  const defaultMutationSequence = useRef(0);
  const defaultMutationPendingRef = useRef<number | null>(null);
  const mountedRef = useRef(true);
  const navigationBlocker = useBlocker({
    shouldBlockFn: () => isDirty && !allowNavigationRef.current,
    enableBeforeUnload: false,
    withResolver: true,
  });

  const selectedProvider =
    PROVIDERS.find((provider) => provider.executor === selectedExecutor) ??
    PROVIDERS[0];
  const hostAvailable = Boolean(
    machineClient &&
      selectedHost &&
      (selectedHost.kind === 'local' || selectedHost.status !== 'offline')
  );
  const queryPrefix = machineClient?.queryScopeKey ?? ['machine', 'unselected'];
  const scopeIdentity = agentCenterScopeIdentity(
    machineClient?.queryScopeKey,
    selectedProvider.settingsProvider,
    activeTab
  );
  const activeScopeRef = useRef<AgentCenterScopeEpoch>({
    identity: scopeIdentity,
    epoch: 0,
  });
  const nextScope = advanceAgentCenterScope(
    activeScopeRef.current,
    scopeIdentity
  );
  if (nextScope !== activeScopeRef.current) {
    activeScopeRef.current = nextScope;
    refreshPendingRef.current = null;
  }
  const machineClientRef = useRef(machineClient);
  machineClientRef.current = machineClient;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      refreshPendingRef.current = null;
      defaultMutationPendingRef.current = null;
    };
  }, []);

  const garageQuery = useQuery({
    queryKey: [...queryPrefix, 'agent-center', 'garage'],
    queryFn: () => machineClient!.getAgentGarage(),
    enabled: hostAvailable,
    staleTime: 60_000,
  });
  const toolsQuery = useQuery({
    queryKey: [...queryPrefix, 'agent-center', 'tools'],
    queryFn: () => machineClient!.listAgentTools(),
    enabled: hostAvailable,
    staleTime: 30_000,
  });
  const commandsQuery = useQuery({
    queryKey: [...queryPrefix, 'agent-center', 'commands'],
    queryFn: () => machineClient!.listAgentCommands(),
    enabled: hostAvailable,
    staleTime: 30_000,
  });
  const settingsQuery = useQuery({
    queryKey: [
      ...queryPrefix,
      'agent-center',
      'settings-summary',
      selectedProvider.settingsProvider,
    ],
    queryFn: () =>
      machineClient!.discoverAgentSettings({
        provider: selectedProvider.settingsProvider,
      }),
    enabled: hostAvailable,
    staleTime: 30_000,
  });

  const selectedGarageEntry = garageEntryFor(
    garageQuery.data,
    selectedExecutor
  );
  const selectedSnapshot = settingsQuery.data?.providers.find(
    (provider) => provider.provider === selectedProvider.settingsProvider
  );
  const defaultModel = useMemo(
    () => effectiveStringSetting(selectedSnapshot, 'common', 'model'),
    [selectedSnapshot]
  );
  const apiAddress = useMemo(
    () => effectiveStringSetting(selectedSnapshot, 'common', 'api_address'),
    [selectedSnapshot]
  );
  const selectedToolInventory = toolsQuery.data?.providers.find(
    (provider) => provider.provider === selectedProvider.toolProvider
  );
  const selectedCommandInventory = commandsQuery.data?.providers.find(
    (provider) => provider.provider === selectedProvider.commandProvider
  );
  const toolPayloadErrors = inventoryPayloadErrors(
    toolsQuery.data,
    selectedProvider.toolProvider
  );
  const selectedSettingsPayloadErrors = settingsPayloadErrors(
    settingsQuery.data,
    selectedProvider.settingsProvider
  );
  const commandPayloadErrors = inventoryPayloadErrors(
    commandsQuery.data,
    selectedProvider.commandProvider
  );
  const garageProjection = projectAgentCenterSource({
    hasCanonicalData: garageQuery.data !== undefined,
    isLoading: garageQuery.isLoading,
    isFetching: garageQuery.isFetching,
    error: garageQuery.error,
    capabilityAvailable: selectedGarageEntry !== null,
  });
  const toolsProjection = projectAgentCenterSource({
    hasCanonicalData: selectedToolInventory !== undefined,
    isLoading: toolsQuery.isLoading,
    isFetching: toolsQuery.isFetching,
    error: toolsQuery.error,
    diagnosticCount: toolPayloadErrors.length,
    capabilityAvailable: Boolean(selectedToolInventory?.installed),
  });
  const settingsProjection = projectAgentCenterSource({
    hasCanonicalData: selectedSnapshot !== undefined,
    isLoading: settingsQuery.isLoading,
    isFetching: settingsQuery.isFetching,
    error: settingsQuery.error,
    diagnosticCount: selectedSettingsPayloadErrors.length,
    capabilityAvailable: Boolean(selectedSnapshot?.installed),
  });
  const commandsProjection = projectAgentCenterSource({
    hasCanonicalData: selectedCommandInventory !== undefined,
    isLoading: commandsQuery.isLoading,
    isFetching: commandsQuery.isFetching,
    error: commandsQuery.error,
    diagnosticCount: commandPayloadErrors.length,
    capabilityAvailable: Boolean(selectedCommandInventory?.installed),
  });
  const configProjection = projectAgentCenterSource({
    hasCanonicalData: machineState.hasCanonicalData,
    isLoading: machineState.isLoading,
    isFetching: machineState.isRetrying,
    error: machineState.error,
    capabilityAvailable: config !== null,
  });
  const toolsState: SummaryState = toolsProjection.state;
  const settingsState: SummaryState = settingsProjection.state;
  const commandsState: SummaryState = commandsProjection.state;
  const mcpItems =
    selectedToolInventory?.items.filter((item) => item.kind === 'mcp_server') ??
    [];
  const skillItems =
    selectedToolInventory?.items.filter((item) => item.kind === 'skill') ?? [];
  const commandItems = selectedCommandInventory?.items ?? [];
  const isDefault =
    config?.executor_profile?.executor === selectedProvider.executor;
  const currentRefreshStatus =
    refreshStatus &&
    isAgentCenterScopeCurrent(refreshStatus.scope, activeScopeRef.current)
      ? refreshStatus
      : null;
  const currentActionError =
    actionError &&
    isAgentCenterScopeCurrent(actionError.scope, activeScopeRef.current)
      ? actionError.message
      : null;
  const rescanning = Boolean(currentRefreshStatus?.pending);
  const currentDefaultMutation =
    defaultMutation &&
    isAgentCenterScopeCurrent(
      defaultMutation.owner.scope,
      activeScopeRef.current
    )
      ? defaultMutation
      : null;
  const defaultSaving = currentDefaultMutation !== null;
  const defaultOwnerRef = useRef<DefaultMutationOwner>({
    client: machineClient,
    scope: activeScopeRef.current,
    executor: selectedProvider.executor,
    canMutate: false,
  });
  defaultOwnerRef.current = {
    client: machineClient,
    scope: activeScopeRef.current,
    executor: selectedProvider.executor,
    canMutate:
      machineState.canMutate &&
      configProjection.canMutate &&
      garageProjection.hasUsableData &&
      isAgentProviderReady(readinessFor(selectedGarageEntry)),
  };

  const confirmDiscard = useCallback(async (): Promise<boolean> => {
    if (navigationConfirmationPending.current) return false;
    navigationConfirmationPending.current = true;
    try {
      const result = await ConfirmDialog.show({
        title: t('agentCenter.unsaved.title'),
        message: t('agentCenter.unsaved.message'),
        confirmText: t('agentCenter.unsaved.discard'),
        cancelText: t('agentCenter.unsaved.cancel'),
        variant: 'destructive',
      });
      return result === 'confirmed';
    } finally {
      navigationConfirmationPending.current = false;
    }
  }, [t]);

  const runAfterDirtyConfirmation = useCallback(
    async (action: () => void) => {
      const expectedScope = activeScopeRef.current;
      if (!isDirty) {
        action();
        return;
      }
      if (await confirmDiscard()) {
        if (!isAgentCenterScopeCurrent(expectedScope, activeScopeRef.current)) {
          return;
        }
        clearDirty();
        action();
      }
    },
    [clearDirty, confirmDiscard, isDirty]
  );

  useEffect(() => {
    if (!isDirty) return;
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', handleBeforeUnload);
    return () => window.removeEventListener('beforeunload', handleBeforeUnload);
  }, [isDirty]);

  useEffect(() => {
    if (
      navigationBlocker.status !== 'blocked' ||
      navigationConfirmationPending.current
    ) {
      return;
    }
    let cancelled = false;
    void confirmDiscard().then((discard) => {
      if (cancelled || navigationBlocker.status !== 'blocked') return;
      if (discard) {
        allowNavigationRef.current = true;
        clearDirty();
        navigationBlocker.proceed();
        queueMicrotask(() => {
          allowNavigationRef.current = false;
        });
      } else {
        navigationBlocker.reset();
      }
    });
    return () => {
      cancelled = true;
    };
  }, [clearDirty, confirmDiscard, navigationBlocker]);

  const refreshSourceMessage = (source: RefreshSource): string => {
    switch (source) {
      case 'garage':
        return t('agentCenter.errors.loadProviders');
      case 'tools':
        return t('agentCenter.errors.loadTools');
      case 'commands':
      case 'settings':
      case 'config':
        return t('agentCenter.errors.rescan');
    }
  };

  const refreshSourceLabel = (source: RefreshSource): string => {
    switch (source) {
      case 'garage':
        return t('agentCenter.tabs.providers');
      case 'tools':
        return t('agentCenter.enabledTools');
      case 'commands':
        return t('agentCenter.tabs.commands');
      case 'settings':
        return t('agentCenter.nativeSummary');
      case 'config':
        return t('agentCenter.tabs.profiles');
    }
  };

  const runRefresh = async (sources: RefreshSource[]) => {
    const scope = activeScopeRef.current;
    if (
      refreshPendingRef.current &&
      isAgentCenterScopeCurrent(refreshPendingRef.current, scope)
    ) {
      return;
    }

    const client = machineClient;
    if (!client || !hostAvailable) return;

    refreshPendingRef.current = scope;
    setRefreshStatus({ scope, pending: true, diagnostics: [] });
    setActionError(null);

    const attempts = sources.map(async (source) => {
      switch (source) {
        case 'garage': {
          const result = await garageQuery.refetch();
          if (result.isError) {
            throw result.error ?? new Error(refreshSourceMessage(source));
          }
          return;
        }
        case 'tools': {
          const result = await toolsQuery.refetch();
          if (
            result.isError ||
            inventoryPayloadErrors(result.data, selectedProvider.toolProvider)
              .length > 0
          ) {
            throw result.error ?? new Error(refreshSourceMessage(source));
          }
          return;
        }
        case 'commands': {
          const result = await commandsQuery.refetch();
          if (
            result.isError ||
            inventoryPayloadErrors(
              result.data,
              selectedProvider.commandProvider
            ).length > 0
          ) {
            throw result.error ?? new Error(refreshSourceMessage(source));
          }
          return;
        }
        case 'settings': {
          const result = await settingsQuery.refetch();
          if (
            result.isError ||
            settingsPayloadErrors(
              result.data,
              selectedProvider.settingsProvider
            ).length > 0
          ) {
            throw result.error ?? new Error(refreshSourceMessage(source));
          }
          return;
        }
        case 'config':
          await machineState.retry();
      }
    });

    try {
      const results = await Promise.allSettled(attempts);
      if (
        !canPublishAgentCenterOperation(
          scope,
          activeScopeRef.current,
          mountedRef.current
        ) ||
        machineClientRef.current !== client
      ) {
        return;
      }
      const diagnostics = results.flatMap<RefreshDiagnostic>((result, index) =>
        result.status === 'rejected'
          ? [
              {
                source: sources[index],
                message: refreshSourceMessage(sources[index]),
              },
            ]
          : []
      );
      setRefreshStatus({ scope, pending: false, diagnostics });
    } finally {
      if (
        canPublishAgentCenterOperation(
          scope,
          activeScopeRef.current,
          mountedRef.current
        ) &&
        refreshPendingRef.current &&
        isAgentCenterScopeCurrent(refreshPendingRef.current, scope)
      ) {
        refreshPendingRef.current = null;
        setRefreshStatus((current) =>
          current && isAgentCenterScopeCurrent(current.scope, scope)
            ? { ...current, pending: false }
            : current
        );
      }
    }
  };

  const refreshToolSummary = async () => {
    await runRefresh(['tools']);
  };

  const refreshCommandSummary = async () => {
    await runRefresh(['commands']);
  };

  const ownerIsCurrent = (owner: DefaultMutationOwner): boolean => {
    const current = defaultOwnerRef.current;
    return (
      current.client === owner.client &&
      current.executor === owner.executor &&
      current.canMutate &&
      mountedRef.current &&
      isAgentCenterScopeCurrent(owner.scope, current.scope)
    );
  };

  const setDefaultProvider = async () => {
    const owner = defaultOwnerRef.current;
    if (
      !owner.canMutate ||
      defaultMutationPendingRef.current !== null ||
      !ownerIsCurrent(owner)
    ) {
      return;
    }

    const sequence = ++defaultMutationSequence.current;
    defaultMutationPendingRef.current = sequence;
    setDefaultMutation({ sequence, owner });
    setActionError(null);
    try {
      if (!ownerIsCurrent(owner)) return;
      const saved = await updateAndSaveConfig({
        executor_profile: {
          executor: owner.executor,
          variant: null,
        },
      });
      if (!saved) throw new Error(t('agentCenter.errors.setDefault'));
    } catch (error) {
      if (ownerIsCurrent(owner)) {
        setActionError({
          scope: owner.scope,
          message:
            error instanceof Error
              ? error.message
              : t('agentCenter.errors.setDefault'),
        });
      }
    } finally {
      if (defaultMutationPendingRef.current === sequence) {
        defaultMutationPendingRef.current = null;
        if (mountedRef.current) {
          setDefaultMutation((current) =>
            current?.sequence === sequence ? null : current
          );
        }
      }
    }
  };

  if (!hostDiscovery.hasCanonicalData && hostDiscovery.isLoading) {
    return (
      <section className="vk-agent-center" aria-label={t('agentCenter.title')}>
        <LoadingState title={t('agentCenter.states.loadingHosts')} />
      </section>
    );
  }

  if (!hostDiscovery.hasCanonicalData && hostDiscovery.error) {
    return (
      <section className="vk-agent-center" aria-label={t('agentCenter.title')}>
        <ErrorState
          title={t('agentCenter.title')}
          description={t('agentCenter.errors.loadRemoteHosts')}
          action={
            hostDiscovery.canRetry ? (
              <button
                type="button"
                className="vk-agent-center__state-action"
                disabled={hostDiscovery.isRetrying}
                onClick={() => void hostDiscovery.retry()}
              >
                {t('buttons.retry')}
              </button>
            ) : undefined
          }
        />
      </section>
    );
  }

  if (!selectedHost) {
    return (
      <section className="vk-agent-center" aria-label={t('agentCenter.title')}>
        <EmptyState
          title={t('agentCenter.title')}
          description={t('agentCenter.states.noHost')}
        />
      </section>
    );
  }

  const hostOffline =
    selectedHost.kind === 'remote' && selectedHost.status === 'offline';

  return (
    <section className="vk-agent-center" aria-labelledby="agent-center-title">
      <header className="vk-agent-center__header">
        <div>
          <p className="vk-agent-center__eyebrow">{t('agentCenter.eyebrow')}</p>
          <h1 id="agent-center-title">{t('agentCenter.title')}</h1>
          <p>{t('agentCenter.description')}</p>
        </div>
        <div className="vk-agent-center__header-actions">
          {availableHosts.length > 1 && (
            <label className="vk-agent-center__host-picker">
              <span>{t('agentCenter.host')}</span>
              <select
                value={selectedHostId ?? ''}
                onChange={(event) => {
                  const hostId = event.target.value;
                  if (hostId === selectedHostId) return;
                  // Keep the controlled selection on the canonical Host while
                  // the discard confirmation is open or gets cancelled.
                  event.currentTarget.value = selectedHostId ?? '';
                  void runAfterDirtyConfirmation(() =>
                    setSelectedHostId(hostId)
                  );
                }}
              >
                {availableHosts.map((host) => (
                  <option key={host.id} value={host.id}>
                    {host.label}
                    {host.status === 'offline'
                      ? ` · ${t('agentCenter.states.offline')}`
                      : ''}
                  </option>
                ))}
              </select>
            </label>
          )}
          <button
            type="button"
            className="vk-agent-center__rescan"
            disabled={!hostAvailable || rescanning}
            title={
              hostAvailable
                ? undefined
                : t('agentCenter.disabled.hostUnavailable')
            }
            onClick={() =>
              void runRefresh([
                'garage',
                'tools',
                'commands',
                'settings',
                'config',
              ])
            }
          >
            {rescanning ? (
              <SpinnerIcon
                className="vk-agent-center-spin"
                aria-hidden="true"
              />
            ) : (
              <ArrowClockwiseIcon aria-hidden="true" />
            )}
            {rescanning ? t('agentCenter.rescanning') : t('agentCenter.rescan')}
          </button>
        </div>
      </header>

      {Boolean(remoteHostDiscovery.error) && (
        <div role="status" className="vk-agent-center__header-actions">
          <span>{t('agentCenter.errors.loadRemoteHosts')}</span>
          {remoteHostDiscovery.canRetry && (
            <button
              type="button"
              className="vk-agent-center__state-action"
              disabled={remoteHostDiscovery.isRetrying}
              onClick={() => void remoteHostDiscovery.retry()}
            >
              {t('buttons.retry')}
            </button>
          )}
        </div>
      )}

      {currentRefreshStatus &&
        !currentRefreshStatus.pending &&
        currentRefreshStatus.diagnostics.length > 0 && (
          <DegradedState
            compact
            title={t('agentCenter.readiness.degraded')}
            description={
              <ul className="vk-agent-center__refresh-diagnostics">
                {currentRefreshStatus.diagnostics.map((diagnostic) => (
                  <li key={diagnostic.source}>
                    <strong>{refreshSourceLabel(diagnostic.source)}</strong>
                    <span>{diagnostic.message}</span>
                  </li>
                ))}
              </ul>
            }
          />
        )}

      <nav
        className="vk-agent-center__tabs"
        aria-label={t('agentCenter.tabsLabel')}
      >
        {TABS.map((tab) => (
          <button
            key={tab}
            type="button"
            aria-current={activeTab === tab ? 'page' : undefined}
            onClick={() => {
              if (activeTab === tab) return;
              void runAfterDirtyConfirmation(() => setActiveTab(tab));
            }}
          >
            {t(`agentCenter.tabs.${tab}`)}
          </button>
        ))}
      </nav>

      {hostOffline ? (
        <OfflineState
          title={t('agentCenter.states.hostOfflineTitle')}
          description={t('agentCenter.states.hostOffline')}
        />
      ) : (
        <div className="vk-agent-center__workspace">
          <aside
            className="vk-agent-center__providers"
            aria-label={t('agentCenter.providerListLabel')}
          >
            <p>{t('agentCenter.providerListTitle')}</p>
            <div>
              {PROVIDERS.map((provider) => {
                const entry = garageEntryFor(
                  garageQuery.data,
                  provider.executor
                );
                const readiness = readinessFor(entry);
                return (
                  <button
                    key={provider.executor}
                    type="button"
                    {...getAgentBrandAttributes(provider.executor)}
                    className="vk-agent-brand-choice"
                    data-active={provider.executor === selectedExecutor}
                    aria-pressed={provider.executor === selectedExecutor}
                    onClick={() => {
                      if (provider.executor === selectedExecutor) return;
                      void runAfterDirtyConfirmation(() =>
                        setSelectedExecutor(provider.executor)
                      );
                    }}
                  >
                    <AgentIcon
                      agent={provider.executor}
                      className="vk-agent-brand-icon h-7 w-7"
                    />
                    <span>{provider.label}</span>
                    <small data-tone={readinessTone(readiness)}>
                      {t(readinessKey(readiness))}
                    </small>
                  </button>
                );
              })}
            </div>
          </aside>

          <section className="vk-agent-center__content">
            {currentActionError && (
              <div className="vk-agent-center__alert" role="alert">
                <WarningCircleIcon aria-hidden="true" />
                {currentActionError}
              </div>
            )}
            {activeTab === 'providers' && (
              <>
                {hostAvailable &&
                  machineClient &&
                  garageProjection.canMutate &&
                  selectedGarageEntry?.availability.type === 'NOT_FOUND' && (
                    <AgentInstallPanel
                      key={`${scopeIdentity}:${activeScopeRef.current.epoch}:${selectedExecutor}`}
                      client={machineClient}
                      executor={selectedExecutor}
                      onRescan={() => runRefresh(['garage'])}
                    />
                  )}
                <ProviderOverview
                  provider={selectedProvider}
                  entry={selectedGarageEntry}
                  model={defaultModel.value}
                  modelSource={defaultModel.source}
                  apiAddress={apiAddress.value}
                  mcpEnabled={
                    mcpItems.filter((item) => item.state === 'enabled').length
                  }
                  mcpTotal={mcpItems.length}
                  skillsEnabled={
                    skillItems.filter((item) => item.state === 'enabled').length
                  }
                  skillsTotal={skillItems.length}
                  commandsEnabled={
                    commandItems.filter((item) => item.state === 'enabled')
                      .length
                  }
                  commandsTotal={commandItems.length}
                  toolsState={toolsState}
                  commandsState={commandsState}
                  settingsState={settingsState}
                  toolsProjection={toolsProjection}
                  commandsProjection={commandsProjection}
                  settingsProjection={settingsProjection}
                  configProjection={configProjection}
                  garageProjection={garageProjection}
                  isDefault={isDefault}
                  canSetDefault={
                    defaultOwnerRef.current.canMutate &&
                    defaultMutationPendingRef.current === null
                  }
                  defaultSaving={defaultSaving}
                  onRetryGarage={() => void runRefresh(['garage'])}
                  onRetryTools={() => void runRefresh(['tools'])}
                  onRetryCommands={() => void runRefresh(['commands'])}
                  onRetrySettings={() => void runRefresh(['settings'])}
                  onRetryConfig={() => void runRefresh(['config'])}
                  onSetDefault={() => void setDefaultProvider()}
                  onOpenConfiguration={() =>
                    void runAfterDirtyConfirmation(() =>
                      setActiveTab('profiles')
                    )
                  }
                  onOpenTools={() =>
                    void runAfterDirtyConfirmation(() => setActiveTab('mcp'))
                  }
                />
              </>
            )}
            {activeTab === 'mcp' && (
              <AgentToolsSettingsSection
                provider={selectedProvider.toolProvider}
                fixedKind="mcp_server"
                onInventoryChange={refreshToolSummary}
              />
            )}
            {activeTab === 'skills' && (
              <AgentToolsSettingsSection
                provider={selectedProvider.toolProvider}
                fixedKind="skill"
                onInventoryChange={refreshToolSummary}
              />
            )}
            {activeTab === 'commands' && (
              <AgentCommandsSettingsSection
                provider={selectedProvider.commandProvider}
                onInventoryChange={refreshCommandSummary}
              />
            )}
            {activeTab === 'profiles' && (
              <AgentConfigurationSettingsPanel
                executor={selectedProvider.executor}
                variant={null}
                includeTools={false}
              />
            )}
          </section>
        </div>
      )}
    </section>
  );
}

function ProviderOverview({
  provider,
  entry,
  model,
  modelSource,
  apiAddress,
  mcpEnabled,
  mcpTotal,
  skillsEnabled,
  skillsTotal,
  commandsEnabled,
  commandsTotal,
  toolsState,
  commandsState,
  settingsState,
  toolsProjection,
  commandsProjection,
  settingsProjection,
  configProjection,
  garageProjection,
  isDefault,
  canSetDefault,
  defaultSaving,
  onRetryGarage,
  onRetryTools,
  onRetryCommands,
  onRetrySettings,
  onRetryConfig,
  onSetDefault,
  onOpenConfiguration,
  onOpenTools,
}: {
  provider: ProviderDefinition;
  entry: AgentGarageEntry | null;
  model: string | null;
  modelSource: string | null;
  apiAddress: string | null;
  mcpEnabled: number;
  mcpTotal: number;
  skillsEnabled: number;
  skillsTotal: number;
  commandsEnabled: number;
  commandsTotal: number;
  toolsState: SummaryState;
  commandsState: SummaryState;
  settingsState: SummaryState;
  toolsProjection: AgentCenterSourceProjection;
  commandsProjection: AgentCenterSourceProjection;
  settingsProjection: AgentCenterSourceProjection;
  configProjection: AgentCenterSourceProjection;
  garageProjection: AgentCenterSourceProjection;
  isDefault: boolean;
  canSetDefault: boolean;
  defaultSaving: boolean;
  onRetryGarage: () => void;
  onRetryTools: () => void;
  onRetryCommands: () => void;
  onRetrySettings: () => void;
  onRetryConfig: () => void;
  onSetDefault: () => void;
  onOpenConfiguration: () => void;
  onOpenTools: () => void;
}) {
  const { t } = useTranslation('common');
  const readiness = readinessFor(entry);
  const hasGarageProviderData =
    garageProjection.hasUsableData && entry !== null;
  const capabilities = entry?.policy?.capabilities ?? [];
  const diagnostics = entry?.policy?.diagnostics ?? [];
  const configurationSource =
    modelSource === 'native_project'
      ? t('agentCenter.configurationSources.nativeProject')
      : modelSource === 'native_user'
        ? t('agentCenter.configurationSources.nativeUser')
        : t('agentCenter.adapterManaged');
  const providerReady = isAgentProviderReady(readiness);
  const defaultState = configProjection.state;
  const defaultUnavailableReason =
    defaultState === 'loading'
      ? t('agentCenter.disabled.configLoading')
      : defaultState !== 'ready'
        ? t('agentCenter.disabled.configUnavailable')
        : !providerReady
          ? t('agentCenter.disabled.providerUnavailable')
          : undefined;

  const summaryValue = (
    state: SummaryState,
    readyValue: string,
    unavailableValue = t('agentCenter.states.summaryUnavailable')
  ) => {
    switch (state) {
      case 'loading':
        return t('agentCenter.states.summaryLoading');
      case 'error':
        return t('agentCenter.states.summaryError');
      case 'unavailable':
        return unavailableValue;
      case 'degraded':
      case 'ready':
        return readyValue;
    }
  };

  return (
    <div className="vk-agent-center__overview">
      <div className="vk-agent-center__provider-heading">
        <div {...getAgentBrandAttributes(provider.executor)}>
          <p>{t('agentCenter.selectedProvider')}</p>
          <h2 className="vk-agent-center__provider-title vk-agent-brand-label">
            <AgentIcon
              agent={provider.executor}
              className="vk-agent-brand-icon h-8 w-8"
            />
            {provider.label}
          </h2>
          <p className="vk-agent-center__provider-trait">
            {t(`agentCenter.providerTraits.${provider.executor}`)}
          </p>
          <span
            data-tone={
              hasGarageProviderData ? readinessTone(readiness) : 'unavailable'
            }
          >
            {hasGarageProviderData ? (
              <>
                {t(readinessKey(readiness))}
                {' · '}
                {entry.bundled_version
                  ? t('agentCenter.bundledVersion', {
                      version: entry.bundled_version,
                    })
                  : t('agentCenter.versionUnavailable')}
              </>
            ) : garageProjection.state === 'loading' ? (
              t('agentCenter.states.loadingProvider')
            ) : garageProjection.state === 'error' ? (
              t('agentCenter.errors.loadProviders')
            ) : (
              t('agentCenter.states.summaryUnavailable')
            )}
          </span>
        </div>
        <div className="vk-agent-center__provider-actions">
          <button
            type="button"
            disabled={
              isDefault ||
              !canSetDefault ||
              defaultSaving ||
              defaultState !== 'ready'
            }
            title={defaultUnavailableReason}
            aria-label={
              defaultUnavailableReason
                ? `${t('agentCenter.setDefault')}: ${defaultUnavailableReason}`
                : undefined
            }
            onClick={onSetDefault}
          >
            {defaultSaving && (
              <SpinnerIcon
                className="vk-agent-center-spin"
                aria-hidden="true"
              />
            )}
            {isDefault
              ? t('agentCenter.defaultProvider')
              : t('agentCenter.setDefault')}
          </button>
          <button type="button" onClick={onOpenConfiguration}>
            {t('agentCenter.openConfiguration')}
          </button>
        </div>
      </div>

      {provider.developerPreview && (
        <p className="text-sm text-low">{t('agentCenter.developerPreview')}</p>
      )}

      {garageProjection.state === 'loading' ? (
        <LoadingState title={t('agentCenter.states.loadingProvider')} />
      ) : garageProjection.state === 'error' ? (
        <ErrorState
          title={t('agentCenter.errors.loadProviders')}
          action={
            <button
              type="button"
              className="vk-agent-center__state-action"
              onClick={onRetryGarage}
            >
              {t('buttons.retry')}
            </button>
          }
        />
      ) : garageProjection.state === 'unavailable' ? (
        <EmptyState
          title={t('agentCenter.states.summaryUnavailable')}
          description={t('agentCenter.errors.loadProviders')}
          action={
            <button
              type="button"
              className="vk-agent-center__state-action"
              onClick={onRetryGarage}
            >
              {t('buttons.retry')}
            </button>
          }
        />
      ) : garageProjection.state === 'degraded' ? (
        <DegradedState
          compact
          title={t('agentCenter.readiness.degraded')}
          description={t('agentCenter.errors.loadProviders')}
          action={
            <button
              type="button"
              className="vk-agent-center__state-action"
              onClick={onRetryGarage}
            >
              {t('buttons.retry')}
            </button>
          }
        />
      ) : null}

      <div className="vk-agent-center__source-states">
        <SourceProjectionNotice
          projection={toolsProjection}
          title={t('agentCenter.enabledTools')}
          description={t('agentCenter.errors.loadTools')}
          onRetry={onRetryTools}
        />
        <SourceProjectionNotice
          projection={commandsProjection}
          title={t('agentCenter.tabs.commands')}
          description={t('agentCenter.errors.rescan')}
          onRetry={onRetryCommands}
        />
        <SourceProjectionNotice
          projection={settingsProjection}
          title={t('agentCenter.nativeSummary')}
          description={t('agentCenter.errors.rescan')}
          onRetry={onRetrySettings}
        />
        <SourceProjectionNotice
          projection={configProjection}
          title={t('agentCenter.tabs.profiles')}
          description={t('agentCenter.errors.rescan')}
          onRetry={onRetryConfig}
        />
      </div>
      {diagnostics.length > 0 && (
        <div className="vk-agent-center__diagnostics">
          {diagnostics.map((diagnostic) => (
            <p key={`${diagnostic.kind}:${diagnostic.message}`}>
              <WarningCircleIcon aria-hidden="true" />
              {entry?.policy?.readiness === AgentProviderReadiness.INSTALLED &&
              diagnostic.kind === AgentProviderDiagnosticKind.AUTH_REQUIRED
                ? t('agentCenter.authenticationUnverified')
                : diagnostic.message}
            </p>
          ))}
        </div>
      )}

      {hasGarageProviderData && (
        <section className="vk-agent-center__section">
          <h3>{t('agentCenter.capabilities')}</h3>
          {capabilities.length > 0 ? (
            <div className="vk-agent-center__capabilities">
              {capabilities.map((capability) => (
                <span key={capability}>
                  <CheckCircleIcon aria-hidden="true" />
                  {t(capabilityKey(capability))}
                </span>
              ))}
            </div>
          ) : (
            <p className="vk-agent-center__empty">
              {t('agentCenter.noCapabilities')}
            </p>
          )}
        </section>
      )}

      <section className="vk-agent-center__section">
        <div className="vk-agent-center__section-heading">
          <h3>{t('agentCenter.enabledTools')}</h3>
          <button type="button" onClick={onOpenTools}>
            {t('agentCenter.manageTools')}
          </button>
        </div>
        <div className="vk-agent-center__metric-grid">
          <Metric
            label={t('agentCenter.tabs.mcp')}
            value={summaryValue(toolsState, `${mcpEnabled} / ${mcpTotal}`)}
            state={toolsState}
          />
          <Metric
            label={t('agentCenter.tabs.skills')}
            value={summaryValue(
              toolsState,
              `${skillsEnabled} / ${skillsTotal}`
            )}
            state={toolsState}
          />
          <Metric
            label={t('agentCenter.tabs.commands')}
            value={summaryValue(
              commandsState,
              `${commandsEnabled} / ${commandsTotal}`
            )}
            state={commandsState}
          />
        </div>
      </section>

      <section className="vk-agent-center__section">
        <div className="vk-agent-center__section-heading">
          <h3>{t('agentCenter.nativeSummary')}</h3>
          <button type="button" onClick={onOpenConfiguration}>
            {t('buttons.edit')}
          </button>
        </div>
        <div className="vk-agent-center__metric-grid">
          <Metric
            label={t('agentCenter.defaultModel')}
            value={summaryValue(
              settingsState,
              model?.trim() ? model : t('modelSelector.unspecified')
            )}
            state={settingsState}
          />
          <Metric
            label={t('agentCenter.apiAddress')}
            value={summaryValue(
              settingsState,
              apiAddress ?? t('agentCenter.inheritedOrUnset')
            )}
            state={settingsState}
          />
          <Metric
            label={t('agentCenter.configurationSource')}
            value={summaryValue(settingsState, configurationSource)}
            state={settingsState}
          />
        </div>
      </section>
    </div>
  );
}

function SourceProjectionNotice({
  projection,
  title,
  description,
  onRetry,
}: {
  projection: AgentCenterSourceProjection;
  title: string;
  description: string;
  onRetry: () => void;
}) {
  const { t } = useTranslation('common');
  const action = (
    <button
      type="button"
      className="vk-agent-center__state-action"
      onClick={onRetry}
    >
      {t('buttons.retry')}
    </button>
  );

  if (projection.state === 'error') {
    return (
      <ErrorState
        compact
        title={title}
        description={description}
        action={action}
      />
    );
  }

  if (projection.state === 'degraded') {
    return (
      <DegradedState
        compact
        title={title}
        description={description}
        action={action}
      />
    );
  }

  return null;
}

function Metric({
  label,
  value,
  state = 'ready',
}: {
  label: string;
  value: string;
  state?: SummaryState;
}) {
  return (
    <div className="vk-agent-center__metric" data-state={state}>
      <span>{label}</span>
      <strong title={value}>{value}</strong>
    </div>
  );
}
