import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  ArrowClockwiseIcon,
  CopyIcon,
  FolderOpenIcon,
  PencilSimpleIcon,
  PlusIcon,
  SpinnerIcon,
  TrashIcon,
} from '@phosphor-icons/react';
import { Button } from '@vibe/ui/components/Button';
import { ConfirmDialog } from '@vibe/ui/components/ConfirmDialog';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';
import { PrimaryButton } from '@vibe/ui/components/PrimaryButton';
import { Switch } from '@vibe/ui/components/Switch';
import { Textarea } from '@vibe/ui/components/Textarea';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import type {
  AgentToolDefinitionSummary,
  AgentToolInventoryView,
  AgentToolKind,
  AgentToolLocator,
  AgentToolOperationError,
  AgentToolProvider,
  AgentToolScope,
  AgentToolView,
  AgentToolWriteDefinition,
  McpServerWriteDefinition,
} from 'shared/types';
import { ApiError } from '@/shared/lib/api';
import { cn } from '@/shared/lib/utils';
import { AGENT_PROVIDERS } from '@/shared/lib/agentProviders';
import { supportedAgentToolScopes } from '@/shared/lib/agentToolScopes';
import { SettingsCard, SettingsInput } from './SettingsComponents';
import { useSettingsMachineClient } from './SettingsHostContext';

const PROVIDERS = AGENT_PROVIDERS.map((provider) => provider.toolProvider);

const PROVIDER_LABELS = Object.fromEntries(
  AGENT_PROVIDERS.map((provider) => [provider.toolProvider, provider.label])
) as Record<AgentToolProvider, string>;

const DEFAULT_MCP_DEFINITION: McpServerWriteDefinition = {
  transport: 'stdio',
  command: { type: 'replace', data: { value: '' } },
  args: { type: 'replace', data: { value: [] } },
  cwd: { type: 'clear' },
  url: { type: 'clear' },
  env: { type: 'clear' },
  headers: { type: 'clear' },
};

const DEFAULT_SKILL_CONTENT =
  '---\nname: my-skill\ndescription: Describe this Skill\n---\n\n# Instructions\n';

type ToolEditorState = {
  mode: 'add' | 'edit';
  kind: AgentToolKind;
  provider: AgentToolProvider;
  scope: AgentToolScope;
  name: string;
  definitionText: string;
  item: AgentToolView | null;
  validationError: string | null;
};

type CopyState = {
  item: AgentToolView;
  target: AgentToolProvider;
};

function locatorFor(
  item: AgentToolView,
  projectPath: string
): AgentToolLocator {
  return {
    provider: item.provider,
    scope: item.scope,
    kind: item.kind,
    name: item.name,
    installation_id: item.installation_id,
    project_path: item.scope === 'project' ? projectPath : undefined,
  };
}

function operationMessage(error: unknown): string {
  if (error instanceof ApiError) {
    const apiError = error as ApiError<AgentToolOperationError>;
    const detail = apiError.error_data;
    return detail ? `${detail.message} (${detail.code})` : apiError.message;
  }
  return error instanceof Error ? error.message : '';
}

function definitionText(
  kind: AgentToolKind,
  definition?: AgentToolDefinitionSummary
): string {
  if (kind === 'mcp_server') {
    const value: McpServerWriteDefinition =
      definition?.type === 'mcp_server'
        ? {
            transport: definition.data.transport,
            command: { type: 'preserve' },
            args: { type: 'preserve' },
            cwd: { type: 'preserve' },
            url: { type: 'preserve' },
            env: { type: 'preserve' },
            headers: { type: 'preserve' },
          }
        : DEFAULT_MCP_DEFINITION;
    return JSON.stringify(value, null, 2);
  }

  return definition?.type === 'skill' ? '' : DEFAULT_SKILL_CONTENT;
}

function parseDefinition(
  kind: AgentToolKind,
  value: string,
  editing: boolean
): AgentToolWriteDefinition {
  if (kind === 'mcp_server') {
    return {
      type: 'mcp_server',
      data: JSON.parse(value) as McpServerWriteDefinition,
    };
  }

  if (editing && value.length === 0) {
    return { type: 'skill', data: { type: 'preserve' } };
  }
  return {
    type: 'skill',
    data: { type: 'replace_contract', data: { value } },
  };
}

