import type {
  ModelInfo,
  ModelSelectorConfig,
  ReasoningOption,
  RunState,
} from 'shared/types';

export function getCurrentObservedConfiguration(
  state:
    | Pick<
        RunState,
        'session_id' | 'last_run_attempt_id' | 'observed_configuration'
      >
    | null
    | undefined,
  sessionId: string | undefined
) {
  const observed = state?.observed_configuration;
  return sessionId &&
    state?.session_id === sessionId &&
    observed &&
    observed.run_attempt_id === state.last_run_attempt_id
    ? observed
    : null;
}

function toPrettyCase(value: string): string {
  return value
    .split('_')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
    .join(' ');
}

// Display only: an explicit null still follows CLI configuration at execution.
// Keep runtime-owned identifiers intact, including provider segments and aliases.
export function resolveModelDisplayId(
  configuredModel: string | null | undefined,
  presetModel: string | null | undefined,
  discoveredDefault: string | null | undefined
): string | null {
  const value =
    configuredModel !== undefined
      ? (configuredModel ?? discoveredDefault)
      : (presetModel ?? discoveredDefault);
  return value?.trim() ? value : null;
}

export function resolveNativeModelId(
  config: ModelSelectorConfig | null
): string | null {
  const configured = config?.native_defaults?.model_id;
  if (configured != null) return configured;
  // A native resolved default alias is authoritative. Catalog recommendations
  // and observations from previous attempts are not current configuration.
  if (config?.catalog_source !== 'native') return null;
  const defaults = config.models.filter(
    (model) =>
      model.capabilities?.is_default &&
      model.capabilities.resolved_model != null
  );
  if (defaults.length !== 1) return null;
  const model = defaults[0];
  return model.provider_id ? `${model.provider_id}/${model.id}` : model.id;
}

export function getSelectedModel(
  models: ModelInfo[],
  selectedProviderId: string | null,
  selectedModelId: string | null
): ModelInfo | null {
  if (!selectedModelId) return null;
  if (selectedProviderId) {
    return (
      models.find(
        (model) =>
          model.id === selectedModelId &&
          model.provider_id === selectedProviderId
      ) ?? null
    );
  }
  const matches = models.filter((model) => model.id === selectedModelId);
  return matches.length === 1 ? matches[0] : null;
}

export function findModelForSelection(
  models: ModelInfo[],
  modelId: string | null,
  providerId?: string
): ModelInfo | null {
  return getSelectedModel(models, providerId ?? null, modelId);
}

export function getReasoningLabel(
  options: ReasoningOption[],
  selectedId: string | null
): string | null {
  if (selectedId == null) return null;
  return (
    options.find((option) => option.id === selectedId)?.label ??
    toPrettyCase(selectedId)
  );
}

export function getReasoningDisplayLabel(
  options: ReasoningOption[],
  configuredReasoningId: string | null | undefined,
  hasConfiguredReasoning: boolean,
  followCliLabel: string,
  inheritedReasoningId?: string | null
): string | null {
  const { selectedReasoningId } = resolveReasoningOverrideState(
    options,
    configuredReasoningId,
    hasConfiguredReasoning
  );
  const explicitLabel = getReasoningLabel(options, selectedReasoningId);
  if (explicitLabel != null) return explicitLabel;
  const inheritedLabel = getReasoningLabel(
    options,
    inheritedReasoningId ?? null
  );
  if (inheritedLabel != null) return inheritedLabel;
  // A catalog default is not a confirmation of this session's actual effort.
  return options.length ? followCliLabel : null;
}

/** Native defaults are display metadata, never per-attempt overrides. */
export function resolveNativeReasoningId(
  config: ModelSelectorConfig | null,
  requestedModelId: string | null
): string | null {
  const native = config?.native_defaults;
  if (!config || !native) return null;
  if (requestedModelId === null) return native.reasoning_id;
  const hasProviders = config.providers.length > 0;
  const selected = parseModelId(requestedModelId, hasProviders);
  const model = getSelectedModel(
    config.models,
    selected.providerId,
    selected.modelId
  );
  for (const key of [requestedModelId, model?.capabilities?.resolved_model]) {
    if (
      key != null &&
      Object.prototype.hasOwnProperty.call(native.model_reasoning, key)
    ) {
      const effort = native.model_reasoning[key];
      if (typeof effort === 'string') return effort;
    }
  }

  // This field is provider-global. Per-model settings stay in model_reasoning
  // so an effort configured for one model never leaks to a different model.
  return native.reasoning_id;
}

