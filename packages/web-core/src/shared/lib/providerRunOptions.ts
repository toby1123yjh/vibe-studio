import type { ProviderRunOptions } from 'shared/types';

/** Validate graph JSON at its boundary; keep native values verbatim. */
export function parseProviderRunOptions(
  value: unknown
): ProviderRunOptions | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const fields = value as Record<string, unknown>;
  const allowed = (keys: string[]) =>
    Object.keys(fields).every(
      (key) => key === 'provider' || keys.includes(key)
    );
  const optional = (key: string, type: 'string' | 'boolean') =>
    fields[key] == null || typeof fields[key] === type;
  switch (fields.provider) {
    case 'codex':
      if (!allowed(['service_tier', 'plan_mode_reasoning_effort'])) return null;
      if (
        !optional('service_tier', 'string') ||
        !optional('plan_mode_reasoning_effort', 'string')
      )
        return null;
      return {
        provider: 'codex',
        service_tier: fields.service_tier as string | undefined,
        plan_mode_reasoning_effort: fields.plan_mode_reasoning_effort as
          | string
          | undefined,
      };
    case 'claude_code':
      if (!allowed(['fast_mode', 'permission_mode', 'ultracode'])) return null;
      if (
        !optional('fast_mode', 'boolean') ||
        !optional('permission_mode', 'string') ||
        !optional('ultracode', 'boolean')
      )
        return null;
      return {
        provider: 'claude_code',
        fast_mode: fields.fast_mode as boolean | undefined,
        permission_mode: fields.permission_mode as string | undefined,
        ultracode: fields.ultracode as boolean | undefined,
      };
    case 'oh_my_pi':
      if (!allowed(['auto_compaction', 'auto_retry'])) return null;
      if (
        !optional('auto_compaction', 'boolean') ||
        !optional('auto_retry', 'boolean')
      )
        return null;
      return {
        provider: 'oh_my_pi',
        auto_compaction: fields.auto_compaction as boolean | undefined,
        auto_retry: fields.auto_retry as boolean | undefined,
      };
    default:
      return null;
  }
}
