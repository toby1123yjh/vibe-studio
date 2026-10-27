import { useTranslation } from 'react-i18next';
import type {
  BaseCodingAgent,
  ExecutorConfig,
  ModelInfo,
  ProviderRunOptions,
} from 'shared/types';
import {
  DropdownMenuLabel,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
} from '@vibe/ui/components/Dropdown';

export const RUN_SETTINGS_SUBMENU_CLASS_NAME =
  'min-w-0 max-w-[calc(100vw_-_1rem)] max-h-[var(--radix-dropdown-menu-content-available-height)] overflow-y-auto ' +
  'data-[side=right]:[translate:min(0px,calc(var(--radix-dropdown-menu-content-available-width)_-_100%))_0] ' +
  'data-[side=left]:[translate:max(0px,calc(100%_-_var(--radix-dropdown-menu-content-available-width)))_0]';

function OptionMenu({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string | null | undefined;
  options: { id: string; label: string; disabled?: boolean }[];
  onChange: (value: string | null) => void;
}) {
  const { t } = useTranslation('common');
  // Index encoding keeps the native empty-string value distinct from inheritance.
  const selected =
    value == null
      ? 'inherit'
      : String(options.findIndex((option) => option.id === value));
  const currentLabel =
    value == null
      ? t('modelSelector.inheritSettings')
      : (options.find((option) => option.id === value)?.label ??
        (value === '' ? '""' : value));
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger className="min-h-9 min-w-0 gap-half">
        <span className="min-w-0 flex-1">{label}</span>
        <span className="max-w-[55%] truncate text-low" title={currentLabel}>
          {currentLabel}
        </span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent
        className={`${RUN_SETTINGS_SUBMENU_CLASS_NAME} w-72`}
      >
        <DropdownMenuRadioGroup
          value={selected}
          onValueChange={(id) =>
            onChange(id === 'inherit' ? null : options[Number(id)].id)
          }
        >
          <DropdownMenuRadioItem value="inherit" className="min-h-9">
            {t('modelSelector.inheritSettings')}
          </DropdownMenuRadioItem>
          {value != null && selected === '-1' && (
            <DropdownMenuRadioItem
              value="-1"
              disabled
              className="min-h-9 break-all"
            >
              {value === '' ? '""' : value}
            </DropdownMenuRadioItem>
          )}
          {options.map((option, index) => (
            <DropdownMenuRadioItem
              key={option.id}
              value={String(index)}
              disabled={option.disabled}
              className="min-h-9 whitespace-normal"
            >
              {option.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );
}

/** Provider-owned overrides use the existing preset menu, never native settings writes. */
export function ProviderRunOptionsMenu({
  agent,
  options,
  model,
  onChange,
}: {
  agent: BaseCodingAgent | null;
  options: ProviderRunOptions | null | undefined;
  model: ModelInfo | null;
  onChange: (change: Partial<ExecutorConfig>) => void;
}) {
  const { t } = useTranslation('common');
  const booleanOptions = (unsupported = false) => [
    { id: 'true', label: t('modelSelector.enabled'), disabled: unsupported },
    { id: 'false', label: t('modelSelector.disabled') },
  ];
  const booleanValue = (value: boolean | null | undefined) =>
    value == null ? null : String(value);
  const booleanChoice = (value: string | null) =>
    value == null ? undefined : value === 'true';
  const reasoningOptions = model?.reasoning_options ?? [];
  if (!['CODEX', 'CLAUDE_CODE', 'OH_MY_PI'].includes(agent ?? '')) return null;

  return (
    <>
      <DropdownMenuLabel className="pt-base">
        {t('modelSelector.providerOptions')}
      </DropdownMenuLabel>
      {agent === 'CODEX' &&
        (() => {
          const current =
            options?.provider === 'codex'
              ? options
              : { provider: 'codex' as const };
          return (
            <>
              <OptionMenu
                label={t('modelSelector.serviceTier')}
                value={current.service_tier}
                options={[
                  { id: 'default', label: t('modelSelector.standardTier') },
                  ...(model?.capabilities?.service_tiers ?? [])
                    .filter((tier) => tier.id !== 'default')
                    .map((tier) => ({ id: tier.id, label: tier.name })),
                ]}
                onChange={(value) =>
                  onChange({
                    provider_options: {
                      ...current,
                      service_tier: value ?? undefined,
                    },
                  })
                }
              />
              {(reasoningOptions.length > 0 ||
                current.plan_mode_reasoning_effort != null) && (
                <OptionMenu
                  label={t('modelSelector.planReasoning')}
                  value={current.plan_mode_reasoning_effort}
                  options={reasoningOptions}
                  onChange={(value) =>
                    onChange({
                      provider_options: {
                        ...current,
                        plan_mode_reasoning_effort: value ?? undefined,
                      },
                    })
                  }
                />
              )}
            </>
          );
        })()}
      {agent === 'CLAUDE_CODE' &&
        (() => {
          const current =
            options?.provider === 'claude_code'
              ? options
              : { provider: 'claude_code' as const };
          return (
            <>
              <OptionMenu
                label={t('modelSelector.fastMode')}
                value={booleanValue(current.fast_mode)}
                options={booleanOptions(
                  model?.capabilities?.fast_mode !== 'supported'
                )}
                onChange={(value) =>
                  onChange({
                    provider_options: {
                      ...current,
                      fast_mode: booleanChoice(value),
                    },
                  })
                }
              />
              <OptionMenu
                label={t('modelSelector.nativePermission')}
                value={current.permission_mode}
                options={[
                  { id: 'default', label: t('modelSelector.permissionAsk') },
                  { id: 'acceptEdits', label: t('modelSelector.acceptEdits') },
                  { id: 'plan', label: t('modelSelector.permissionPlan') },
                  {
                    id: 'auto',
                    label: t('modelSelector.nativeAuto'),
                    disabled: model?.capabilities?.auto_mode !== 'supported',
                  },
                  {
                    id: 'bypassPermissions',
                    label: t('modelSelector.bypassPermissions'),
                  },
                ]}
                onChange={(value) =>
                  onChange({
                    permission_policy: null,
                    provider_options: {
                      ...current,
                      permission_mode: value ?? undefined,
                    },
                  })
                }
              />
              <OptionMenu
                label={t('modelSelector.ultracode')}
                value={booleanValue(current.ultracode)}
                options={booleanOptions()}
                onChange={(value) =>
                  onChange({
                    provider_options: {
                      ...current,
                      ultracode: booleanChoice(value),
                    },
                  })
                }
              />
            </>
          );
        })()}
      {agent === 'OH_MY_PI' &&
        (() => {
          const current =
            options?.provider === 'oh_my_pi'
              ? options
              : { provider: 'oh_my_pi' as const };
          return (
            <>
              <OptionMenu
                label={t('modelSelector.autoCompaction')}
                value={booleanValue(current.auto_compaction)}
                options={booleanOptions()}
                onChange={(value) =>
                  onChange({
                    provider_options: {
                      ...current,
                      auto_compaction: booleanChoice(value),
                    },
                  })
                }
              />
              <OptionMenu
                label={t('modelSelector.autoRetry')}
                value={booleanValue(current.auto_retry)}
                options={booleanOptions()}
                onChange={(value) =>
                  onChange({
                    provider_options: {
                      ...current,
                      auto_retry: booleanChoice(value),
                    },
                  })
                }
              />
            </>
          );
        })()}
    </>
  );
}