export function AgentToolsSettingsSection({
  provider,
  fixedKind,
  onInventoryChange,
}: {
  provider?: AgentToolProvider;
  fixedKind?: AgentToolKind;
  onInventoryChange?: () => void | Promise<void>;
} = {}) {
  const { t } = useTranslation('common');
  const getOperationMessage = useCallback(
    (operationError: unknown) =>
      operationMessage(operationError) ||
      t('agentCenter.tools.errors.operationFailed'),
    [t]
  );
  const machineClient = useSettingsMachineClient();
  const activeClientRef = useRef(machineClient);
  activeClientRef.current = machineClient;
  const [inventory, setInventory] = useState<AgentToolInventoryView | null>(
    null
  );
  const [inventoryHostKey, setInventoryHostKey] = useState<string | null>(null);
  const [loadedProjectPath, setLoadedProjectPath] = useState<string | null>(
    null
  );
  const [kind, setKind] = useState<AgentToolKind>(fixedKind ?? 'mcp_server');
  const [projectPath, setProjectPath] = useState('');
  const [loading, setLoading] = useState(false);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [editor, setEditor] = useState<ToolEditorState | null>(null);
  const [copyState, setCopyState] = useState<CopyState | null>(null);
  const refreshSequence = useRef(0);
  const operationSequence = useRef(0);

  const currentHostKey = JSON.stringify(
    machineClient?.queryScopeKey ?? ['machine', 'unselected']
  );
  const normalizedProjectPath = projectPath.trim();
  const activeInventory =
    inventoryHostKey === currentHostKey &&
    loadedProjectPath === normalizedProjectPath
      ? inventory
      : null;

  const loadInventory = useCallback(
    async (nextProjectPath?: string, clearFeedback = true) => {
      const sequence = ++refreshSequence.current;
      const client = machineClient;
      const hostKey = JSON.stringify(
        client?.queryScopeKey ?? ['machine', 'unselected']
      );
      const requestedPath = nextProjectPath?.trim() ?? '';

      setInventory(null);
      setInventoryHostKey(null);
      setLoadedProjectPath(null);
      setLoading(Boolean(client));
      if (clearFeedback) {
        setError(null);
        setNotice(null);
      }
      if (!client) return;

      try {
        const nextInventory = await client.listAgentTools(
          requestedPath || undefined
        );
        if (
          sequence === refreshSequence.current &&
          activeClientRef.current === client
        ) {
          setInventory(nextInventory);
          setInventoryHostKey(hostKey);
          setLoadedProjectPath(requestedPath);
        }
      } catch (nextError) {
        if (
          sequence === refreshSequence.current &&
          activeClientRef.current === client
        ) {
          setError(getOperationMessage(nextError));
        }
      } finally {
        if (
          sequence === refreshSequence.current &&
          activeClientRef.current === client
        ) {
          setLoading(false);
        }
      }
    },
    [getOperationMessage, machineClient]
  );

  const refresh = useCallback(
    (clearFeedback = true) =>
      loadInventory(normalizedProjectPath || undefined, clearFeedback),
    [loadInventory, normalizedProjectPath]
  );

  useEffect(() => {
    operationSequence.current += 1;
    setBusyKey(null);
    setEditor(null);
    setCopyState(null);
    void loadInventory();
  }, [loadInventory]);

  useEffect(() => {
    if (fixedKind) setKind(fixedKind);
  }, [fixedKind]);

  const allInventories = useMemo(
    () =>
      PROVIDERS.map(
        (providerId) =>
          activeInventory?.providers.find(
            (entry) => entry.provider === providerId
          ) ?? {
            provider: providerId,
            installed: false,
            mcp_scopes: [],
            skill_scopes: [],
            items: [],
            limitations: [],
            errors: [],
          }
      ),
    [activeInventory]
  );
  const inventories = useMemo(
    () =>
      provider
        ? allInventories.filter((entry) => entry.provider === provider)
        : allInventories,
    [allInventories, provider]
  );
  const installedProviders = useMemo(
    () =>
      allInventories
        .filter(
          (providerInventory) =>
            providerInventory.installed &&
            supportedAgentToolScopes(providerInventory, kind).length > 0
        )
        .map((providerInventory) => providerInventory.provider),
    [allInventories, kind]
  );
  const supportedScopesFor = (
    providerId: AgentToolProvider,
    toolKind: AgentToolKind
  ) =>
    supportedAgentToolScopes(
      allInventories.find((entry) => entry.provider === providerId),
      toolKind
    );
  const copyTargetsFor = (item: AgentToolView) =>
    allInventories
      .filter(
        (entry) =>
          entry.installed &&
          entry.provider !== item.provider &&
          supportedAgentToolScopes(entry, item.kind).includes(item.scope)
      )
      .map((entry) => entry.provider);
  const addProviderAvailable = provider
    ? installedProviders.includes(provider)
    : installedProviders.length > 0;

  const run = async (
    key: string,
    operation: () => Promise<unknown>
  ): Promise<boolean> => {
    const client = machineClient;
    if (!client) return false;
    const sequence = ++operationSequence.current;
    setBusyKey(key);
    setError(null);
    setNotice(null);
    try {
      const result = await operation();
      const nextNotice = typeof result === 'string' ? result : undefined;
      if (
        sequence !== operationSequence.current ||
        activeClientRef.current !== client
      ) {
        return false;
      }
      await refresh(false);
      if (
        sequence === operationSequence.current &&
        activeClientRef.current === client
      ) {
        setNotice(nextNotice ?? null);
        try {
          await onInventoryChange?.();
        } catch (summaryError) {
          setError(getOperationMessage(summaryError));
        }
        return true;
      }
      return false;
    } catch (nextError) {
      if (
        sequence === operationSequence.current &&
        activeClientRef.current === client
      ) {
        setError(getOperationMessage(nextError));
      }
      return false;
    } finally {
      if (
        sequence === operationSequence.current &&
        activeClientRef.current === client
      ) {
        setBusyKey(null);
      }
    }
  };

  const openAddDialog = () => {
    const selectedProvider =
      provider === undefined
        ? installedProviders[0]
        : installedProviders.includes(provider)
          ? provider
          : undefined;
    if (!selectedProvider) {
      setError(t('agentCenter.tools.errors.noInstalledProvider'));
      return;
    }
    setEditor({
      mode: 'add',
      kind,
      provider: selectedProvider,
      scope: supportedScopesFor(selectedProvider, kind)[0],
      name: '',
      definitionText: definitionText(kind),
      item: null,
      validationError: null,
    });
  };

  const openEditDialog = (item: AgentToolView) => {
    setEditor({
      mode: 'edit',
      kind: item.kind,
      provider: item.provider,
      scope: item.scope,
      name: item.name,
      definitionText: definitionText(item.kind, item.definition),
      item,
      validationError: null,
    });
  };

  const submitEditor = async () => {
    if (!machineClient || !editor) return;
    if (
      !supportedScopesFor(editor.provider, editor.kind).includes(editor.scope)
    ) {
      setEditor({
        ...editor,
        validationError: t('agentCenter.tools.errors.operationFailed'),
      });
      return;
    }
    const name = editor.name.trim();
    if (!name) {
      setEditor((current) =>
        current
          ? {
              ...current,
              validationError: t('agentCenter.tools.validation.nameRequired'),
            }
          : null
      );
      return;
    }
    if (editor.scope === 'project' && !normalizedProjectPath) {
      setEditor((current) =>
        current
          ? {
              ...current,
              validationError: t(
                'agentCenter.tools.validation.projectPathRequired'
              ),
            }
          : null
      );
      return;
    }

    let definition: AgentToolWriteDefinition;
    try {
      definition = parseDefinition(
        editor.kind,
        editor.definitionText,
        editor.mode === 'edit'
      );
    } catch (nextError) {
      setEditor((current) =>
        current
          ? {
              ...current,
              validationError: t(
                editor.kind === 'mcp_server'
                  ? 'agentCenter.tools.validation.invalidMcpJson'
                  : 'agentCenter.tools.validation.invalidSkill',
                { message: getOperationMessage(nextError) }
              ),
            }
          : null
      );
      return;
    }

    const succeeded = await run(
      `${editor.mode}:${editor.provider}:${name}`,
      () => {
        if (editor.mode === 'edit' && editor.item) {
          return machineClient.updateAgentTool({
            target: locatorFor(editor.item, normalizedProjectPath),
            expected_revision: editor.item.revision,
            definition,
          });
        }
        return machineClient.createAgentTool({
          target: {
            provider: editor.provider,
            scope: editor.scope,
            kind: editor.kind,
            name,
            project_path:
              editor.scope === 'project' ? normalizedProjectPath : undefined,
          },
          definition,
          replace: false,
        });
      }
    );
    if (succeeded) setEditor(null);
  };

  const openCopyDialog = (item: AgentToolView) => {
    const target = copyTargetsFor(item)[0];
    if (!target) {
      setError(t('agentCenter.tools.errors.noCopyTarget'));
      return;
    }
    setCopyState({ item, target });
  };

  const targetItemFor = (state: CopyState | null) => {
    if (!state) return undefined;
    return activeInventory?.providers
      .find((entry) => entry.provider === state.target)
      ?.items.find(
        (candidate) =>
          candidate.scope === state.item.scope &&
          candidate.kind === state.item.kind &&
          candidate.name === state.item.name
      );
  };

  const submitCopy = async () => {
    if (!machineClient || !copyState) return;
    if (!copyTargetsFor(copyState.item).includes(copyState.target)) return;
    const targetItem = targetItemFor(copyState);
    const { item, target } = copyState;
    const succeeded = await run(
      `copy:${item.provider}:${item.name}`,
      async () => {
        const result = await machineClient.copyAgentTool({
          source: locatorFor(item, normalizedProjectPath),
          expected_revision: item.revision,
          target_provider: target,
          target_scope: item.scope,
          target_project_path:
            item.scope === 'project' ? normalizedProjectPath : undefined,
          replace: Boolean(targetItem),
          target_expected_revision: targetItem?.revision,
        });
        return result.warnings.length ? result.warnings.join(' ') : undefined;
      }
    );
    if (succeeded) setCopyState(null);
  };

  const handleReveal = async (item: AgentToolView) => {
    if (!machineClient) return;
    await run(`reveal:${item.provider}:${item.name}`, async () => {
      const result = await machineClient.revealAgentTool(
        locatorFor(item, normalizedProjectPath)
      );
      await navigator.clipboard.writeText(
        formatPathForDisplay(result.native_path)
      );
      return t('agentCenter.tools.notices.pathCopied', {
        path: formatPathForDisplay(result.native_path),
      });
    });
  };

  const handleRemove = async (item: AgentToolView) => {
    if (!machineClient) return;
    const result = await ConfirmDialog.show({
      title: t('agentCenter.tools.remove.title', { name: item.name }),
      message: t('agentCenter.tools.remove.message'),
      confirmText: t('agentCenter.tools.actions.remove'),
      cancelText: t('buttons.cancel'),
      variant: 'destructive',
    });
    if (result !== 'confirmed') return;
    await run(`remove:${item.provider}:${item.name}`, () =>
      machineClient.removeAgentTool({
        target: locatorFor(item, normalizedProjectPath),
        expected_revision: item.revision,
      })
    );
  };

  const targetItem = targetItemFor(copyState);
  const editorBusy = Boolean(editor && busyKey?.startsWith(`${editor.mode}:`));
  const copyBusy = Boolean(copyState && busyKey?.startsWith('copy:'));

  return (
    <>
      <SettingsCard
        title={t(
          fixedKind === 'mcp_server'
            ? 'agentCenter.tools.titles.mcp'
            : fixedKind === 'skill'
              ? 'agentCenter.tools.titles.skills'
              : 'agentCenter.tools.titles.all'
        )}
        description={t(
          fixedKind === 'mcp_server'
            ? 'agentCenter.tools.descriptions.mcp'
            : fixedKind === 'skill'
              ? 'agentCenter.tools.descriptions.skills'
              : 'agentCenter.tools.descriptions.all'
        )}
        headerAction={
          <div className="flex gap-half">
            <PrimaryButton
              variant="tertiary"
              value={t('agentCenter.tools.actions.refresh')}
              onClick={() => void refresh()}
              disabled={loading || busyKey !== null || !machineClient}
              actionIcon={loading ? 'spinner' : undefined}
            />
            <PrimaryButton
              value={t('agentCenter.tools.actions.add')}
              onClick={openAddDialog}
              disabled={
                !machineClient ||
                busyKey !== null ||
                !addProviderAvailable ||
                activeInventory === null
              }
            />
          </div>
        }
      >
        <div className="space-y-2">
          <label
            htmlFor="agent-tools-project-path"
            className="text-sm font-medium text-normal"
          >
            {t('agentCenter.tools.projectPath.label')}
          </label>
          <div className="flex gap-2">
            <SettingsInput
              id="agent-tools-project-path"
              value={formatPathForDisplay(projectPath)}
              onChange={setProjectPath}
              placeholder={t('agentCenter.tools.projectPath.placeholder')}
            />
            <button
              type="button"
              onClick={() => void refresh()}
              disabled={loading || busyKey !== null || !machineClient}
              className="min-h-11 min-w-11 rounded-sm border border-border px-3 text-low hover:text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:cursor-not-allowed disabled:opacity-40"
              title={t('agentCenter.tools.projectPath.discover')}
              aria-label={t('agentCenter.tools.projectPath.discover')}
            >
              <ArrowClockwiseIcon className="size-icon-xs" aria-hidden="true" />
            </button>
          </div>
          {loadedProjectPath !== normalizedProjectPath && !loading && (
            <p className="text-xs text-low" role="status">
              {t('agentCenter.tools.projectPath.refreshRequired')}
            </p>
          )}
        </div>

        {!fixedKind && (
          <div className="flex border-b border-border" role="tablist">
            {(['mcp_server', 'skill'] as AgentToolKind[]).map((tab) => (
              <button
                key={tab}
                type="button"
                role="tab"
                aria-selected={kind === tab}
                aria-controls="agent-tools-provider-list"
                onClick={() => setKind(tab)}
                className={cn(
                  'min-h-11 -mb-px border-b-2 px-4 py-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand',
                  kind === tab
                    ? 'border-brand text-brand'
                    : 'border-transparent text-low hover:text-normal'
                )}
              >
                {t(
                  tab === 'mcp_server'
                    ? 'agentCenter.tools.kinds.mcp'
                    : 'agentCenter.tools.kinds.skill'
                )}
              </button>
            ))}
          </div>
        )}

        {error && (
          <div
            className="rounded-sm border border-error/50 bg-error/10 p-3 text-sm text-error"
            role="alert"
          >
            {error}
          </div>
        )}
        {notice && (
          <div
            className="rounded-sm border border-success/50 bg-success/10 p-3 text-sm text-success"
            role="status"
            aria-live="polite"
          >
            {notice}
          </div>
        )}

        {activeInventory === null ? (
          <div
            className="flex items-center gap-2 py-4 text-sm text-low"
            role="status"
          >
            {loading && (
              <SpinnerIcon
                className="size-icon-xs animate-spin"
                aria-hidden="true"
              />
            )}
            {loading
              ? t('agentCenter.tools.states.loading')
              : t('agentCenter.tools.states.unavailable')}
          </div>
        ) : (
          <div
            id="agent-tools-provider-list"
            className="space-y-4"
            role="tabpanel"
          >
            {activeInventory.errors.map((inventoryError) => (
              <div
                key={`${inventoryError.provider}:${inventoryError.message}`}
                className="rounded-sm border border-error/50 bg-error/10 p-3 text-sm text-error"
                role="alert"
              >
                {PROVIDER_LABELS[inventoryError.provider]}:{' '}
                {inventoryError.message}
              </div>
            ))}
            {inventories.map((providerInventory) => {
              const items = providerInventory.items.filter(
                (item) => item.kind === kind
              );
              return (
                <section
                  key={providerInventory.provider}
                  className="rounded-sm border border-border bg-secondary/20"
                >
                  <div className="flex items-center justify-between border-b border-border px-3 py-2">
                    <div>
                      <span className="text-sm font-medium text-high">
                        {PROVIDER_LABELS[providerInventory.provider]}
                      </span>
                      <span className="ml-2 text-xs text-low">
                        {t(
                          providerInventory.installed
                            ? 'agentCenter.tools.states.installed'
                            : 'agentCenter.tools.states.notDetected'
                        )}
                      </span>
                    </div>
                    <span className="text-xs text-low">
                      {t('agentCenter.tools.itemCount', {
                        count: items.length,
                      })}
                    </span>
                  </div>

                  {providerInventory.errors.map((providerError) => (
                    <div
                      key={providerError}
                      className="border-b border-error/30 bg-error/5 px-3 py-2 text-xs text-error"
                      role="alert"
                    >
                      {providerError}
                    </div>
                  ))}

                  {providerInventory.limitations.map((limitation) => (
                    <div
                      key={limitation}
                      className="border-b border-border/60 px-3 py-2 text-xs text-low"
                    >
                      {limitation}
                    </div>
                  ))}

                  {items.length === 0 ? (
                    <div className="px-3 py-4 text-sm text-low">
                      {t(
                        kind === 'mcp_server'
                          ? 'agentCenter.tools.empty.mcp'
                          : 'agentCenter.tools.empty.skills'
                      )}
                    </div>
                  ) : (
                    items.map((item) => {
                      const busy = busyKey?.endsWith(
                        `:${item.provider}:${item.name}`
                      );
                      const hasCopyTarget = copyTargetsFor(item).length > 0;
                      const editDisabledReason = busyKey
                        ? t('agentCenter.tools.disabled.operationPending')
                        : !item.capabilities.editable
                          ? t('agentCenter.tools.disabled.notEditable')
                          : undefined;
                      const copyDisabledReason = busyKey
                        ? t('agentCenter.tools.disabled.operationPending')
                        : item.state !== 'enabled'
                          ? t('agentCenter.tools.disabled.copyRequiresEnabled')
                          : !item.capabilities.exportable
                            ? t('agentCenter.tools.disabled.notExportable')
                            : !hasCopyTarget
                              ? t('agentCenter.tools.disabled.noCopyTarget')
                              : undefined;
                      const removeDisabledReason = busyKey
                        ? t('agentCenter.tools.disabled.operationPending')
                        : !item.capabilities.removable
                          ? t('agentCenter.tools.disabled.notRemovable')
                          : undefined;
                      return (
                        <div
                          key={item.installation_id}
                          className="flex flex-col gap-2 border-b border-border/60 px-3 py-3 last:border-0"
                        >
                          <div className="flex items-start justify-between gap-3">
                            <div className="min-w-0">
                              <div className="flex items-center gap-2">
                                <span className="truncate text-sm font-medium text-normal">
                                  {item.name}
                                </span>
                                <span
                                  className={cn(
                                    'rounded px-1.5 py-0.5 text-xs',
                                    item.state === 'enabled'
                                      ? 'bg-success/10 text-success'
                                      : item.state === 'error'
                                        ? 'bg-error/10 text-error'
                                        : 'bg-secondary text-low'
                                  )}
                                >
                                  {t(
                                    `agentCenter.tools.toolStates.${item.state}`
                                  )}
                                </span>
                                <span className="rounded bg-secondary px-1.5 py-0.5 text-xs text-low">
                                  {t(`agentCenter.tools.scopes.${item.scope}`)}
                                </span>
                              </div>
                              {item.definition.type === 'mcp_server' && (
                                <p className="mt-1 truncate font-mono text-xs text-low">
                                  {item.definition.data.transport === 'stdio'
                                    ? (item.definition.data.command_display ??
                                      t(
                                        'agentCenter.tools.security.configured'
                                      ))
                                    : (item.definition.data.url_display ??
                                      t(
                                        'agentCenter.tools.security.configured'
                                      ))}
                                </p>
                              )}
                              {item.error && (
                                <p className="mt-1 text-xs text-error">
                                  {item.error}
                                </p>
                              )}
                            </div>
                            {busy ? (
                              <span
                                role="status"
                                aria-label={t(
                                  'agentCenter.tools.states.updating',
                                  { name: item.name }
                                )}
                              >
                                <SpinnerIcon
                                  className="size-icon-xs animate-spin text-low"
                                  aria-hidden="true"
                                />
                              </span>
                            ) : (
                              item.capabilities.toggleable &&
                              ['enabled', 'disabled'].includes(item.state) && (
                                <Switch
                                  checked={item.state === 'enabled'}
                                  disabled={busyKey !== null}
                                  aria-label={t(
                                    'agentCenter.tools.actions.toggle',
                                    { name: item.name }
                                  )}
                                  onCheckedChange={(enabled) =>
                                    void run(
                                      `toggle:${item.provider}:${item.name}`,
                                      () =>
                                        machineClient!.toggleAgentTool({
                                          target: locatorFor(
                                            item,
                                            normalizedProjectPath
                                          ),
                                          expected_revision: item.revision,
                                          enabled,
                                        })
                                    )
                                  }
                                />
                              )
                            )}
                          </div>
                          <div className="flex flex-wrap gap-1">
                            <ToolButton
                              label={t('agentCenter.tools.actions.edit')}
                              icon={PencilSimpleIcon}
                              disabledReason={editDisabledReason}
                              onClick={() => openEditDialog(item)}
                            />
                            <ToolButton
                              label={t('agentCenter.tools.actions.copy')}
                              icon={CopyIcon}
                              disabledReason={copyDisabledReason}
                              onClick={() => openCopyDialog(item)}
                            />
                            <ToolButton
                              label={t('agentCenter.tools.actions.reveal')}
                              icon={FolderOpenIcon}
                              disabledReason={
                                busyKey
                                  ? t(
                                      'agentCenter.tools.disabled.operationPending'
                                    )
                                  : undefined
                              }
                              onClick={() => void handleReveal(item)}
                            />
                            <ToolButton
                              label={t('agentCenter.tools.actions.remove')}
                              icon={TrashIcon}
                              danger
                              disabledReason={removeDisabledReason}
                              onClick={() => void handleRemove(item)}
                            />
                          </div>
                        </div>
                      );
                    })
                  )}
                </section>
              );
            })}
          </div>
        )}
      </SettingsCard>

      <Dialog
        open={editor !== null}
        onOpenChange={(open) => {
          if (!open && !editorBusy) setEditor(null);
        }}
      >
        <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto sm:max-w-[680px]">
          {editor && (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                void submitEditor();
              }}
            >
              <DialogHeader>
                <DialogTitle>
                  {t(
                    editor.mode === 'add'
                      ? 'agentCenter.tools.editor.addTitle'
                      : 'agentCenter.tools.editor.editTitle',
                    { name: editor.name }
                  )}
                </DialogTitle>
                <DialogDescription className="text-left">
                  {t('agentCenter.tools.editor.description')}
                </DialogDescription>
              </DialogHeader>
              <div className="space-y-4 py-4">
                <label className="block space-y-2 text-sm font-medium text-normal">
                  <span>{t('agentCenter.tools.editor.provider')}</span>
                  <select
                    value={editor.provider}
                    disabled={editor.mode === 'edit' || provider !== undefined}
                    onChange={(event) => {
                      const nextProvider = event.target
                        .value as AgentToolProvider;
                      const scopes = supportedScopesFor(
                        nextProvider,
                        editor.kind
                      );
                      setEditor({
                        ...editor,
                        provider: nextProvider,
                        scope: scopes.includes(editor.scope)
                          ? editor.scope
                          : scopes[0],
                        validationError: null,
                      });
                    }}
                    className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:opacity-60"
                  >
                    {installedProviders.map((providerId) => (
                      <option key={providerId} value={providerId}>
                        {PROVIDER_LABELS[providerId]}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="block space-y-2 text-sm font-medium text-normal">
                  <span>{t('agentCenter.tools.editor.scope')}</span>
                  <select
                    value={editor.scope}
                    disabled={editor.mode === 'edit'}
                    onChange={(event) =>
                      setEditor({
                        ...editor,
                        scope: event.target.value as AgentToolScope,
                        validationError: null,
                      })
                    }
                    className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:opacity-60"
                  >
                    {(editor.mode === 'edit'
                      ? [editor.scope]
                      : supportedScopesFor(editor.provider, editor.kind)
                    ).map((scope) => (
                      <option key={scope} value={scope}>
                        {t(`agentCenter.tools.scopes.${scope}`)}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="block space-y-2 text-sm font-medium text-normal">
                  <span>{t('agentCenter.tools.editor.name')}</span>
                  <input
                    value={editor.name}
                    disabled={editor.mode === 'edit'}
                    onChange={(event) =>
                      setEditor({
                        ...editor,
                        name: event.target.value,
                        validationError: null,
                      })
                    }
                    className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:opacity-60"
                    autoFocus={editor.mode === 'add'}
                  />
                </label>
                <label className="block space-y-2 text-sm font-medium text-normal">
                  <span>
                    {t(
                      editor.kind === 'mcp_server'
                        ? 'agentCenter.tools.editor.mcpDefinition'
                        : 'agentCenter.tools.editor.skillDefinition'
                    )}
                  </span>
                  <Textarea
                    value={editor.definitionText}
                    onChange={(event) =>
                      setEditor({
                        ...editor,
                        definitionText: event.target.value,
                        validationError: null,
                      })
                    }
                    rows={editor.kind === 'mcp_server' ? 14 : 12}
                    className="font-ibm-plex-mono text-xs"
                  />
                  {editor.mode === 'edit' && (
                    <span className="block text-xs font-normal text-low">
                      {t(
                        editor.kind === 'mcp_server'
                          ? 'agentCenter.tools.security.writeOnlyMcpHelp'
                          : 'agentCenter.tools.security.writeOnlySkillHelp'
                      )}
                    </span>
                  )}
                </label>
                {editor.scope === 'project' && (
                  <p className="text-xs text-low">
                    {t('agentCenter.tools.editor.projectTarget', {
                      path:
                        formatPathForDisplay(normalizedProjectPath) ||
                        t('agentCenter.tools.editor.noProjectPath'),
                    })}
                  </p>
                )}
                {editor.validationError && (
                  <p className="text-sm text-error" role="alert">
                    {editor.validationError}
                  </p>
                )}
              </div>
              <DialogFooter>
                <Button
                  type="button"
                  variant="outline"
                  disabled={editorBusy}
                  onClick={() => setEditor(null)}
                >
                  {t('buttons.cancel')}
                </Button>
                <Button type="submit" disabled={editorBusy}>
                  {editorBusy && (
                    <SpinnerIcon
                      className="mr-2 size-icon-xs animate-spin"
                      aria-hidden="true"
                    />
                  )}
                  {t(
                    editor.mode === 'add'
                      ? 'agentCenter.tools.actions.add'
                      : 'agentCenter.tools.actions.save'
                  )}
                </Button>
              </DialogFooter>
            </form>
          )}
        </DialogContent>
      </Dialog>

      <Dialog
        open={copyState !== null}
        onOpenChange={(open) => {
          if (!open && !copyBusy) setCopyState(null);
        }}
      >
        <DialogContent className="sm:max-w-[520px]">
          {copyState && (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                void submitCopy();
              }}
            >
              <DialogHeader>
                <DialogTitle>
                  {t('agentCenter.tools.copy.title', {
                    name: copyState.item.name,
                  })}
                </DialogTitle>
                <DialogDescription className="text-left">
                  {t('agentCenter.tools.copy.description', {
                    provider: PROVIDER_LABELS[copyState.item.provider],
                  })}
                </DialogDescription>
              </DialogHeader>
              <div className="space-y-4 py-4">
                <label className="block space-y-2 text-sm font-medium text-normal">
                  <span>{t('agentCenter.tools.copy.target')}</span>
                  <select
                    value={copyState.target}
                    onChange={(event) =>
                      setCopyState({
                        ...copyState,
                        target: event.target.value as AgentToolProvider,
                      })
                    }
                    className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
                  >
                    {copyTargetsFor(copyState.item).map((providerId) => (
                      <option key={providerId} value={providerId}>
                        {PROVIDER_LABELS[providerId]}
                      </option>
                    ))}
                  </select>
                </label>
                <div className="rounded-sm border border-border bg-secondary/30 p-3 text-sm text-low">
                  {t('agentCenter.tools.copy.adapterNotice')}
                </div>
                {targetItem && (
                  <div
                    className="rounded-sm border border-error/40 bg-error/5 p-3 text-sm text-normal"
                    role="alert"
                  >
                    {t('agentCenter.tools.copy.replaceWarning', {
                      provider: PROVIDER_LABELS[copyState.target],
                      name: copyState.item.name,
                    })}
                  </div>
                )}
              </div>
              <DialogFooter>
                <Button
                  type="button"
                  variant="outline"
                  disabled={copyBusy}
                  onClick={() => setCopyState(null)}
                >
                  {t('buttons.cancel')}
                </Button>
                <Button
                  type="submit"
                  variant={targetItem ? 'destructive' : 'default'}
                  disabled={copyBusy}
                >
                  {copyBusy && (
                    <SpinnerIcon
                      className="mr-2 size-icon-xs animate-spin"
                      aria-hidden="true"
                    />
                  )}
                  {t(
                    targetItem
                      ? 'agentCenter.tools.actions.replace'
                      : 'agentCenter.tools.actions.copy'
                  )}
                </Button>
              </DialogFooter>
            </form>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}

function ToolButton({
  label,
  icon: Icon,
  onClick,
  disabledReason,
  danger,
}: {
  label: string;
  icon: typeof PlusIcon;
  onClick: () => void;
  disabledReason?: string;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={Boolean(disabledReason)}
      title={disabledReason}
      aria-label={disabledReason ? `${label}: ${disabledReason}` : label}
      className={cn(
        'inline-flex min-h-11 items-center gap-1 rounded-sm px-2 py-1 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand',
        danger
          ? 'text-error hover:bg-error/10'
          : 'text-low hover:bg-secondary hover:text-normal',
        disabledReason && 'cursor-not-allowed opacity-40'
      )}
    >
      <Icon className="size-icon-2xs" aria-hidden="true" />
      {label}
    </button>
  );
}
