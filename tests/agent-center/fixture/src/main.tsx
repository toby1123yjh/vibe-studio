import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { HotkeysProvider } from 'react-hotkeys-hook';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import {
  AgentProviderCapability,
  BaseCodingAgent,
  type ExecutorConfig,
} from 'shared/types';
import { AgentCenterPage } from '../../../../packages/web-core/src/features/agent-center/ui/AgentCenterPage';
import { ModelSelectorContainer } from '../../../../packages/web-core/src/shared/components/ModelSelectorContainer';
import { useAgentProviderOptions } from '../../../../packages/web-core/src/shared/hooks/useAgentProviderPolicy';
import { WorkflowAgentExecutorField } from '../../../../packages/web-core/src/features/workflow/ui/WorkflowAgentExecutorField';
import { SettingsDirtyProvider } from '../../../../packages/web-core/src/shared/dialogs/settings/settings/SettingsDirtyContext';
import {
  SettingsHostProvider,
  useSettingsHost,
} from '../../../../packages/web-core/src/shared/dialogs/settings/settings/SettingsHostContext';
import '../../../../packages/web-core/src/i18n/config';
import '../../../../packages/ui/src/styles/tokens.css';
import './style.css';

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false } },
});

const root = document.getElementById('root');
if (!root) throw new Error('Agent Center fixture root is missing');

function HostDiscoveryProbe() {
  const { hostDiscovery, remoteHostDiscovery } = useSettingsHost();
  return (
    <output
      hidden
      data-testid="host-discovery-probe"
      data-canonical={hostDiscovery.hasCanonicalData}
      data-host-error={hostDiscovery.error != null}
      data-remote-error={remoteHostDiscovery.error != null}
    />
  );
}

const SESSION_CAPABILITIES = [AgentProviderCapability.INITIAL_RUN] as const;

function SelectorsFixture() {
  const params = new URLSearchParams(window.location.search);
  const [overrides, setOverrides] = useState<Partial<ExecutorConfig>[]>([]);
  const [executorConfig, setExecutorConfig] = useState<ExecutorConfig>({
    executor: BaseCodingAgent.OPENCODE,
    variant: null,
  });
  const [workflowConfig, setWorkflowConfig] = useState<
    ExecutorConfig | undefined
  >();
  const { options } = useAgentProviderOptions({
    requiredCapabilities: SESSION_CAPABILITIES,
  });

  return (
    <>
      <section aria-label="Session agent selector">
        {options.map((option) => (
          <button
            key={option.executor}
            disabled={!option.enabled}
            onClick={() =>
              setExecutorConfig({ executor: option.executor, variant: null })
            }
          >
            {option.executor}
          </button>
        ))}
        <ModelSelectorContainer
          key={executorConfig.executor}
          agent={executorConfig.executor}
          workspaceId={undefined}
          onAdvancedSettings={() => undefined}
          presets={['DEFAULT']}
          selectedPreset={null}
          onPresetSelect={() => undefined}
          onOverrideChange={(change) => {
            setOverrides((previous) => [...previous, change]);
            setExecutorConfig((previous) => ({ ...previous, ...change }));
          }}
          executorConfig={executorConfig}
          presetOptions={
            params.has('preset') &&
            executorConfig.executor === BaseCodingAgent.CODEX
              ? {
                  executor: BaseCodingAgent.CODEX,
                  model_id: 'gpt-6.0',
                  reasoning_id: 'high',
                }
              : null
          }
        />
        <output data-testid="session-selection">
          {JSON.stringify(executorConfig)}
        </output>
        <output data-testid="selector-overrides">
          {JSON.stringify(overrides)}
        </output>
      </section>
      <section aria-label="Workflow agent selector">
        <WorkflowAgentExecutorField
          value={workflowConfig}
          onChange={setWorkflowConfig}
        />
        <output data-testid="workflow-selection">
          {JSON.stringify(workflowConfig)}
        </output>
      </section>
    </>
  );
}

createRoot(root).render(
  <HotkeysProvider initiallyActiveScopes={['kanban', 'projects']}>
    <QueryClientProvider client={queryClient}>
      <main className="fixture-shell">
        <SettingsHostProvider>
          <SettingsDirtyProvider>
            <HostDiscoveryProbe />
            {new URLSearchParams(window.location.search).has('selectors') ? (
              <SelectorsFixture />
            ) : (
              <AgentCenterPage />
            )}
          </SettingsDirtyProvider>
        </SettingsHostProvider>
      </main>
    </QueryClientProvider>
  </HotkeysProvider>
);