/** Resolve a provider-advertised alias for presentation without rewriting IDs. */
export function getModelDisplayId(
  model: ModelInfo | null,
  requestedModelId: string | null
): string | null {
  return model?.capabilities?.resolved_model ?? requestedModelId;
}

export function escapeAttributeValue(value: string): string {
  if (typeof CSS !== 'undefined' && typeof CSS.escape === 'function') {
    return CSS.escape(value);
  }
  return value.replace(/["\\]/g, '\\$&');
}

export function parseModelId(
  value?: string | null,
  hasProviders?: boolean
): {
  providerId: string | null;
  modelId: string | null;
} {
  if (!value) return { providerId: null, modelId: null };
  if (!hasProviders) return { providerId: null, modelId: value };
  const slashIdx = value.indexOf('/');
  if (slashIdx === -1) return { providerId: null, modelId: value };
  return {
    providerId: value.substring(0, slashIdx),
    modelId: value.substring(slashIdx + 1),
  };
}

export function appendPresetModel(
  config: ModelSelectorConfig | null,
  presetModel: string | null | undefined
): ModelSelectorConfig | null {
  if (!config || !presetModel) return config;
  const hasProviders = config.providers.length > 0;
  const { providerId, modelId } = parseModelId(presetModel, hasProviders);
  if (!modelId) return config;

  const exists = config.models.some(
    (m) => m.id === modelId && (!providerId || m.provider_id === providerId)
  );
  if (exists) return config;

  return {
    ...config,
    models: [
      {
        id: modelId,
        name: modelId,
        provider_id: providerId,
        reasoning_options: [],
      },
      ...config.models,
    ],
  };
}

export function resolveDefaultModelId(
  models: ModelInfo[],
  providerId: string | null,
  defaultModel: string | null | undefined,
  hasProviders?: boolean
): string | null {
  if (models.length === 0) return null;
  const scoped = providerId
    ? models.filter((model) => model.provider_id === providerId)
    : models;
  if (scoped.length === 0) return null;

  const { providerId: defaultProvider, modelId: defaultId } = parseModelId(
    defaultModel,
    hasProviders
  );
  if (
    defaultId &&
    (!providerId || !defaultProvider || providerId === defaultProvider)
  ) {
    const match = scoped.find((model) => model.id === defaultId);
    if (match) return match.id;
  }

  if (!defaultModel) return null;

  return null;
}

export function isModelAvailable(
  config: ModelSelectorConfig,
  providerId: string,
  modelId: string
): boolean {
  return config.models.some(
    (model) => model.id === modelId && model.provider_id === providerId
  );
}

export function resolveDefaultReasoningId(
  options: ReasoningOption[]
): string | null {
  return options.find((option) => option.is_default)?.id ?? null;
}

export function isReasoningOptionAvailable(
  options: ReasoningOption[],
  reasoningId: string | null | undefined
): boolean {
  return (
    reasoningId != null && options.some((option) => option.id === reasoningId)
  );
}

export function resolveReasoningIdForOptions(
  options: ReasoningOption[],
  preferredReasoningId: string | null | undefined
): string | null {
  if (isReasoningOptionAvailable(options, preferredReasoningId)) {
    return preferredReasoningId ?? null;
  }
  return null;
}

export function resolveConfiguredReasoningIdForOptions(
  _options: ReasoningOption[],
  configuredReasoningId: string | null | undefined,
  hasConfiguredReasoning: boolean
): string | null {
  return hasConfiguredReasoning ? (configuredReasoningId ?? null) : null;
}

export function resolveReasoningOverrideState(
  options: ReasoningOption[],
  configuredReasoningId: string | null | undefined,
  hasConfiguredReasoning: boolean
): {
  selectedReasoningId: string | null;
  unavailable: boolean;
} {
  return {
    selectedReasoningId: resolveConfiguredReasoningIdForOptions(
      options,
      configuredReasoningId,
      hasConfiguredReasoning
    ),
    unavailable:
      hasConfiguredReasoning &&
      configuredReasoningId != null &&
      !isReasoningOptionAvailable(options, configuredReasoningId),
  };
}

export function buildModelSelectionOverride(
  _models: ModelInfo[],
  modelId: string | null,
  providerId?: string
): {
  model_id: string | null;
} {
  const model_id = modelId
    ? providerId
      ? `${providerId}/${modelId}`
      : modelId
    : null;

  return { model_id };
}
