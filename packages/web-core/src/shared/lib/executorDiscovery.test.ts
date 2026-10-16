import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BaseCodingAgent } from 'shared/types';
import {
  buildExecutorDiscoveryStreamUrl,
  resolveModelDiscoveryVariant,
} from './executorDiscovery';

test('ACP model discovery carries the selected profile and workspace scope', () => {
  for (const agent of [
    BaseCodingAgent.OPENCODE,
    BaseCodingAgent.DEEPSEEK_HARNESS,
  ]) {
    const variant = resolveModelDiscoveryVariant(agent, 'CUSTOM_ROUTE');
    assert.equal(variant, 'CUSTOM_ROUTE');
    const url = new URL(
      buildExecutorDiscoveryStreamUrl(agent, {
        variant,
        sessionId: 'session-1',
        workspaceId: 'workspace-1',
        repoId: 'repo-1',
      }),
      'http://localhost'
    );
    assert.equal(url.searchParams.get('executor'), agent);
    assert.equal(url.searchParams.get('variant'), 'CUSTOM_ROUTE');
    assert.equal(url.searchParams.get('session_id'), 'session-1');
    assert.equal(url.searchParams.get('workspace_id'), 'workspace-1');
    assert.equal(url.searchParams.get('repo_id'), 'repo-1');
    assert.equal(resolveModelDiscoveryVariant(agent, 'DEFAULT'), 'DEFAULT');
    assert.equal(resolveModelDiscoveryVariant(agent, null), undefined);
    for (const blank of ['', '   ']) {
      const defaultVariant = resolveModelDiscoveryVariant(agent, blank);
      assert.equal(defaultVariant, undefined);
      assert.equal(
        new URL(
          buildExecutorDiscoveryStreamUrl(agent, { variant: defaultVariant }),
          'http://localhost'
        ).searchParams.has('variant'),
        false
      );
    }
  }
});

test('all providers discover the actual selected profile', () => {
  for (const agent of [
    BaseCodingAgent.GEMINI,
    BaseCodingAgent.CODEX,
    BaseCodingAgent.CLAUDE_CODE,
    BaseCodingAgent.OH_MY_PI,
  ]) {
    const variant = resolveModelDiscoveryVariant(agent, 'CUSTOM_ROUTE');
    assert.equal(variant, 'CUSTOM_ROUTE');
    assert.equal(
      buildExecutorDiscoveryStreamUrl(agent, { variant }),
      `/api/agents/discovered-options/ws?executor=${agent}&variant=CUSTOM_ROUTE`
    );
  }
});

test('explicit rescan retains profile and workspace scope', () => {
  const url = new URL(
    buildExecutorDiscoveryStreamUrl(BaseCodingAgent.CODEX, {
      workspaceId: 'workspace',
      sessionId: 'session',
      variant: 'CUSTOM',
      refresh: true,
    }),
    'http://localhost'
  );
  assert.equal(url.searchParams.get('refresh'), 'true');
  assert.equal(url.searchParams.get('variant'), 'CUSTOM');
  assert.equal(url.searchParams.get('workspace_id'), 'workspace');
  assert.equal(url.searchParams.get('session_id'), 'session');
});
