import { expect, test as it } from '@playwright/test';
import { createRequire } from 'node:module';
const describe = it.describe;
import type {
  ExecutorConfig,
  ModelInfo,
  ModelSelectorConfig,
} from 'shared/types';
import {
  appendPresetModel,
  buildModelSelectionOverride,
  parseModelId,
  resolveConfiguredReasoningIdForOptions,
  resolveDefaultReasoningId,
  resolveReasoningOverrideState,
  resolveReasoningIdForOptions,
  getModelDisplayId,
  isUnresolvedClaudeModelAlias,
  getReasoningDisplayLabel,
  resolveModelDisplayId,
  resolveNativeModelId,
  resolveNativeReasoningId,
  resolveDefaultModelId,
  isCodexReasoningConfigError,
  resolveActiveReasoning,
  buildReasoningSelectionOverride,
} from './modelSelector';

// The generated file belongs to the root CommonJS package in Node tests.
const { PermissionPolicy }: typeof import('shared/types') = createRequire(
  import.meta.url
)('../../../../../shared/types.ts');

describe('Codex configuration compatibility warning', () => {
  const error =
    'Codex native configuration discovery failed: I/O error: config/read request failed: invalid configuration: unknown variant `max`, expected one of `none`, `minimal`, `low`, `medium`, `high`, `xhigh` in `model_reasoning_effort`';

  it('recognizes the native effort parser error without rewriting it', () => {
    expect(isCodexReasoningConfigError('CODEX', error)).toBe(true);
    expect(
      isCodexReasoningConfigError(
        'CODEX',
        error.replace('unknown variant', 'invalid variant')
      )
    ).toBe(true);
    expect(
      isCodexReasoningConfigError(
        'CODEX',
        `Update Codex on this host. ${error}`
      )
    ).toBe(true);
  });

  it('does not call unrelated discovery failures a Codex effort incompatibility', () => {
    expect(isCodexReasoningConfigError('GEMINI', error)).toBe(false);
    expect(isCodexReasoningConfigError(null, error)).toBe(false);
    expect(isCodexReasoningConfigError('CODEX', null)).toBe(false);
    expect(
      isCodexReasoningConfigError('CODEX', 'config/read request timed out')
    ).toBe(false);
    expect(
      isCodexReasoningConfigError(
        'CODEX',
        'config/read: unknown variant `other` in `sandbox_mode`'
      )
    ).toBe(false);
  });
});

const models: ModelInfo[] = [
  {
    id: 'gpt-5.5',
    name: 'GPT-5.5',
    provider_id: null,
    reasoning_options: [
      { id: 'medium', label: 'Medium', is_default: false },
      { id: 'xhigh', label: 'Extra High', is_default: true },
    ],
  },
  {
    id: 'sonnet',
    name: 'Sonnet',
    provider_id: 'anthropic',
    reasoning_options: [
      { id: 'medium', label: 'Medium', is_default: false },
      { id: 'high', label: 'High', is_default: true },
    ],
  },
  {
    id: 'haiku',
    name: 'Haiku',
    provider_id: 'anthropic',
    reasoning_options: [],
  },
];

describe('model selector model ID semantics', () => {
  it('preserves colons inside model IDs', () => {
    expect(parseModelId('custom:lfm2.5:8b', true)).toEqual({
      providerId: null,
      modelId: 'custom:lfm2.5:8b',
    });
  });

  it('splits only the first slash when providers exist', () => {
    expect(parseModelId('custom/model/family:latest', true)).toEqual({
      providerId: 'custom',
      modelId: 'model/family:latest',
    });
  });

  for (const modelId of ['custom:lfm2.5:8b', 'model/family:latest']) {
    it(`round-trips provider-scoped model ID ${modelId} without rewriting it`, () => {
      const { model_id: value } = buildModelSelectionOverride(
        models,
        modelId,
        'custom'
      );

      expect(value).toBe(`custom/${modelId}`);
      expect(parseModelId(value, true)).toEqual({
        providerId: 'custom',
        modelId,
      });
    });
  }

  it('treats the entire value as a model ID when providers are absent', () => {
    const value = 'custom/model/family:latest';

    expect(parseModelId(value, false)).toEqual({
      providerId: null,
      modelId: value,
    });
  });
});

