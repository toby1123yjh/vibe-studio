import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  BaseCodingAgent,
  type ExecutorProfile,
  type ModelSelectorConfig,
} from 'shared/types';
import {
  appendPresetModel,
  buildModelSelectionOverride,
  getReasoningDisplayLabel,
  getSelectedModel,
  isModelAvailable,
  parseModelId,
  resolveDefaultModelId,
  resolveReasoningOverrideState,
} from './modelSelector';
import {
  getRecentModelEntries,
  getRecentReasoningByModel,
  setRecentReasoning,
  sortByRecency,
  touchRecentModel,
} from './recentModels';

test('ACP model IDs remain opaque when no separate provider IDs are advertised', () => {
  const modelId = 'vendor/model:preview/variant';
  assert.deepEqual(parseModelId(modelId, false), {
    providerId: null,
    modelId,
  });
  assert.deepEqual(buildModelSelectionOverride([], modelId), {
    model_id: modelId,
  });
});

test('missing discovery does not invent a default ACP model or reasoning level', () => {
  assert.equal(resolveDefaultModelId([], null, null, false), null);
  assert.deepEqual(resolveReasoningOverrideState([], 'high', true), {
    selectedReasoningId: 'high',
    unavailable: true,
  });
});

test('reasoning choices are accepted only from live advertised options', () => {
  const options = [
    { id: 'provider-specific-effort', label: 'Custom', is_default: true },
  ];
  assert.deepEqual(
    resolveReasoningOverrideState(options, 'provider-specific-effort', true),
    {
      selectedReasoningId: 'provider-specific-effort',
      unavailable: false,
    }
  );
  assert.deepEqual(resolveReasoningOverrideState(options, undefined, false), {
    selectedReasoningId: null,
    unavailable: false,
  });
});

test('an advertised empty effort remains distinct from following CLI configuration', () => {
  const options = [
    { id: '', label: 'No effort', is_default: false },
    { id: 'high', label: 'High', is_default: true },
  ];
  assert.deepEqual(resolveReasoningOverrideState(options, '', true), {
    selectedReasoningId: '',
    unavailable: false,
  });
  assert.equal(
    getReasoningDisplayLabel(options, '', true, 'Follow CLI'),
    'No effort'
  );
  assert.deepEqual(resolveReasoningOverrideState(options, null, true), {
    selectedReasoningId: null,
    unavailable: false,
  });
  assert.deepEqual(resolveReasoningOverrideState([options[1]], '', true), {
    selectedReasoningId: '',
    unavailable: true,
  });
});

test('opaque model identity selects the exact effort catalog without case folding', () => {
  const config: ModelSelectorConfig = {
    providers: [],
    agents: [],
    permissions: [],
    models: [
      {
        id: 'vendor/Foo',
        name: 'Uppercase route',
        reasoning_options: [{ id: 'upper', label: 'Upper', is_default: true }],
      },
      {
        id: 'vendor/foo',
        name: 'Lowercase route',
        reasoning_options: [{ id: 'lower', label: 'Lower', is_default: true }],
      },
    ],
  };
  assert.equal(
    getSelectedModel(config.models, null, 'vendor/foo'),
    config.models[1]
  );
  assert.equal(getSelectedModel(config.models, null, 'vendor/FOO'), null);
  const appended = appendPresetModel(config, 'vendor/FOO');
  assert.equal(appended?.models.length, 3);
  assert.equal(appended?.models[0].id, 'vendor/FOO');

  const providerConfig = {
    ...config,
    providers: [{ id: 'Vendor', name: 'Vendor' }],
    models: config.models.map((model) => ({ ...model, provider_id: 'Vendor' })),
  };
  assert.equal(isModelAvailable(providerConfig, 'Vendor', 'vendor/foo'), true);
  assert.equal(isModelAvailable(providerConfig, 'vendor', 'vendor/foo'), false);
  assert.equal(
    getSelectedModel(providerConfig.models, 'vendor', 'vendor/foo'),
    null
  );
});

test('recent preferences retain advertised empty effort and opaque model identity', () => {
  const model = { id: ' vendor/Foo ', name: 'Upper', reasoning_options: [] };
  const lower = { id: ' vendor/foo ', name: 'Lower', reasoning_options: [] };
  const profiles: Record<string, ExecutorProfile> = {};
  const saved = setRecentReasoning(
    profiles,
    BaseCodingAgent.OPENCODE,
    model,
    ''
  );
  assert.deepEqual(getRecentReasoningByModel(saved, BaseCodingAgent.OPENCODE), {
    ' vendor/Foo ': '',
  });
  assert.deepEqual(
    getRecentReasoningByModel(
      setRecentReasoning(saved, BaseCodingAgent.OPENCODE, model, null),
      BaseCodingAgent.OPENCODE
    ),
    {}
  );
  const entries = touchRecentModel([model.id], lower);
  assert.deepEqual(entries, [model.id, lower.id]);
  assert.deepEqual(sortByRecency([model, lower], [lower.id], 'top'), [
    lower,
    model,
  ]);
  const recentProfile: ExecutorProfile = {};
  recentProfile.recently_used_models = { models: entries };
  const recent = { OPENCODE: recentProfile };
  assert.deepEqual(
    getRecentModelEntries(recent, BaseCodingAgent.OPENCODE),
    entries
  );
});

test('recent effort skips absent map values without dropping an advertised empty value', () => {
  const profile: ExecutorProfile = {};
  profile.recently_used_models = {
    reasoning_by_model: { missing: undefined, empty: '', high: 'high' },
  };
  const profiles = { OPENCODE: profile };
  assert.deepEqual(
    getRecentReasoningByModel(profiles, BaseCodingAgent.OPENCODE),
    { empty: '', high: 'high' }
  );
});
