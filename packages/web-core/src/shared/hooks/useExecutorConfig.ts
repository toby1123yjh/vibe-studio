import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type {
  BaseCodingAgent,
  ExecutorConfig,
  ExecutorProfile,
  ExecutorProfileId,
} from 'shared/types';
import { areProfilesEqual, getVariantOptions } from '@/shared/lib/executor';
import { filterVisibleAgents } from '@/shared/lib/agentVisibility';
import { usePresetOptions } from '@/shared/hooks/usePresetOptions';
import {
  executorProfileKey,
  resolveExecutorOverrides,
} from '@/shared/lib/executorConfig';

/**
 * Resolves effective executor.
 * userSelections.executor → scratch → lastUsedConfig → configDefault → first available
 */
function useEffectiveExecutor(
  userSelections: Partial<ExecutorConfig>,
  profiles: Record<string, ExecutorProfile> | null,
  scratchConfig: ExecutorConfig | null | undefined,
  lastUsedConfig: ExecutorConfig | null,
  configExecutorProfile: ExecutorProfileId | null | undefined,
  hiddenAgents?: readonly BaseCodingAgent[] | null
) {
  const options = useMemo(
    () =>
      filterVisibleAgents({
        agents: Object.keys(profiles ?? {}) as BaseCodingAgent[],
        hiddenAgents,
        preserveAgents: [
          userSelections.executor,
          scratchConfig?.executor,
          lastUsedConfig?.executor,
          configExecutorProfile?.executor,
        ],
      }),
    [
      profiles,
      hiddenAgents,
      userSelections.executor,
      scratchConfig?.executor,
      lastUsedConfig?.executor,
      configExecutorProfile?.executor,
    ]
  );

  const effective = useMemo(
    () =>
      userSelections.executor ??
      scratchConfig?.executor ??
      lastUsedConfig?.executor ??
      configExecutorProfile?.executor ??
      options[0] ??
      null,
    [
      userSelections.executor,
      scratchConfig,
      lastUsedConfig,
      configExecutorProfile,
      options,
    ]
  );

  return { effective, options };
}

/**
 * Resolves effective variant.
 * userSelections.variant → scratch (if same executor) → lastUsedConfig (if same executor)
 * → configDefault → DEFAULT/first
 */
function useEffectiveVariant(
  userSelections: Partial<ExecutorConfig>,
  effectiveExecutor: BaseCodingAgent | null,
  profiles: Record<string, ExecutorProfile> | null,
  scratchConfig: ExecutorConfig | null | undefined,
  lastUsedConfig: ExecutorConfig | null,
  configExecutorProfile: ExecutorProfileId | null | undefined
) {
  const options = useMemo(
    () => getVariantOptions(effectiveExecutor, profiles),
    [effectiveExecutor, profiles]
  );

  const wasUserSelected = 'variant' in userSelections;

  const resolved = useMemo(() => {
    if (wasUserSelected) return userSelections.variant ?? null;

    if (
      scratchConfig !== undefined &&
      scratchConfig?.executor === effectiveExecutor &&
      scratchConfig?.variant !== undefined
    ) {
      return scratchConfig.variant ?? null;
    }

    if (lastUsedConfig?.executor === effectiveExecutor) {
      return lastUsedConfig.variant ?? null;
    }

    if (configExecutorProfile?.executor === effectiveExecutor) {
      return configExecutorProfile.variant ?? null;
    }

    return (options.includes('DEFAULT') ? 'DEFAULT' : options[0]) ?? null;
  }, [
    wasUserSelected,
    userSelections.variant,
    scratchConfig,
    effectiveExecutor,
    lastUsedConfig,
    configExecutorProfile,
    options,
  ]);

  return { resolved, options, wasUserSelected };
}

/**
 * Resolves each override field independently through the fallback chain:
 * userSelections[field] → scratch[field] → lastUsed[field] → preset[field]
 */
function useEffectiveOverrides(
  effectiveExecutor: BaseCodingAgent | null,
  resolvedVariant: string | null,
  userSelections: Partial<ExecutorConfig>,
  scratchConfig: ExecutorConfig | null | undefined,
  lastUsedConfig: ExecutorConfig | null,
  presetOptions: ExecutorConfig | null | undefined
) {
  return useMemo((): ExecutorConfig | null => {
    return resolveExecutorOverrides(
      effectiveExecutor,
      resolvedVariant,
      userSelections,
      scratchConfig,
      lastUsedConfig,
      presetOptions
    );
  }, [
    effectiveExecutor,
    resolvedVariant,
    userSelections,
    scratchConfig,
    lastUsedConfig,
    presetOptions,
  ]);
}

interface UseExecutorConfigOptions {
  profiles: Record<string, ExecutorProfile> | null;
  lastUsedConfig: ExecutorConfig | null;
  scratchConfig?: ExecutorConfig | null;
  configExecutorProfile?: ExecutorProfileId | null;
  hiddenAgents?: readonly BaseCodingAgent[] | null;
  onPersist?: (config: ExecutorConfig) => void;
  /** Isolate in-memory choices when the owning workspace/session changes. */
  scopeKey?: string;
  /** Existing sessions cannot change their provider or bound runtime profile. */
  lockedExecutor?: BaseCodingAgent | null;
  lockedConfig?: ExecutorConfig | null;
  enabled?: boolean;
}