describe('native configured model and reasoning display', () => {
  const config: ModelSelectorConfig = {
    providers: [],
    models,
    agents: [],
    permissions: [],
    default_model: 'gpt-5.5',
    native_defaults: {
      model_id: 'gpt-6.1-sol',
      reasoning_id: 'max',
      model_reasoning: {},
    },
    catalog_source: 'native',
  };

  it('shows native configured values instead of catalog defaults without creating overrides', () => {
    const request = { model_id: undefined, reasoning_id: undefined };
    const modelId = resolveModelDisplayId(
      request.model_id,
      undefined,
      config.native_defaults?.model_id
    );
    expect(modelId).toBe('gpt-6.1-sol');
    const effort = resolveNativeReasoningId(config, modelId);
    expect(effort).toBe('max');
    expect(
      getReasoningDisplayLabel([], undefined, false, 'Follow CLI', effort)
    ).toBe('Max');
    expect(request).toEqual({ model_id: undefined, reasoning_id: undefined });
  });

  it('keeps native models absent from the catalog visible with unknown capabilities', () => {
    const appended = appendPresetModel(
      config,
      config.native_defaults?.model_id
    );
    expect(appended?.models[0]).toEqual({
      id: 'gpt-6.1-sol',
      name: 'gpt-6.1-sol',
      provider_id: null,
      reasoning_options: [],
    });
    expect(appended?.models[0].capabilities).toBeUndefined();
  });

  it('preserves explicit overrides and explicit null bypasses a preset', () => {
    expect(resolveModelDisplayId('gpt-5.5', 'preset', 'gpt-6.1-sol')).toBe(
      'gpt-5.5'
    );
    expect(resolveModelDisplayId(null, 'preset', 'gpt-6.1-sol')).toBe(
      'gpt-6.1-sol'
    );
    expect(
      getReasoningDisplayLabel(
        models[0].reasoning_options,
        'medium',
        true,
        'Follow CLI',
        'max'
      )
    ).toBe('Medium');
    expect(
      getReasoningDisplayLabel(
        models[0].reasoning_options,
        null,
        true,
        'Follow CLI',
        'max'
      )
    ).toBe('Max');
  });

  it('does not use catalog defaults or list order when native configuration is missing', () => {
    const unknown: ModelSelectorConfig = {
      ...config,
      native_defaults: undefined,
    };
    expect(resolveNativeReasoningId(unknown, 'gpt-5.5')).toBeNull();
    expect(
      resolveModelDisplayId(
        undefined,
        undefined,
        unknown.native_defaults?.model_id
      )
    ).toBeNull();
    expect(resolveDefaultModelId(models, null, 'missing', false)).toBeNull();
  });

  const aliasConfig: ModelSelectorConfig = {
    ...config,
    models: [
      {
        id: 'opus',
        name: 'Opus',
        reasoning_options: [
          { id: 'xhigh', label: 'Extra High', is_default: false },
        ],
        capabilities: {
          reasoning: 'supported',
          input_modalities: ['text'],
          fast_mode: 'unknown',
          auto_mode: 'unknown',
          resolved_model: 'claude-opus-4-6',
          is_default: false,
          service_tiers: [],
        },
      },
      { id: 'sonnet', name: 'Sonnet', reasoning_options: [] },
    ],
    native_defaults: {
      model_id: 'opus',
      reasoning_id: null,
      model_reasoning: { 'claude-opus-4-6': 'xhigh' },
    },
  };

  it('uses an advertised alias resolution for display and per-model settings without rewriting execution IDs', () => {
    const selected = aliasConfig.models[0];
    expect(getModelDisplayId(selected, selected.id)).toBe('claude-opus-4-6');
    expect(resolveNativeReasoningId(aliasConfig, selected.id)).toBe('xhigh');
    expect(
      buildModelSelectionOverride(aliasConfig.models, selected.id)
    ).toEqual({ model_id: 'opus' });
  });

  it('does not carry one model-specific effort into another model', () => {
    expect(resolveNativeReasoningId(aliasConfig, 'sonnet')).toBeNull();
    expect(resolveNativeReasoningId(aliasConfig, 'OPUS')).toBeNull();
  });

  it('uses exact model settings before resolved alias settings and preserves empty effort', () => {
    const exact = {
      ...aliasConfig,
      native_defaults: {
        ...aliasConfig.native_defaults!,
        model_reasoning: { opus: '', 'claude-opus-4-6': 'xhigh' },
      },
    };
    expect(resolveNativeReasoningId(exact, 'opus')).toBe('');
    expect(
      getReasoningDisplayLabel(
        [{ id: '', label: 'No effort', is_default: false }],
        null,
        true,
        'Follow CLI',
        ''
      )
    ).toBe('No effort');
  });

  it('keeps a provider-global effort when only the requested model changes', () => {
    expect(resolveNativeReasoningId(config, 'another-exact-model')).toBe('max');
  });

  it('does not infer a full model ID from an unadvertised alias', () => {
    expect(getModelDisplayId(null, 'opus')).toBe('opus');
    expect(resolveNativeReasoningId(aliasConfig, 'claude-opus-4-7')).toBeNull();
  });

  it('uses Claude native display names when the exact resolved ID is not advertised', () => {
    const model: ModelInfo = {
      id: 'opus',
      name: 'Opus 4.6',
      reasoning_options: [],
    };
    expect(getModelDisplayId(model, 'opus', 'CLAUDE_CODE')).toBe('Opus 4.6');
    expect(isUnresolvedClaudeModelAlias(model, 'opus', 'CLAUDE_CODE')).toBe(
      true
    );
    expect(buildModelSelectionOverride([model], model.id)).toEqual({
      model_id: 'opus',
    });
  });

  it('distinguishes unresolved Claude aliases from advertised concrete IDs and other agents', () => {
    expect(isUnresolvedClaudeModelAlias(null, 'opus', 'CLAUDE_CODE')).toBe(
      true
    );
    expect(isUnresolvedClaudeModelAlias(null, 'opus[1m]', 'CLAUDE_CODE')).toBe(
      true
    );
    expect(
      isUnresolvedClaudeModelAlias(null, 'claude-opus-4-6', 'CLAUDE_CODE')
    ).toBe(false);
    expect(isUnresolvedClaudeModelAlias(null, 'opus', 'OPENCODE')).toBe(false);
    expect(
      isUnresolvedClaudeModelAlias(aliasConfig.models[0], 'opus', 'CLAUDE_CODE')
    ).toBe(false);
    expect(
      getModelDisplayId(aliasConfig.models[0], 'opus', 'CLAUDE_CODE')
    ).toBe('claude-opus-4-6');
  });

  it('keeps other providers IDs verbatim even when the native display name differs', () => {
    const model: ModelInfo = {
      id: 'Family/Exact:ID',
      provider_id: 'Vendor',
      name: 'Friendly name',
      reasoning_options: [],
    };
    expect(getModelDisplayId(model, 'Vendor/Family/Exact:ID', 'OPENCODE')).toBe(
      'Vendor/Family/Exact:ID'
    );
    expect(
      buildModelSelectionOverride([model], model.id, model.provider_id!)
    ).toEqual({
      model_id: 'Vendor/Family/Exact:ID',
    });
  });

  it('uses only a unique native resolved default alias when no model is configured', () => {
    const unconfigured: ModelSelectorConfig = {
      ...aliasConfig,
      native_defaults: undefined,
      models: aliasConfig.models.map((model) => ({
        ...model,
        capabilities: model.capabilities
          ? { ...model.capabilities, is_default: true }
          : model.capabilities,
      })),
    };
    expect(resolveNativeModelId(unconfigured)).toBe('opus');
    expect(
      resolveNativeModelId({ ...unconfigured, catalog_source: 'observed' })
    ).toBeNull();
    expect(
      resolveNativeModelId({ ...unconfigured, catalog_source: 'builtin' })
    ).toBeNull();
    expect(
      resolveNativeModelId({ ...config, native_defaults: undefined })
    ).toBeNull();
    expect(
      resolveNativeModelId({
        ...unconfigured,
        models: [
          ...unconfigured.models,
          { ...unconfigured.models[0], id: 'other' },
        ],
      })
    ).toBeNull();
  });
});

