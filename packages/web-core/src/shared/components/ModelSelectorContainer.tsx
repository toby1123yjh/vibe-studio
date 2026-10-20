import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  CheckIcon,
  FastForwardIcon,
  GearIcon,
  HandIcon,
  ListBulletsIcon,
  SlidersHorizontalIcon,
  type Icon,
} from '@phosphor-icons/react';
import type {
  BaseCodingAgent,
  ExecutorConfig,
  ModelInfo,
  ModelSelectorConfig,
} from 'shared/types';
import { PermissionPolicy } from 'shared/types';
import { useOptionalAgentRunSession } from '@/features/agent-runtime/model/AgentRunSessionContext';
import { ProviderRunOptionsMenu } from './ProviderRunOptionsMenu';
import { toPrettyCase } from '@/shared/lib/string';
import {
  getModelKey,
  getRecentModelEntries,
  touchRecentModel,
  updateRecentModelEntries,
  setRecentReasoning,
} from '@/shared/lib/recentModels';
import {
  getReasoningDisplayLabel,
  getSelectedModel,
  escapeAttributeValue,
  parseModelId,
  appendPresetModel,
  buildModelSelectionOverride,
  findModelForSelection,
  resolveReasoningOverrideState,
  resolveModelDisplayId,
  resolveNativeModelId,
  getModelDisplayId,
  isUnresolvedClaudeModelAlias,
  getReasoningLabel,
  resolveNativeReasoningId,
  getCurrentObservedConfiguration,
} from '@/shared/lib/modelSelector';
import { profilesApi } from '@/shared/lib/api';
import { resolveModelDiscoveryVariant } from '@/shared/lib/executorDiscovery';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { getResolvedTheme, useTheme } from '@/shared/hooks/useTheme';
import { useModelSelectorConfig } from '@/shared/hooks/useExecutorDiscovery';
import { ModelSelectorPopover } from '@vibe/ui/components/ModelSelectorPopover';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTriggerButton,
} from '@vibe/ui/components/Dropdown';

interface ModelSelectorContainerProps {
  agent: BaseCodingAgent | null;
  workspaceId: string | undefined;
  sessionId?: string;
  onAdvancedSettings: () => void;
  presets: string[];
  selectedPreset: string | null;
  onPresetSelect: (presetId: string | null) => void;
  onOverrideChange: (partial: Partial<ExecutorConfig>) => void;
  executorConfig: ExecutorConfig | null;
  presetOptions: ExecutorConfig | null | undefined;
  readOnly?: boolean;
  onInputModalitiesChange?: (modalities: string[] | undefined) => void;
}

const EMPTY_REASONING_OPTIONS: ModelInfo['reasoning_options'] = [];
const EMPTY_MODEL_CONFIG: ModelSelectorConfig = {
  models: [],
  providers: [],
  default_model: null,
  agents: [],
  permissions: [],
};

type PendingReasoningSelection = {
  reasoningId: string | null;
};

