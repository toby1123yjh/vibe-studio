import { useCallback, useState, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { HotkeysProvider } from 'react-hotkeys-hook';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import {
  AgentProviderCapability,
  BaseCodingAgent,
  PermissionPolicy,
  type ExecutorConfig,
} from 'shared/types';
import { AgentCenterPage } from '../../../../packages/web-core/src/features/agent-center/ui/AgentCenterPage';
import { ModelSelectorContainer } from '../../../../packages/web-core/src/shared/components/ModelSelectorContainer';
import { CreateChatBox } from '../../../../packages/ui/src/components/CreateChatBox';
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

if (new URLSearchParams(window.location.search).has('composerPreview')) {
  await import('../../../../packages/web-core/src/app/styles/new/index.css');
  document.documentElement.classList.add(
    new URLSearchParams(window.location.search).get('theme') === 'dark'
      ? 'dark'
      : 'light'
  );
}

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

function ComposerPreview({
  executor,
  onExecutorChange,
  children,
}: {
  executor: BaseCodingAgent;
  onExecutorChange: (executor: BaseCodingAgent) => void;
  children: ReactNode;
}) {
  const [text, setText] = useState('');
  if (!new URLSearchParams(window.location.search).has('composerPreview')) {
    return children;
  }
  return (
    <div
      data-testid="composer-preview"
      className="new-design font-ibm-plex-sans"
    >
      <CreateChatBox
        editor={{ value: text, onChange: setText }}
        renderEditor={({ value, onChange }) => (
          <textarea
            aria-label="Message"
            placeholder="Describe what you want to do…"
            value={value}
            onChange={(event) => onChange(event.target.value)}
            className="min-h-28 w-full resize-none bg-transparent text-base text-normal placeholder:text-low focus:outline-none"
          />
        )}
        executor={{
          selected: executor,
          options: [BaseCodingAgent.CODEX, BaseCodingAgent.CLAUDE_CODE],
          onChange: onExecutorChange,
        }}
        modelSelector={children}
        onSend={() => undefined}
        isSending={false}
        projectSelector={
          <label className="inline-flex min-w-0 max-w-[240px] items-center gap-half text-sm text-low">
            <span className="shrink-0">Project</span>
            <select
              aria-label="Project"
              defaultValue="test"
              disabled
              className="min-h-8 min-w-0 max-w-full appearance-none rounded-sm bg-transparent px-half font-medium text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:cursor-default disabled:opacity-70"
            >
              <option value="test">test</option>
            </select>
          </label>
        }
        linkedTask={{ simpleId: 'TASK-2', title: 'Review project delivery' }}
      />
    </div>
  );
}

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
  const handleInputModalitiesChange = useCallback(
    (modalities: string[] | undefined) => {
      setInputModalities((previous) =>
        JSON.stringify(previous) === JSON.stringify(modalities)
          ? previous
          : modalities
      );
    },
    []
  );
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
                ...(option.executor === BaseCodingAgent.CODEX &&
                params.has('codexExplicitMax')
                  ? { model_id: 'gpt-6.1-sol', reasoning_id: 'max' }
                  : {}),
                ...(option.executor === BaseCodingAgent.CODEX &&
                params.has('codexPreserveSettings')
                  ? {
                      provider_options: {
                        provider: 'codex',
                        service_tier: 'fast',
                        plan_mode_reasoning_effort: 'high',
                      },
                    }
                  : {}),
                ...(option.executor === BaseCodingAgent.CODEX &&
                (params.has('codexPlan') || params.has('codexPlanUnknown'))
                  ? {
                      reasoning_id: 'medium',
                      ...(params.has('codexPlanUnknown')
                        ? { permission_policy: PermissionPolicy.PLAN }
                        : {}),
                      provider_options: {
                        provider: 'codex',
                        service_tier: 'priority',
                      },
                    }
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
          <ComposerPreview
            executor={executorConfig.executor}
            onExecutorChange={(executor) =>
              setExecutorConfig({ executor, variant: null })
            }
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
              onInputModalitiesChange={handleInputModalitiesChange}
              presetOptions={
                params.has('codexPlan') &&
                executorConfig.executor === BaseCodingAgent.CODEX
                  ? {
                      executor: BaseCodingAgent.CODEX,
                      permission_policy: PermissionPolicy.PLAN,
                      reasoning_id: 'xhigh',
                      provider_options: {
                        provider: 'codex',
                        plan_mode_reasoning_effort: 'high',
                      },
                    }
                  : params.has('preset') &&
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
          </ComposerPreview>
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