describe('model selector reasoning overrides', () => {
  it('does not write discovery default reasoning when selecting a model', () => {
    expect(buildModelSelectionOverride(models, 'gpt-5.5')).toEqual({
      model_id: 'gpt-5.5',
    });
  });

  it('includes provider-scoped model ids without reasoning overrides', () => {
    expect(buildModelSelectionOverride(models, 'sonnet', 'anthropic')).toEqual({
      model_id: 'anthropic/sonnet',
    });
  });

  it('returns only the model override when the model has no reasoning options', () => {
    expect(buildModelSelectionOverride(models, 'haiku', 'anthropic')).toEqual({
      model_id: 'anthropic/haiku',
    });
  });

  it('keeps the discovery default available only as metadata', () => {
    expect(resolveDefaultReasoningId(models[0].reasoning_options)).toBe(
      'xhigh'
    );
  });

  it('does not invent a default reasoning option from list order', () => {
    expect(
      resolveDefaultReasoningId([
        { id: 'medium', label: 'Medium', is_default: false },
        { id: 'high', label: 'High', is_default: false },
      ])
    ).toBeNull();
  });

  it('validates preferred reasoning without falling back to the model default', () => {
    expect(
      resolveReasoningIdForOptions(models[0].reasoning_options, 'medium')
    ).toBe('medium');
    expect(
      resolveReasoningIdForOptions(models[0].reasoning_options, 'low')
    ).toBeNull();
    expect(
      resolveReasoningIdForOptions(models[0].reasoning_options, null)
    ).toBeNull();
  });

  it('resolves only configured reasoning for display and send semantics', () => {
    expect(
      resolveConfiguredReasoningIdForOptions(
        models[0].reasoning_options,
        'xhigh',
        true
      )
    ).toBe('xhigh');
    expect(
      resolveConfiguredReasoningIdForOptions(
        models[0].reasoning_options,
        'xhigh',
        false
      )
    ).toBeNull();
    expect(
      resolveConfiguredReasoningIdForOptions(
        models[0].reasoning_options,
        'low',
        true
      )
    ).toBe('low');
    expect(
      resolveConfiguredReasoningIdForOptions(
        models[0].reasoning_options,
        null,
        true
      )
    ).toBeNull();
  });

  it('retains unavailable explicit values, including an opaque empty string', () => {
    for (const options of [models[0].reasoning_options, []]) {
      for (const value of ['low', 'Vendor/Exact:Effort', '']) {
        expect(resolveReasoningOverrideState(options, value, true)).toEqual({
          selectedReasoningId: value,
          unavailable: true,
        });
      }
    }
  });

  it('keeps the container state on CLI config when no override exists', () => {
    expect(
      resolveReasoningOverrideState(
        models[0].reasoning_options,
        undefined,
        false
      )
    ).toEqual({
      selectedReasoningId: null,
      unavailable: false,
    });
  });

  it('preserves stale container state for explicit correction or runtime validation', () => {
    expect(
      resolveReasoningOverrideState(
        models[0].reasoning_options,
        'unsupported',
        true
      )
    ).toEqual({
      selectedReasoningId: 'unsupported',
      unavailable: true,
    });
  });
});

