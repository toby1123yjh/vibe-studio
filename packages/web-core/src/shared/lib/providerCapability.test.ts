import { expect, test } from '@playwright/test';
import { parseProviderRunOptions } from './providerRunOptions';
import { supportsImageInput } from '../hooks/useModelAttachmentGuard';
import { getCurrentObservedConfiguration } from './modelSelector';

test('native observations never leak across sessions or attempts', () => {
  const observed = {
    model_id: 'resolved-model',
    reasoning_id: 'medium',
    source: 'native',
    event_id: 'event',
    run_attempt_id: 'attempt-1',
  };
  const state = {
    session_id: 'session',
    last_run_attempt_id: 'attempt-1',
    observed_configuration: observed,
  };
  expect(getCurrentObservedConfiguration(state, 'session')).toBe(observed);
  expect(getCurrentObservedConfiguration(state, 'another')).toBeNull();
  expect(
    getCurrentObservedConfiguration(
      { ...state, last_run_attempt_id: 'attempt-2' },
      'session'
    )
  ).toBeNull();
});

test('input capabilities distinguish unknown from explicitly text-only', () => {
  expect(supportsImageInput(undefined)).toBeUndefined();
  expect(supportsImageInput([])).toBeUndefined();
  expect(supportsImageInput(['text'])).toBe(false);
  expect(supportsImageInput(['text', 'image'])).toBe(true);
});

test('provider settings keep opaque native values and explicit false', () => {
  expect(
    parseProviderRunOptions({
      provider: 'claude_code',
      fast_mode: false,
      permission_mode: 'auto',
      ultracode: true,
    })
  ).toEqual({
    provider: 'claude_code',
    fast_mode: false,
    permission_mode: 'auto',
    ultracode: true,
  });
  expect(
    parseProviderRunOptions({
      provider: 'codex',
      service_tier: 'flex',
      plan_mode_reasoning_effort: 'future/effort',
    })
  ).toEqual({
    provider: 'codex',
    service_tier: 'flex',
    plan_mode_reasoning_effort: 'future/effort',
  });
  expect(parseProviderRunOptions({ provider: 'gemini' })).toBeNull();
  expect(
    parseProviderRunOptions({ provider: 'oh_my_pi', auto_retry: 'false' })
  ).toBeNull();
  expect(
    parseProviderRunOptions({ provider: 'codex', typo_tier: 'priority' })
  ).toBeNull();
});
