import { useEffect, useMemo, useState } from 'react';
import { CheckIcon } from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { AgentIcon, getAgentName } from '@/shared/components/AgentIcon';
import { getAgentBrandAttributes } from '@/shared/lib/agentProviders';
import { ModelSelectorContainer } from '@/shared/components/ModelSelectorContainer';
import { useSettingsNavigation } from '@/shared/hooks/useSettingsNavigation';
import { useAgentProviderOptions } from '@/shared/hooks/useAgentProviderPolicy';
import { useExecutorConfig } from '@/shared/hooks/useExecutorConfig';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { getAgentProviderBlockedReasonLabel } from '@/shared/lib/agentProviderOptions';
import { cn } from '@/shared/lib/utils';
import {
  DropdownMenuItem,
  DropdownMenuLabel,
} from '@vibe/ui/components/Dropdown';
import { ToolbarDropdown } from '@vibe/ui/components/Toolbar';
import {
  AgentProviderCapability,
  type BaseCodingAgent,
  type ExecutorConfig,
} from 'shared/types';
import { coerceWorkflowNodeExecutorConfig } from '../model/workflowAgentNodeDraft';

const WORKFLOW_AGENT_REQUIRED_CAPABILITIES = [
  AgentProviderCapability.INITIAL_RUN,
  AgentProviderCapability.WORKFLOW_AGENT_STEP,
] as const;

interface WorkflowAgentExecutorFieldProps {
  value?: unknown;
  readOnly?: boolean;
  requiredCapabilities?: readonly AgentProviderCapability[];
  onChange: (value: ExecutorConfig) => void;
}

export function WorkflowAgentExecutorField({
  value,
  readOnly = false,
  requiredCapabilities = WORKFLOW_AGENT_REQUIRED_CAPABILITIES,
  onChange,
}: WorkflowAgentExecutorFieldProps) {
  const { t } = useTranslation('common');
  const { openAgentCenter } = useSettingsNavigation();
  const { profiles, config } = useUserSystem();
  const [hasExplicitSelection, setHasExplicitSelection] = useState(false);
  const storedExecutorConfig = useMemo(
    () => coerceWorkflowNodeExecutorConfig(value),
    [value]
  );

  useEffect(() => {
    setHasExplicitSelection(false);
  }, [value]);

  const {
    executorConfig,
    effectiveExecutor,
    selectedVariant,
    executorOptions,
    variantOptions,
    presetOptions,
    setExecutor,
    setVariant,
    setOverrides,
  } = useExecutorConfig({
    profiles,
    lastUsedConfig: null,
    scratchConfig: storedExecutorConfig,
    configExecutorProfile: config?.executor_profile,
    hiddenAgents: config?.hidden_agents,
    onPersist: onChange,
  });

  const policyExecutorSource = useMemo(() => {
    const next = [...executorOptions];
    if (effectiveExecutor && !next.includes(effectiveExecutor)) {
      next.unshift(effectiveExecutor);
    }
    return next;
  }, [effectiveExecutor, executorOptions]);
  const { options: policyExecutorOptions } = useAgentProviderOptions({
    executors: policyExecutorSource,
    preserveExecutors: [storedExecutorConfig?.executor],
    requiredCapabilities,
  });

  const hasSelection = storedExecutorConfig !== null || hasExplicitSelection;
  const selectedExecutor = hasSelection ? effectiveExecutor : null;
  const selectedExecutorConfig = hasSelection ? executorConfig : null;

  const handleExecutorSelect = (executor: BaseCodingAgent) => {
    setHasExplicitSelection(true);
    setExecutor(executor);
  };

  return (
    <div className="flex flex-col gap-2">
      <span className="text-xs font-semibold text-high">
        {t('workflow.agentEdit.agent', { defaultValue: 'Agent' })}
      </span>
      <div className="rounded-md border border-secondary bg-primary p-2 shadow-sm">
        <div
          {...getAgentBrandAttributes(selectedExecutor)}
          className={cn(
            'flex flex-wrap items-center gap-2',
            readOnly && 'pointer-events-none opacity-60'
          )}
          aria-disabled={readOnly}
        >
          <AgentIcon
            agent={selectedExecutor}
            className="vk-agent-brand-icon size-icon-xl shrink-0"
          />
          <ToolbarDropdown
            className="vk-agent-brand-label"
            label={
              selectedExecutor
                ? getAgentName(selectedExecutor)
                : t('workflow.agentEdit.selectExecutor', {
                    defaultValue: 'Select Agent',
                  })
            }
            disabled={readOnly}
          >
            <DropdownMenuLabel>
              {t('workflow.agentEdit.agent', { defaultValue: 'Agent' })}
            </DropdownMenuLabel>
            {policyExecutorOptions.map((option) => (
              <DropdownMenuItem
                key={option.executor}
                icon={
                  selectedExecutor === option.executor ? CheckIcon : undefined
                }
                disabled={!option.enabled}
                badge={
                  getAgentProviderBlockedReasonLabel(option.disabledReason) ??
                  undefined
                }
                onClick={() => handleExecutorSelect(option.executor)}
              >
                <span className="flex items-center gap-2">
                  <AgentIcon
                    agent={option.executor}
                    className="size-icon-base shrink-0"
                  />
                  {getAgentName(option.executor)}
                </span>
              </DropdownMenuItem>
            ))}
          </ToolbarDropdown>

          {selectedExecutor && selectedExecutorConfig ? (
            <ModelSelectorContainer
              agent={selectedExecutor}
              workspaceId={undefined}
              onAdvancedSettings={openAgentCenter}
              presets={variantOptions}
              selectedPreset={selectedVariant}
              onPresetSelect={setVariant}
              onOverrideChange={setOverrides}
              executorConfig={selectedExecutorConfig}
              presetOptions={presetOptions}
              readOnly={readOnly}
            />
          ) : null}
        </div>
      </div>
      <p className="text-xs leading-relaxed text-low">
        {t('workflow.inspector.agentInheritance', {
          defaultValue:
            'Model and runtime settings inherit from Agent Center until you choose an override here.',
        })}
      </p>
    </div>
  );
}