describe('Codex active-mode reasoning selection', () => {
  const preset: ExecutorConfig = {
    executor: 'CODEX',
    permission_policy: PermissionPolicy.PLAN,
    reasoning_id: 'xhigh',
    provider_options: {
      provider: 'codex',
      service_tier: 'priority',
      plan_mode_reasoning_effort: 'medium',
    },
  };

  it('uses the plan effort for a plan preset, preserving ordinary reasoning', () => {
    const request: ExecutorConfig = {
      executor: 'CODEX',
      reasoning_id: 'xhigh',
      provider_options: {
        provider: 'codex',
        plan_mode_reasoning_effort: 'high',
      },
    };
    expect(resolveActiveReasoning('CODEX', request, preset, 'max')).toEqual({
      isCodexPlan: true,
      configuredReasoningId: 'high',
      inheritedReasoningId: 'medium',
    });
    expect(request.reasoning_id).toBe('xhigh');
  });

  for (const policy of [PermissionPolicy.AUTO, PermissionPolicy.SUPERVISED]) {
    it(`uses ordinary reasoning when ${policy} overrides a plan preset`, () => {
      const request: ExecutorConfig = {
        ...preset,
        variant: 'PLAN',
        permission_policy: policy,
        reasoning_id: null,
      };
      expect(resolveActiveReasoning('CODEX', request, preset, 'high')).toEqual({
        isCodexPlan: false,
        configuredReasoningId: null,
        inheritedReasoningId: 'high',
      });
    });
  }

  it('selects plan mode by effective policy rather than a preset name', () => {
    expect(
      resolveActiveReasoning(
        'CODEX',
        {
          executor: 'CODEX',
          variant: 'CUSTOM',
          permission_policy: PermissionPolicy.PLAN,
        },
        null,
        'xhigh'
      )
    ).toEqual({
      isCodexPlan: true,
      configuredReasoningId: undefined,
      inheritedReasoningId: null,
    });
    expect(
      resolveActiveReasoning(
        'CODEX',
        { executor: 'CODEX', variant: 'PLAN' },
        {
          executor: 'CODEX',
          permission_policy: PermissionPolicy.AUTO,
          reasoning_id: 'high',
        },
        'xhigh'
      ).isCodexPlan
    ).toBe(false);
  });

  for (const planEffort of [undefined, null]) {
    it(`inherits the preset plan effort for ${String(planEffort)}, without borrowing ordinary native defaults`, () => {
      const request: ExecutorConfig = {
        executor: 'CODEX',
        permission_policy: null,
        reasoning_id: 'max',
        provider_options: {
          provider: 'codex',
          plan_mode_reasoning_effort: planEffort,
        },
      };
      const active = resolveActiveReasoning('CODEX', request, preset, 'xhigh');
      expect(active).toEqual({
        isCodexPlan: true,
        configuredReasoningId: planEffort,
        inheritedReasoningId: 'medium',
      });
      expect(
        getReasoningDisplayLabel(
          models[0].reasoning_options,
          active.configuredReasoningId,
          active.configuredReasoningId !== undefined,
          'Inherit settings',
          active.inheritedReasoningId
        )
      ).toBe('Medium');

      const unknown = resolveActiveReasoning(
        'CODEX',
        { ...request, permission_policy: PermissionPolicy.PLAN },
        null,
        'xhigh'
      );
      expect(unknown.inheritedReasoningId).toBeNull();
      expect(
        getReasoningDisplayLabel(
          models[0].reasoning_options,
          unknown.configuredReasoningId,
          unknown.configuredReasoningId !== undefined,
          'Inherit settings',
          unknown.inheritedReasoningId
        )
      ).toBe('Inherit settings');
    });
  }

  it('changes only plan effort and retains service tier, ordinary effort and opaque IDs', () => {
    const request: ExecutorConfig = { ...preset, reasoning_id: null };
    const change = buildReasoningSelectionOverride({
      isCodexPlan: true,
      reasoningId: 'Vendor/Exact:Effort',
      providerOptions: request.provider_options,
    });
    expect(change).toEqual({
      provider_options: {
        provider: 'codex',
        service_tier: 'priority',
        plan_mode_reasoning_effort: 'Vendor/Exact:Effort',
      },
    });
    expect({ ...request, ...change }.reasoning_id).toBeNull();
    expect(request.provider_options).toEqual(preset.provider_options);
  });

  it('preserves explicit null for plan inheritance and does not clear ordinary reasoning', () => {
    const change = buildReasoningSelectionOverride({
      isCodexPlan: true,
      reasoningId: null,
      providerOptions: preset.provider_options,
    });
    const request: ExecutorConfig = JSON.parse(
      JSON.stringify({ ...preset, ...change })
    );
    expect(request.reasoning_id).toBe('xhigh');
    expect(request.provider_options).toEqual({
      provider: 'codex',
      service_tier: 'priority',
      plan_mode_reasoning_effort: null,
    });
    expect(resolveActiveReasoning('CODEX', request, preset, 'max')).toEqual({
      isCodexPlan: true,
      configuredReasoningId: null,
      inheritedReasoningId: 'medium',
    });
  });

  it('leaves ordinary and non-Codex reasoning overrides unchanged', () => {
    expect(
      buildReasoningSelectionOverride({
        isCodexPlan: false,
        reasoningId: null,
        providerOptions: preset.provider_options,
      })
    ).toEqual({ reasoning_id: null });
    expect(
      resolveActiveReasoning(
        'CLAUDE_CODE',
        { executor: 'CLAUDE_CODE', permission_policy: PermissionPolicy.PLAN },
        { executor: 'CLAUDE_CODE', reasoning_id: 'high' },
        'xhigh'
      )
    ).toEqual({
      isCodexPlan: false,
      configuredReasoningId: 'high',
      inheritedReasoningId: 'xhigh',
    });
  });
});
