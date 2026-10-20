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
import {
  PriorityIcon,
  type PriorityLevel,
} from '../../../../packages/ui/src/components/PriorityIcon';
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
  const [priorityClicks, setPriorityClicks] = useState(0);
  const [submitCount, setSubmitCount] = useState(0);
  const [outerEscapes, setOuterEscapes] = useState(0);
  const [, setHostVersion] = useState(0);
  const [workspaceId, setWorkspaceId] = useState<string | undefined>(
    params.has('scopeControls') ? 'fixture-workspace-a' : undefined
  );
  const [sessionId, setSessionId] = useState<string | undefined>(
    params.has('scopeControls') ? 'fixture-session-a' : undefined
  );
  const [inputModalities, setInputModalities] = useState<
    string[] | undefined
  >();
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
      <section
        aria-label="Session agent selector"
        onKeyDown={(event) => {
          if (event.key === 'Escape') setOuterEscapes((count) => count + 1);
        }}
      >
        {options.map((option) => (
          <button
            key={option.executor}
            disabled={!option.enabled}
            onClick={() =>
              setExecutorConfig({
                executor: option.executor,
                variant: null,
                ...(option.executor === BaseCodingAgent.CODEX &&
                params.has('codexExplicitEffort')
                  ? { model_id: 'gpt-6.1-sol', reasoning_id: 'xhigh' }
                  : {}),
              })
            }
          >
            {option.executor}
          </button>
        ))}
        {params.has('scopeControls') && (
          <>
            <button
              data-fixture-scope="host"
              onClick={() => {
                const url = new URL(window.location.href);
                url.searchParams.set('host', 'remote-fixture');
                window.history.replaceState(null, '', url);
                setHostVersion((count) => count + 1);
              }}
            >
              Switch host
            </button>
            <button
              data-fixture-scope="profile"
              onClick={() =>
                setExecutorConfig((previous) => ({
                  ...previous,
                  variant: 'ALT',
                }))
              }
            >
              Switch profile
            </button>
            <button
              data-fixture-scope="workspace"
              onClick={() => setWorkspaceId('fixture-workspace-b')}
            >
              Switch workspace
            </button>
            <button
              data-fixture-scope="session"
              onClick={() => setSessionId('fixture-session-b')}
            >
              Switch session
            </button>
          </>
        )}
        <form
          onSubmit={(event) => {
            event.preventDefault();
            setSubmitCount((count) => count + 1);
          }}
        >
          <ModelSelectorContainer
            key={executorConfig.executor}
            agent={executorConfig.executor}
            workspaceId={workspaceId}
            sessionId={sessionId}
            onAdvancedSettings={() => undefined}
            presets={
              params.has('scopeControls') ? ['DEFAULT', 'ALT'] : ['DEFAULT']
            }
            selectedPreset={executorConfig.variant ?? null}
            onPresetSelect={(variant) =>
              setExecutorConfig((previous) => ({ ...previous, variant }))
            }
            onOverrideChange={(change) => {
              setOverrides((previous) => [...previous, change]);
              setExecutorConfig((previous) => ({ ...previous, ...change }));
            }}
            executorConfig={executorConfig}
            readOnly={params.has('readonly')}
            onInputModalitiesChange={setInputModalities}
            presetOptions={
              params.has('preset') &&
              executorConfig.executor === BaseCodingAgent.CODEX
                ? {
                    executor: BaseCodingAgent.CODEX,
                    model_id: 'gpt-6.0',
                    reasoning_id: 'high',
                  }
                : params.has('geminiPreset') &&
                    executorConfig.executor === BaseCodingAgent.GEMINI
                  ? {
                      executor: BaseCodingAgent.GEMINI,
                      model_id: 'gemini-preset',
                    }
                  : null
            }
          />
        </form>
        <output data-testid="session-submit-count">{submitCount}</output>
        <output data-testid="outer-escape-count">{outerEscapes}</output>
        <output data-testid="input-modalities">
          {JSON.stringify(inputModalities ?? null)}
        </output>
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
      {params.has('priorities') && (
        <section aria-label="Task priorities">
          {(['urgent', 'high', 'medium', 'low'] as PriorityLevel[]).map(
            (priority) => (
              <button
                key={priority}
                onClick={() => setPriorityClicks((count) => count + 1)}
              >
                <PriorityIcon priority={priority} /> TASK-2
              </button>
            )
          )}
          <PriorityIcon priority={null} />
          <output data-testid="priority-clicks">{priorityClicks}</output>
        </section>
      )}
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
