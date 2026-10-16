import { BaseCodingAgent, type ExecutorConfig } from 'shared/types';
import { parseProviderRunOptions } from '@/shared/lib/providerRunOptions';
import type { WorkflowNode } from './workflowGraph';

const BASE_CODING_AGENT_VALUES = new Set<BaseCodingAgent>(
  Object.values(BaseCodingAgent)
);

function coerceBaseCodingAgent(value: unknown): BaseCodingAgent | null {
  if (typeof value !== 'string') return null;

  const normalized = value.trim().replaceAll('-', '_').toUpperCase();
  if (BASE_CODING_AGENT_VALUES.has(normalized as BaseCodingAgent)) {
    return normalized as BaseCodingAgent;
  }
  return null;
}

export function isWorkflowAgentDraftNode(
  node: WorkflowNode | null
): node is WorkflowNode & { type: 'agent' } {
  return node?.type === 'agent';
}

export function coerceWorkflowNodeExecutorConfig(
  value: unknown
): ExecutorConfig | null {
  if (!value || typeof value !== 'object') return null;

  const candidate = value as Record<string, unknown>;
  const executor = coerceBaseCodingAgent(candidate.executor);
  if (!executor) return null;

  const config: ExecutorConfig = {
    executor,
    variant:
      typeof candidate.variant === 'string' || candidate.variant === null
        ? candidate.variant
        : null,
  };

  for (const key of [
    'model_id',
    'agent_id',
    'reasoning_id',
    'permission_policy',
  ] as const) {
    const value = candidate[key];
    if (value === null || typeof value === 'string') {
      config[key] = value as never;
    }
  }

  if (candidate.provider_options === null) config.provider_options = null;
  else if (candidate.provider_options !== undefined) {
    const options = parseProviderRunOptions(candidate.provider_options);
    if (!options || options.provider.toUpperCase() !== executor) return null;
    config.provider_options = options;
  }
  return config;
}
