import type { BaseCodingAgent, ExecutorConfig } from 'shared/types';
import { areProfilesEqual } from './executor';

export function executorProfileKey(
  executor: BaseCodingAgent | null,
  variant?: string | null
): string | null {
  return executor ? `${executor}:${variant ?? 'DEFAULT'}` : null;
}

/** A persisted request is a complete snapshot, not a partial preset patch. */
export function restoreExecutorConfig(config: ExecutorConfig): ExecutorConfig {
  return {
    executor: config.executor,
    variant: config.variant ?? null,
    model_id: config.model_id ?? null,
    agent_id: config.agent_id ?? null,
    reasoning_id: config.reasoning_id ?? null,
    permission_policy: config.permission_policy ?? null,
    provider_options: config.provider_options ?? null,
  };
}

/** Undefined falls through; explicit null means follow CLI configuration. */
export function resolveExecutorOverrides(
  executor: BaseCodingAgent | null,
  variant: string | null,
  selections: Partial<ExecutorConfig>,
  scratch: ExecutorConfig | null | undefined,
  lastUsed: ExecutorConfig | null,
  preset: ExecutorConfig | null | undefined
): ExecutorConfig | null {
  if (!executor) return null;
  const resolved: ExecutorConfig = { executor, variant };
  const sources = [
    selections,
    scratch && areProfilesEqual(scratch, resolved) ? scratch : undefined,
    lastUsed && areProfilesEqual(lastUsed, resolved) ? lastUsed : undefined,
    preset,
  ];
  for (const field of [
    'model_id',
    'agent_id',
    'reasoning_id',
    'permission_policy',
    'provider_options',
  ] as const) {
    for (const source of sources) {
      if (source?.[field] === undefined) {
        continue;
      }
      Object.assign(resolved, { [field]: source[field] });
      break;
    }
  }
  return resolved;
}