interface UseExecutorConfigResult {
  executorConfig: ExecutorConfig | null;
  effectiveExecutor: BaseCodingAgent | null;
  selectedVariant: string | null;
  executorOptions: BaseCodingAgent[];
  variantOptions: string[];
  presetOptions: ExecutorConfig | null | undefined;
  setExecutor: (executor: BaseCodingAgent) => void;
  setVariant: (variant: string | null) => void;
  setOverrides: (partial: Partial<ExecutorConfig>) => void;
}

/** Unified executor + variant + model selector overrides management. */
export function useExecutorConfig({
  profiles,
  lastUsedConfig,
  scratchConfig,
  configExecutorProfile,
  hiddenAgents,
  onPersist,
  scopeKey,
  lockedExecutor,
  lockedConfig,
  enabled = true,
}: UseExecutorConfigOptions): UseExecutorConfigResult {
  const [userSelections, setUserSelections] = useState<Partial<ExecutorConfig>>(
    {}
  );
  const [selectionScope, setSelectionScope] = useState(scopeKey);
  // Reset during render, before children can observe the previous session's config.
  if (selectionScope !== scopeKey) {
    setSelectionScope(scopeKey);
    setUserSelections({});
  }
  const selections = useMemo(
    () => ({
      ...(selectionScope === scopeKey ? userSelections : {}),
      ...(lockedExecutor ? { executor: lockedExecutor } : {}),
      ...(lockedConfig
        ? {
            executor: lockedConfig.executor,
            variant: lockedConfig.variant ?? null,
          }
        : {}),
    }),
    [selectionScope, scopeKey, userSelections, lockedExecutor, lockedConfig]
  );
  const compatibleScratch =
    scratchConfig &&
    ((lockedConfig && !areProfilesEqual(scratchConfig, lockedConfig)) ||
      (lockedExecutor && scratchConfig.executor !== lockedExecutor))
      ? undefined
      : scratchConfig;

  const executor = useEffectiveExecutor(
    selections,
    profiles,
    compatibleScratch,
    lastUsedConfig,
    configExecutorProfile,
    hiddenAgents
  );

  const variant = useEffectiveVariant(
    selections,
    executor.effective,
    profiles,
    compatibleScratch,
    lastUsedConfig,
    configExecutorProfile
  );

  const { data: presetOptions } = usePresetOptions(
    executor.effective,
    variant.resolved
  );

  const executorConfig = useEffectiveOverrides(
    executor.effective,
    variant.resolved,
    selections,
    compatibleScratch,
    lastUsedConfig,
    presetOptions
  );

  const profileKey = executorProfileKey(executor.effective, variant.resolved);
  const prevProfileKeyRef = useRef<string | null>(profileKey);
  useEffect(() => {
    const prev = prevProfileKeyRef.current;
    prevProfileKeyRef.current = profileKey;
    if (prev !== null && prev !== profileKey) {
      setUserSelections((s) => {
        const { executor, variant, ...rest } = s;
        if (Object.keys(rest).length === 0) return s;
        return { executor, variant };
      });
    }
  }, [profileKey]);

  const onPersistRef = useRef(onPersist);
  onPersistRef.current = onPersist;

  const persist = useCallback((config: ExecutorConfig | null) => {
    if (config) onPersistRef.current?.(config);
  }, []);

  // Setting executor → replaces entire selections with just { executor }.
  // Clears variant + all override fields.
  const setExecutor = useCallback(
    (exec: BaseCodingAgent) => {
      if (!enabled || lockedExecutor || lockedConfig) return;
      setUserSelections({ executor: exec });
      // Persist with auto-resolved variant (no overrides)
      const newVariants = getVariantOptions(exec, profiles);
      const newVariant = newVariants[0] ?? null;
      persist({ executor: exec, variant: newVariant });
    },
    [profiles, persist, enabled, lockedExecutor, lockedConfig]
  );

  // Setting variant → keeps executor, sets variant, clears all override fields.
  // Since 'variant' is in userSelections → variantWasUserSelected=true
  // → override fields fall through to preset options for the new variant.
  const setVariant = useCallback(
    (v: string | null) => {
      if (!enabled || lockedConfig) return;
      setUserSelections((prev) => ({ executor: prev.executor, variant: v }));
      if (executor.effective) {
        persist({ executor: executor.effective, variant: v });
      }
    },
    [executor.effective, persist, enabled, lockedConfig]
  );

  // Model selector updates individual override fields (merge into existing).
  // Keep explicit reasoning across model changes; the new native catalog validates it.
  const setOverrides = useCallback(
    (partial: Partial<ExecutorConfig>) => {
      if (!enabled) return;
      setUserSelections((prev) => {
        const next = { ...prev, ...partial };
        if (lockedExecutor || lockedConfig) {
          delete next.executor;
        }
        if (lockedConfig) delete next.variant;
        const persistedConfig = executor.effective
          ? {
              ...executorConfig,
              ...next,
              executor: executor.effective,
              variant: variant.resolved,
            }
          : null;
        // Persist with current effective executor/variant
        if (persistedConfig) {
          persist(persistedConfig);
        }
        return next;
      });
    },
    [
      executor.effective,
      variant.resolved,
      persist,
      executorConfig,
      enabled,
      lockedExecutor,
      lockedConfig,
    ]
  );

  return {
    executorConfig,
    effectiveExecutor: executor.effective,
    selectedVariant: variant.resolved,
    executorOptions: executor.options,
    variantOptions: variant.options,
    presetOptions,
    setExecutor,
    setVariant,
    setOverrides,
  };
}
