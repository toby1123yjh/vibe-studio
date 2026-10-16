import { expect, test } from '@playwright/test';
import type { BaseCodingAgent, PermissionPolicy } from 'shared/types';
import {
  restoreExecutorConfig,
  resolveExecutorOverrides,
} from '../../../shared/lib/executorConfig';
import { areProfilesEqual } from '../../../shared/lib/executor';

const executor = 'CODEX' as BaseCodingAgent;
const preset = {
  executor,
  variant: 'PLAN',
  model_id: 'preset-model',
  agent_id: 'preset-agent',
  reasoning_id: 'high',
  permission_policy: 'AUTO' as PermissionPolicy,
};

test('canonical omitted overrides are full-snapshot nulls, not preset fallbacks', () => {
  const saved = restoreExecutorConfig({ executor, variant: 'PLAN' });
  expect(
    resolveExecutorOverrides(executor, 'PLAN', {}, undefined, saved, preset)
  ).toEqual({
    executor,
    variant: 'PLAN',
    model_id: null,
    agent_id: null,
    reasoning_id: null,
    permission_policy: null,
    provider_options: null,
  });
});

test('explicit null draft fields override saved values while missing fields inherit', () => {
  const scratch = {
    executor,
    variant: 'PLAN',
    model_id: null,
    reasoning_id: null,
  };
  expect(
    resolveExecutorOverrides(executor, 'PLAN', {}, scratch, preset, null)
  ).toEqual({
    ...preset,
    model_id: null,
    reasoning_id: null,
  });
});

test('default aliases match but foreign provider and named profiles do not', () => {
  expect(areProfilesEqual({ executor }, { executor, variant: 'DEFAULT' })).toBe(
    true
  );
  expect(areProfilesEqual({ executor }, { executor, variant: 'default' })).toBe(
    false
  );
  expect(areProfilesEqual({ executor }, { ...preset })).toBe(false);
  expect(
    areProfilesEqual(
      { executor },
      { executor: 'CLAUDE_CODE' as BaseCodingAgent }
    )
  ).toBe(false);
});

test('foreign profiles stay isolated and saved explicit effort is retained after a model change', () => {
  const resolved = resolveExecutorOverrides(
    executor,
    'PLAN',
    { model_id: 'selected-model' },
    { ...preset, variant: 'OTHER', agent_id: 'foreign' },
    restoreExecutorConfig(preset),
    preset
  );
  expect(resolved?.model_id).toBe('selected-model');
  expect(resolved?.agent_id).toBe('preset-agent');
  expect(resolved?.reasoning_id).toBe('high');
});

test('provider options survive drafts, restoration and explicit clearing', () => {
  const saved = restoreExecutorConfig({
    ...preset,
    provider_options: { provider: 'codex', service_tier: 'priority' },
  });
  expect(
    resolveExecutorOverrides(executor, 'PLAN', {}, undefined, saved, preset)
      ?.provider_options
  ).toEqual(saved.provider_options);
  expect(
    resolveExecutorOverrides(
      executor,
      'PLAN',
      { provider_options: null },
      undefined,
      saved,
      preset
    )?.provider_options
  ).toBeNull();
});