export function ModelSelectorContainer({
  agent,
  workspaceId,
  sessionId,
  onAdvancedSettings,
  presets,
  selectedPreset,
  onPresetSelect,
  onOverrideChange,
  executorConfig,
  presetOptions,
  readOnly = false,
  onInputModalitiesChange,
}: ModelSelectorContainerProps) {
  const { t } = useTranslation('common');
  const hostId = useHostId();
  const runSession = useOptionalAgentRunSession();
  const { theme } = useTheme();
  const resolvedTheme = getResolvedTheme(theme);
  const [isOpen, setIsOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const [expandedProviderId, setExpandedProviderId] = useState('');
  const { profiles, setProfiles, reloadSystem } = useUserSystem();
  const defaultLabel = t('modelSelector.default');
  const unspecifiedModelLabel = t('modelSelector.unspecified');
  const loadingLabel = t('states.loading');

  const permissionMetaByPolicy: Record<
    PermissionPolicy,
    { label: string; icon: Icon }
  > = {
    [PermissionPolicy.AUTO]: {
      label: t('modelSelector.permissionAuto'),
      icon: FastForwardIcon,
    },
    [PermissionPolicy.SUPERVISED]: {
      label: t('modelSelector.permissionAsk'),
      icon: HandIcon,
    },
    [PermissionPolicy.PLAN]: {
      label: t('modelSelector.permissionPlan'),
      icon: ListBulletsIcon,
    },
  };

  const resolvedPreset =
    selectedPreset ??
    (presets.includes('DEFAULT') ? 'DEFAULT' : (presets[0] ?? null));
  const selectionScope = JSON.stringify([
    agent,
    hostId,
    workspaceId,
    sessionId,
    resolvedPreset,
  ]);
  const supportsManualModel = agent === 'GEMINI';

  const {
    config: streamConfig,
    loadingModels,
    error: streamError,
    refresh,
  } = useModelSelectorConfig(agent, {
    workspaceId,
    sessionId,
    variant: resolveModelDiscoveryVariant(agent, resolvedPreset),
  });

  useEffect(() => {
    if (streamError) {
      console.error('Failed to fetch model config', streamError);
    }
  }, [streamError]);

  const baseConfig = streamConfig;
  // A failed/empty catalog is unknown, not a reason to hide native selection.
  // This UI-only shell never advertises model capabilities.
  const editableConfig =
    baseConfig ??
    (supportsManualModel || streamError ? EMPTY_MODEL_CONFIG : null);
  const config = appendPresetModel(
    appendPresetModel(
      appendPresetModel(editableConfig, baseConfig?.native_defaults?.model_id),
      presetOptions?.model_id
    ),
    executorConfig?.model_id
  );

  const hasProviders = (config?.providers.length ?? 0) > 0;
  const nativeModelId = resolveNativeModelId(config);
  const requestedModelId = resolveModelDisplayId(
    executorConfig?.model_id,
    presetOptions?.model_id,
    nativeModelId
  );
  const { providerId: selectedProviderId, modelId: selectedModelId } =
    parseModelId(requestedModelId, hasProviders);
  const nativeModelSelection = parseModelId(nativeModelId, hasProviders);
  const nativeModel = config
    ? getSelectedModel(
        config.models,
        nativeModelSelection.providerId,
        nativeModelSelection.modelId
      )
    : null;

  const selectedModel = config
    ? getSelectedModel(config.models, selectedProviderId, selectedModelId)
    : null;
  const discoveredSelectedModel = baseConfig
    ? getSelectedModel(baseConfig.models, selectedProviderId, selectedModelId)
    : null;
  const inputModalities =
    !streamError && !loadingModels
      ? discoveredSelectedModel?.capabilities?.input_modalities
      : undefined;
  useEffect(() => {
    onInputModalitiesChange?.(inputModalities);
  }, [inputModalities, onInputModalitiesChange]);

  const selectedReasoningOptions =
    selectedModel?.reasoning_options ?? EMPTY_REASONING_OPTIONS;
  const configuredReasoningId =
    executorConfig?.reasoning_id !== undefined
      ? executorConfig.reasoning_id
      : presetOptions?.reasoning_id;
  const hasConfiguredReasoning = configuredReasoningId !== undefined;
  const reasoningOverrideState = useMemo(
    () =>
      resolveReasoningOverrideState(
        selectedReasoningOptions,
        configuredReasoningId,
        hasConfiguredReasoning
      ),
    [configuredReasoningId, hasConfiguredReasoning, selectedReasoningOptions]
  );
  const selectedReasoningId = reasoningOverrideState.selectedReasoningId;
  const inheritedReasoningId = resolveNativeReasoningId(
    config,
    requestedModelId
  );
  const inheritedReasoningLabel = getReasoningLabel(
    selectedReasoningOptions,
    inheritedReasoningId
  );
  const followCliReasoningLabel = inheritedReasoningLabel
    ? t('modelSelector.followCliValue', { value: inheritedReasoningLabel })
    : t('modelSelector.followCliConfig');

  const providerOptions =
    executorConfig?.provider_options !== undefined
      ? executorConfig.provider_options
      : presetOptions?.provider_options;
  const latestRun =
    sessionId && runSession?.timeline?.sessionId === sessionId
      ? runSession.timeline.latestRun
      : null;
  const runState = latestRun?.timeline?.state ?? latestRun?.summary.state;
  const currentObservation = getCurrentObservedConfiguration(
    runState,
    sessionId
  );
  const capabilityWarning =
    configuredReasoningId != null &&
    (!discoveredSelectedModel || reasoningOverrideState.unavailable);

  const defaultAgentId =
    config?.agents.find((entry) => entry.is_default)?.id ?? null;

  const selectedAgentId =
    executorConfig?.agent_id !== undefined
      ? executorConfig.agent_id
      : (presetOptions?.agent_id ?? defaultAgentId);

  // Claude has one native mode control; auto is not bypassPermissions.
  const supportsPermissions =
    agent !== 'CLAUDE_CODE' && (config?.permissions.length ?? 0) > 0;

  const basePermissionPolicy = supportsPermissions
    ? (presetOptions?.permission_policy ?? config?.permissions[0] ?? null)
    : null;
  const permissionPolicy = supportsPermissions
    ? executorConfig?.permission_policy !== undefined
      ? executorConfig.permission_policy
      : basePermissionPolicy
    : null;

  // LRU persistence (on popover close)

  const recentModelEntries = getRecentModelEntries(profiles, agent);
  const pendingModelRef = useRef<ModelInfo | null>(null);
  const pendingReasoningRef = useRef<PendingReasoningSelection | null>(null);
  const pendingScopeRef = useRef(selectionScope);

  useEffect(() => {
    if (pendingScopeRef.current === selectionScope) return;
    pendingScopeRef.current = selectionScope;
    pendingModelRef.current = null;
    pendingReasoningRef.current = null;
    setIsOpen(false);
    setSearchQuery('');
    setExpandedProviderId('');
  }, [selectionScope]);

  const persistPendingSelections = useCallback(() => {
    if (pendingScopeRef.current !== selectionScope) return;
    if (!profiles || !agent) return;
    if (!pendingModelRef.current && pendingReasoningRef.current === null) {
      return;
    }

    let nextProfiles = profiles;

    const model = pendingModelRef.current;
    if (model) {
      pendingModelRef.current = null;
      const current = getRecentModelEntries(nextProfiles, agent);
      const nextEntries = touchRecentModel(current, model);
      nextProfiles = updateRecentModelEntries(nextProfiles, agent, nextEntries);
    }

    const reasoningModel =
      model ??
      (selectedModelId && config
        ? getSelectedModel(config.models, selectedProviderId, selectedModelId)
        : null);
    const pendingReasoning = pendingReasoningRef.current;
    if (pendingReasoning && reasoningModel) {
      nextProfiles = setRecentReasoning(
        nextProfiles,
        agent,
        reasoningModel,
        pendingReasoning.reasoningId
      );
      pendingReasoningRef.current = null;
    }

    if (nextProfiles !== profiles) {
      setProfiles(nextProfiles);
      void profilesApi
        .save(JSON.stringify({ executors: nextProfiles }, null, 2))
        .catch((error) => {
          console.error('Failed to save recent models', error);
          void reloadSystem();
        });
    }
  }, [
    agent,
    config,
    profiles,
    reloadSystem,
    selectedModelId,
    selectedProviderId,
    selectionScope,
    setProfiles,
  ]);

  const handleModelSelect = (modelId: string | null, providerId?: string) => {
    const modelOverride = config
      ? buildModelSelectionOverride(config.models, modelId, providerId)
      : {
          model_id: modelId
            ? providerId
              ? `${providerId}/${modelId}`
              : modelId
            : null,
        };
    onOverrideChange(modelOverride);

    pendingModelRef.current =
      modelId && config
        ? findModelForSelection(config.models, modelId, providerId)
        : null;
    pendingReasoningRef.current = null;
  };

  const handleReasoningSelect = (reasoningId: string | null) => {
    onOverrideChange({ reasoning_id: reasoningId });
    pendingReasoningRef.current = { reasoningId };
  };

  const handleAgentSelect = (id: string | null) => {
    onOverrideChange({ agent_id: id });
  };

  const handlePermissionPolicyChange = (policy: PermissionPolicy) => {
    if (!supportsPermissions) return;
    onOverrideChange({ permission_policy: policy });
  };

  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    setSearchQuery('');
  }, [selectedProviderId]);

  useEffect(() => {
    if (!isOpen) {
      setSearchQuery('');
      return;
    }
    requestAnimationFrame(() => {
      const node = scrollRef.current;
      if (!node) return;
      if (selectedModelId && config) {
        const selected = getSelectedModel(
          config.models,
          selectedProviderId,
          selectedModelId
        );
        if (selected) {
          const key = getModelKey(selected);
          const selector = `[data-model-key="${escapeAttributeValue(key)}"]`;
          const target = node.querySelector(selector);
          if (target instanceof HTMLElement) {
            target.scrollIntoView({ block: 'nearest' });
            return;
          }
        }
      }
      if (!selectedModelId) {
        node.scrollTop = node.scrollHeight;
      }
    });
  }, [config, isOpen, selectedModelId, selectedProviderId]);

  const handleOpenChange = (open: boolean) => {
    setIsOpen(open);
    if (open) {
      const selected =
        selectedModelId && config
          ? getSelectedModel(config.models, selectedProviderId, selectedModelId)
          : null;
      setExpandedProviderId(selected?.provider_id ?? selectedProviderId ?? '');
    } else {
      persistPendingSelections();
    }
  };

  useEffect(() => {
    if (isOpen) return;
    persistPendingSelections();
  }, [isOpen, persistPendingSelections]);

  const presetLabel = resolvedPreset
    ? toPrettyCase(resolvedPreset)
    : defaultLabel;

  const getModelLabel = (model: ModelInfo | null, modelId: string | null) => {
    const label = getModelDisplayId(model, modelId, agent);
    return label && isUnresolvedClaudeModelAlias(model, modelId, agent)
      ? t('modelSelector.aliasModel', { model: label })
      : label;
  };
  const displayModelId = getModelLabel(selectedModel, requestedModelId);
  const displayConfig = config && {
    ...config,
    models: config.models.map((model) => {
      const modelKey = getModelKey(model);
      const displayName = getModelLabel(model, modelKey) ?? modelKey;
      return {
        ...model,
        display_name: displayName,
        description: isUnresolvedClaudeModelAlias(model, modelKey, agent)
          ? t('modelSelector.unresolvedAlias', { alias: modelKey })
          : displayName !== modelKey
            ? t('modelSelector.nativeModelId', { id: modelKey })
            : undefined,
      };
    }),
  };

  if (!config) {
    return (
      <>
        <DropdownMenu>
          <DropdownMenuTriggerButton
            size="sm"
            label={
              loadingModels
                ? loadingLabel
                : (displayModelId ?? unspecifiedModelLabel)
            }
            wrapLabel
            disabled
          />
        </DropdownMenu>
        {!readOnly && streamError && (
          <button
            type="button"
            onClick={refresh}
            className="text-sm text-error focus-visible:ring-1 focus-visible:ring-brand"
          >
            {t('modelSelector.retryDiscovery')}
          </button>
        )}
      </>
    );
  }

  const showModelSelector =
    loadingModels ||
    config.models.length > 0 ||
    supportsManualModel ||
    Boolean(streamError);
  const showDefaultOption =
    config.models.length > 0 || supportsManualModel || Boolean(streamError);
  const reasoningLabel =
    loadingModels && !streamError
      ? null
      : getReasoningDisplayLabel(
          selectedReasoningOptions,
          configuredReasoningId,
          hasConfiguredReasoning,
          t('modelSelector.followCliConfig'),
          inheritedReasoningId
        );
  const modelLabelBase =
    loadingModels && !streamError && !displayModelId
      ? loadingLabel
      : (displayModelId ?? unspecifiedModelLabel);
  const modelLabel = reasoningLabel
    ? `${modelLabelBase} · ${reasoningLabel}`
    : modelLabelBase;

  const agentLabel = selectedAgentId
    ? (config.agents.find((entry) => entry.id === selectedAgentId)?.label ??
      toPrettyCase(selectedAgentId))
    : defaultLabel;

  const permissionMeta = permissionPolicy
    ? (permissionMetaByPolicy[permissionPolicy] ?? null)
    : null;
  const permissionIcon = permissionMeta?.icon ?? HandIcon;

  const observationDetails = (
    <>
      <DropdownMenuLabel>{t('modelSelector.latestAttempt')}</DropdownMenuLabel>
      <p className="max-w-72 break-words px-base pb-half text-sm text-low">
        {currentObservation
          ? t('modelSelector.observed', {
              model:
                currentObservation.model_id ?? t('modelSelector.unconfirmed'),
              reasoning:
                currentObservation.reasoning_id ??
                t('modelSelector.unconfirmed'),
            })
          : t('modelSelector.unconfirmed')}
      </p>
      {currentObservation && (
        <p className="max-w-72 break-words px-base pb-half text-sm text-low">
          {t('modelSelector.observationSource', {
            source: currentObservation.source,
          })}
        </p>
      )}
    </>
  );

  if (readOnly) {
    return (
      <DropdownMenu>
        <DropdownMenuTriggerButton size="sm" label={modelLabel} wrapLabel />
        <DropdownMenuContent align="start">
          {streamError && (
            <p
              role="alert"
              className="max-w-72 break-words px-base pb-half text-sm text-error"
            >
              {streamError}
            </p>
          )}
          {observationDetails}
        </DropdownMenuContent>
      </DropdownMenu>
    );
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTriggerButton
          size="sm"
          icon={SlidersHorizontalIcon}
          label={
            resolvedPreset?.toLowerCase() !== 'default'
              ? presetLabel
              : undefined
          }
          showCaret={false}
          aria-label={t('modelSelector.custom')}
        />
        <DropdownMenuContent align="start">
          <DropdownMenuLabel>{t('modelSelector.preset')}</DropdownMenuLabel>
          {presets.length > 0 ? (
            presets.map((preset) => (
              <DropdownMenuItem
                key={preset}
                icon={preset === resolvedPreset ? CheckIcon : undefined}
                onClick={() => onPresetSelect?.(preset)}
              >
                {toPrettyCase(preset)}
              </DropdownMenuItem>
            ))
          ) : (
            <DropdownMenuItem disabled>{presetLabel}</DropdownMenuItem>
          )}
          <DropdownMenuSeparator />
          <DropdownMenuLabel>{t('modelSelector.requested')}</DropdownMenuLabel>
          <p className="max-w-72 break-words px-base pb-half text-sm text-normal">
            {modelLabel}
          </p>
          {capabilityWarning && (
            <p
              role="status"
              className="max-w-72 px-base pb-half text-sm text-error"
            >
              {t('modelSelector.reasoningUnverified', {
                value: configuredReasoningId,
              })}
            </p>
          )}
          {configuredReasoningId != null && (
            <DropdownMenuItem onClick={() => handleReasoningSelect(null)}>
              {t('modelSelector.clearReasoning')}
            </DropdownMenuItem>
          )}
          {observationDetails}
          <ProviderRunOptionsMenu
            agent={agent}
            options={providerOptions}
            model={discoveredSelectedModel}
            onChange={onOverrideChange}
          />
          {agent === 'GEMINI' && (
            <p className="max-w-72 px-base py-half text-sm text-low">
              {t('modelSelector.geminiNativeThinking')}
            </p>
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={refresh} disabled={loadingModels}>
            {t('modelSelector.refreshDiscovery')}
          </DropdownMenuItem>
          <p className="max-w-72 px-base pb-half text-sm text-low">
            {t('modelSelector.refreshScope')}
          </p>
          <DropdownMenuItem icon={GearIcon} onClick={onAdvancedSettings}>
            {t('modelSelector.custom')}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      {(streamError || capabilityWarning) && (
        <button
          type="button"
          onClick={streamError ? refresh : () => setIsOpen(true)}
          className="text-sm text-error focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand"
          title={
            streamError ??
            t('modelSelector.reasoningUnverified', {
              value: configuredReasoningId,
            })
          }
        >
          {t(
            streamError
              ? 'modelSelector.retryDiscovery'
              : 'modelSelector.checkReasoning'
          )}
        </button>
      )}

      {showModelSelector && displayConfig && (
        <ModelSelectorPopover
          key={selectionScope}
          isOpen={isOpen}
          onOpenChange={handleOpenChange}
          trigger={
            <DropdownMenuTriggerButton
              size="sm"
              label={modelLabel}
              title={modelLabel}
              wrapLabel
              disabled={loadingModels && !streamError && !supportsManualModel}
            />
          }
          config={displayConfig}
          error={streamError}
          onRetry={refresh}
          refreshing={loadingModels && !streamError}
          onManualModelSelect={
            supportsManualModel ? (id) => handleModelSelect(id) : undefined
          }
          selectedProviderId={selectedProviderId}
          selectedModelId={selectedModelId}
          selectedReasoningId={selectedReasoningId}
          followCliReasoningLabel={followCliReasoningLabel}
          inheritedModelLabel={getModelLabel(nativeModel, nativeModelId)}
          searchQuery={searchQuery}
          onSearchChange={setSearchQuery}
          onModelSelect={handleModelSelect}
          onReasoningSelect={handleReasoningSelect}
          recentModelEntries={recentModelEntries}
          showDefaultOption={showDefaultOption}
          onSelectDefault={() => handleModelSelect(null)}
          scrollRef={scrollRef}
          expandedProviderId={expandedProviderId}
          onExpandedProviderIdChange={setExpandedProviderId}
          resolvedTheme={resolvedTheme}
        />
      )}

      {!showModelSelector && (
        <DropdownMenu>
          <DropdownMenuTriggerButton
            size="sm"
            label={modelLabel}
            title={modelLabel}
            wrapLabel
            disabled
          />
        </DropdownMenu>
      )}

      {permissionPolicy && config.permissions.length > 0 && (
        <DropdownMenu>
          <DropdownMenuTriggerButton
            size="sm"
            icon={permissionIcon}
            showCaret={false}
          />
          <DropdownMenuContent align="start">
            <DropdownMenuLabel>
              {t('modelSelector.permissions')}
            </DropdownMenuLabel>
            {config.permissions.map((policy) => {
              const meta = permissionMetaByPolicy[policy];
              return (
                <DropdownMenuItem
                  key={policy}
                  icon={meta?.icon ?? HandIcon}
                  onClick={() => handlePermissionPolicyChange(policy)}
                >
                  {meta?.label ?? toPrettyCase(policy)}
                </DropdownMenuItem>
              );
            })}
          </DropdownMenuContent>
        </DropdownMenu>
      )}

      {config.agents.length > 0 && (
        <DropdownMenu>
          <DropdownMenuTriggerButton size="sm" label={agentLabel} />
          <DropdownMenuContent align="start">
            <DropdownMenuLabel>{t('modelSelector.agent')}</DropdownMenuLabel>
            <DropdownMenuItem
              icon={selectedAgentId === null ? CheckIcon : undefined}
              onClick={() => handleAgentSelect(null)}
            >
              {t('modelSelector.default')}
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            {config.agents.map((agentOption) => (
              <DropdownMenuItem
                key={agentOption.id}
                icon={
                  agentOption.id === selectedAgentId ? CheckIcon : undefined
                }
                onClick={() => handleAgentSelect(agentOption.id)}
              >
                {agentOption.label}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </>
  );
}
