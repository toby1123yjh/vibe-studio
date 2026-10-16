import { expect, test } from '@playwright/test';
import {
  resolveModelDisplayId,
  getReasoningDisplayLabel,
} from '../../../shared/lib/modelSelector';
import {
  agentInstallRequest,
  isValidInstallRegistry,
} from '../../../shared/lib/agentInstall';
import type { BaseCodingAgent } from 'shared/types';
import {
  advanceAgentCenterScope,
  canPublishAgentCenterOperation,
  isAgentCenterScopeCurrent,
  projectAgentCenterSource,
} from './agentCenterState';

test('model display preserves full provider IDs and runtime punctuation', () => {
  expect(
    resolveModelDisplayId(
      'custom/model/family:Latest (Fast)',
      'preset',
      'default'
    )
  ).toBe('custom/model/family:Latest (Fast)');
});

test('reasoning display uses explicit effort or follows CLI without adopting defaults', () => {
  const options = [
    { id: 'high', label: 'High', is_default: false },
    { id: 'xhigh', label: 'Extra High', is_default: true },
  ];
  expect(
    getReasoningDisplayLabel(options, 'high', true, 'Follow CLI config')
  ).toBe('High');
  expect(
    getReasoningDisplayLabel(options, 'xhigh', true, 'Follow CLI config')
  ).toBe('Extra High');
  for (const effort of [null, undefined]) {
    expect(
      getReasoningDisplayLabel(options, effort, true, '遵循 CLI 配置')
    ).toBe('遵循 CLI 配置');
  }
  expect(
    getReasoningDisplayLabel(options, 'high', false, 'Follow CLI config')
  ).toBe('Follow CLI config');
  expect(getReasoningDisplayLabel([], 'high', true, 'Follow CLI config')).toBe(
    'High'
  );
  expect(
    getReasoningDisplayLabel([], null, true, 'Follow CLI config')
  ).toBeNull();
});

test('model display honors overrides and displays CLI defaults without changing null', () => {
  expect(resolveModelDisplayId(undefined, 'provider/preset', 'fallback')).toBe(
    'provider/preset'
  );
  expect(resolveModelDisplayId(null, 'provider/preset', 'provider/cli')).toBe(
    'provider/cli'
  );
});

test('model display never invents a model or expands runtime aliases', () => {
  expect(resolveModelDisplayId(undefined, null, 'opus')).toBe('opus');
  expect(resolveModelDisplayId(null, 'preset', null)).toBeNull();
  expect(resolveModelDisplayId(undefined, undefined, undefined)).toBeNull();
  expect(resolveModelDisplayId('', 'preset', 'fallback')).toBeNull();
});
test('installer preserves npm defaults and never sends npm options to native installers', () => {
  for (const executor of ['CODEX', 'GEMINI'] as BaseCodingAgent[]) {
    expect(agentInstallRequest(executor, '  ')).toEqual({ executor });
    expect(
      agentInstallRequest(executor, ' https://registry.example.test/ ')
    ).toEqual({ executor, npm_registry: 'https://registry.example.test/' });
  }
  expect(
    agentInstallRequest(
      'CLAUDE_CODE' as BaseCodingAgent,
      'https://registry.example.test/'
    )
  ).toEqual({ executor: 'CLAUDE_CODE' });
  expect(
    agentInstallRequest(
      'OH_MY_PI' as BaseCodingAgent,
      'https://registry.example.test/'
    )
  ).toEqual({ executor: 'OH_MY_PI' });
});

test('installer registry validation rejects unsafe or malformed URLs', () => {
  for (const registry of [
    '',
    ' https://registry.example.test/path ',
    'http://localhost:4873',
  ]) {
    expect(isValidInstallRegistry(registry)).toBe(true);
  }
  for (const registry of [
    'invalid',
    'file:///tmp/npm',
    'https://user:password@example.test',
    'https://example.test/?token=value',
    'https://example.test/#fragment',
    `https://example.test/${'x'.repeat(2048)}`,
  ]) {
    expect(isValidInstallRegistry(registry)).toBe(false);
  }
});
const baseFacts = {
  hasCanonicalData: false,
  isLoading: false,
  isFetching: false,
  error: null,
  diagnosticCount: 0,
  capabilityAvailable: true,
};

test.describe('Agent Center source projection', () => {
  test('keeps first load, initial error, and successful unavailable distinct', () => {
    expect(
      projectAgentCenterSource({
        ...baseFacts,
        isLoading: true,
        isFetching: true,
      }).state
    ).toBe('loading');
    expect(
      projectAgentCenterSource({
        ...baseFacts,
        error: new Error('offline'),
      }).state
    ).toBe('error');
    expect(projectAgentCenterSource(baseFacts).state).toBe('unavailable');
  });

  test('preserves cached data as degraded and fails closed for writes', () => {
    expect(
      projectAgentCenterSource({
        ...baseFacts,
        hasCanonicalData: true,
        error: new Error('refresh failed'),
      })
    ).toEqual({
      state: 'degraded',
      hasUsableData: true,
      isRefreshing: false,
      canMutate: false,
    });
    expect(
      projectAgentCenterSource({
        ...baseFacts,
        hasCanonicalData: true,
        diagnosticCount: 1,
      }).state
    ).toBe('degraded');
  });

  test('keeps usable content during an ordinary refresh', () => {
    expect(
      projectAgentCenterSource({
        ...baseFacts,
        hasCanonicalData: true,
        isFetching: true,
      })
    ).toEqual({
      state: 'ready',
      hasUsableData: true,
      isRefreshing: true,
      canMutate: true,
    });
  });

  test('does not turn a successful missing capability into fake counts', () => {
    expect(
      projectAgentCenterSource({
        ...baseFacts,
        hasCanonicalData: true,
        capabilityAvailable: false,
      })
    ).toEqual({
      state: 'unavailable',
      hasUsableData: true,
      isRefreshing: false,
      canMutate: false,
    });
  });
});

test.describe('Agent Center scope epochs', () => {
  test('rejects an old A completion after an A to B to A transition', () => {
    const firstA = { identity: 'A', epoch: 0 };
    const nextB = advanceAgentCenterScope(firstA, 'B');
    const nextA = advanceAgentCenterScope(nextB, 'A');

    expect(nextB).toEqual({ identity: 'B', epoch: 1 });
    expect(nextA).toEqual({ identity: 'A', epoch: 2 });
    expect(isAgentCenterScopeCurrent(firstA, nextA)).toBe(false);
    expect(isAgentCenterScopeCurrent(nextA, nextA)).toBe(true);
    expect(canPublishAgentCenterOperation(firstA, nextA, true)).toBe(false);
    expect(canPublishAgentCenterOperation(nextA, nextA, true)).toBe(true);
  });

  test('rejects completion after the owner unmounts', () => {
    const scope = { identity: 'A', epoch: 4 };

    expect(canPublishAgentCenterOperation(scope, scope, false)).toBe(false);
  });
});
