import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  ArrowClockwiseIcon,
  MagnifyingGlassIcon,
  SpinnerIcon,
} from '@phosphor-icons/react';
import { ConfirmDialog } from '@vibe/ui/components/ConfirmDialog';
import { PrimaryButton } from '@vibe/ui/components/PrimaryButton';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import type {
  AgentCommandInventoryView,
  AgentCommandOperationError,
  AgentCommandProvider,
  AgentCommandView,
  AgentCommandWriteDefinition,
} from 'shared/types';
import {
  SettingsCard,
  SettingsInput,
} from '@/shared/dialogs/settings/settings/SettingsComponents';
import { useSettingsDirty } from '@/shared/dialogs/settings/settings/SettingsDirtyContext';
import { useSettingsMachineClient } from '@/shared/dialogs/settings/settings/SettingsHostContext';
import { ApiError } from '@/shared/lib/api';
import { AgentCommandEditorDialog } from './AgentCommandEditorDialog';
import { AgentCommandInventory } from './AgentCommandInventory';
import {
  type CommandEditorState,
  commandNameIsValid,
  definitionDescription,
  editorFromItem,
  locatorFor,
  writeDefinition,
} from './agent-command-model';

const DIRTY_SECTION_ID = 'agent-center-command-editor';

type OperationFailure = {
  message: string;
  revisionConflict: boolean;
};

export function AgentCommandsSettingsSection({
  provider,
  onInventoryChange,
}: {
  provider: AgentCommandProvider;
  onInventoryChange?: () => void | Promise<void>;
}) {
  const { t } = useTranslation('common');
  const machineClient = useSettingsMachineClient();
  const { setDirty } = useSettingsDirty();
  const activeClientRef = useRef(machineClient);
  activeClientRef.current = machineClient;
  const [inventory, setInventory] = useState<AgentCommandInventoryView | null>(
    null
  );
  const [inventoryHostKey, setInventoryHostKey] = useState<string | null>(null);
  const [loadedProjectPath, setLoadedProjectPath] = useState<string | null>(
    null
  );
  const [projectPath, setProjectPath] = useState('');
  const [search, setSearch] = useState('');
  const [loading, setLoading] = useState(false);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [error, setError] = useState<OperationFailure | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [editor, setEditor] = useState<CommandEditorState | null>(null);
  const refreshSequence = useRef(0);
  const operationSequence = useRef(0);

  const currentHostKey = JSON.stringify(
    machineClient?.queryScopeKey ?? ['machine', 'unselected']
  );
  const normalizedProjectPath = projectPath.trim();
  const currentHostInventory =
    inventoryHostKey === currentHostKey ? inventory : null;
  const activeInventory =
    currentHostInventory && loadedProjectPath === normalizedProjectPath
      ? currentHostInventory
      : null;
  const providerInventory = activeInventory?.providers.find(
    (entry) => entry.provider === provider
  );
  const providerDescriptor = currentHostInventory?.providers.find(
    (entry) => entry.provider === provider
  );

  useEffect(() => {
    setDirty(DIRTY_SECTION_ID, Boolean(editor?.dirty));
    return () => setDirty(DIRTY_SECTION_ID, false);
  }, [editor?.dirty, setDirty]);

  const operationFailure = useCallback(
    (operationError: unknown): OperationFailure => {
      if (operationError instanceof ApiError) {
        const apiError = operationError as ApiError<AgentCommandOperationError>;
        const detail = apiError.error_data;
        if (detail?.code === 'stale_revision') {
          return {
            message: t('agentCenter.commands.errors.staleRevision'),
            revisionConflict: true,
          };
        }
        if (detail) {
          return {
            message: t('agentCenter.commands.errors.operationFailed'),
            revisionConflict: false,
          };
        }
        return {
          message: t('agentCenter.commands.errors.operationFailed'),
          revisionConflict: false,
        };
      }
      return {
        message: t('agentCenter.commands.errors.operationFailed'),
        revisionConflict: false,
      };
    },
    [t]
  );

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
      if (!client) return false;

      try {
        const nextInventory = await client.listAgentCommands(
          requestedPath || undefined
        );
        if (
          sequence === refreshSequence.current &&
          activeClientRef.current === client
        ) {
          setInventory(nextInventory);
          setInventoryHostKey(hostKey);
          setLoadedProjectPath(requestedPath);
          return true;
        }
        return false;
      } catch (nextError) {
        if (
          sequence === refreshSequence.current &&
          activeClientRef.current === client
        ) {
          setError(operationFailure(nextError));
        }
        return false;
      } finally {
        if (
          sequence === refreshSequence.current &&
          activeClientRef.current === client
        ) {
          setLoading(false);
        }
      }
    },
    [machineClient, operationFailure]
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
    void loadInventory();
  }, [loadInventory]);

  useEffect(() => {
    setEditor(null);
    setSearch('');
  }, [provider]);

  const run = async (
    key: string,
    operation: () => Promise<unknown>,
    onFailure?: (failure: OperationFailure) => void
  ): Promise<boolean> => {
    const client = machineClient;
    if (!client) return false;
    const sequence = ++operationSequence.current;
    setBusyKey(key);
    setError(null);
    setNotice(null);
    try {
      await operation();
      if (
        sequence !== operationSequence.current ||
        activeClientRef.current !== client
      ) {
        return false;
      }
      const refreshed = await refresh(false);
      if (
        sequence === operationSequence.current &&
        activeClientRef.current === client
      ) {
        if (!refreshed) return true;
        setNotice(t('agentCenter.commands.notices.saved'));
        try {
          await onInventoryChange?.();
        } catch (summaryError) {
          setError(operationFailure(summaryError));
        }
        return true;
      }
      return false;
    } catch (nextError) {
      if (
        sequence === operationSequence.current &&
        activeClientRef.current === client
      ) {
        const failure = operationFailure(nextError);
        setError(failure);
        onFailure?.(failure);
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

  const updateEditor = (patch: Partial<CommandEditorState>) => {
    setEditor((current) =>
      current
        ? {
            ...current,
            ...patch,
            dirty: true,
            validationError: null,
            revisionConflict: false,
          }
        : null
    );
  };

  const closeEditor = () => {
    setEditor(null);
    setDirty(DIRTY_SECTION_ID, false);
  };

  const requestCloseEditor = async () => {
    if (!editor?.dirty) {
      closeEditor();
      return;
    }
    const result = await ConfirmDialog.show({
      title: t('agentCenter.commands.unsaved.title'),
      message: t('agentCenter.commands.unsaved.message'),
      confirmText: t('agentCenter.commands.unsaved.discard'),
      cancelText: t('buttons.cancel'),
      variant: 'destructive',
    });
    if (result === 'confirmed') closeEditor();
  };

  const openAddDialog = () => {
    if (
      !providerInventory?.installed ||
      !providerInventory.capabilities.creatable
    ) {
      return;
    }
    const format = providerInventory.capabilities.writable_formats[0];
    const scope = providerInventory.capabilities.supported_scopes[0];
    if (!format || !scope) return;
    setEditor({
      mode: 'add',
      item: null,
      scope,
      format,
      name: '',
      description: '',
      argumentHint: '',
      body: '',
      dirty: false,
      validationError: null,
      revisionConflict: false,
    });
  };

  const submitEditor = async () => {
    if (!machineClient || !editor || !providerInventory) return;
    const name = editor.name.trim();
    if (!name) {
      setEditor({
        ...editor,
        validationError: t('agentCenter.commands.validation.nameRequired'),
      });
      return;
    }
    if (!commandNameIsValid(provider, name)) {
      setEditor({
        ...editor,
        validationError: t('agentCenter.commands.validation.invalidName'),
      });
      return;
    }
    if (editor.scope === 'project' && !normalizedProjectPath) {
      setEditor({
        ...editor,
        validationError: t(
          'agentCenter.commands.validation.projectPathRequired'
        ),
      });
      return;
    }
    if (!editor.body.trim()) {
      setEditor({
        ...editor,
        validationError: t('agentCenter.commands.validation.bodyRequired'),
      });
      return;
    }

    let definition: AgentCommandWriteDefinition;
    try {
      definition = writeDefinition(editor);
    } catch (nextError) {
      setEditor({
        ...editor,
        validationError: operationFailure(nextError).message,
      });
      return;
    }

    const succeeded = await run(
      `${editor.mode}:${provider}:${name}`,
      () => {
        if (editor.mode === 'edit' && editor.item) {
          return machineClient.updateAgentCommand({
            target: locatorFor(editor.item, normalizedProjectPath),
            expected_revision: editor.item.revision,
            definition,
          });
        }
        return machineClient.createAgentCommand({
          target: {
            provider,
            scope: editor.scope,
            name,
            project_path:
              editor.scope === 'project' ? normalizedProjectPath : undefined,
          },
          definition,
          replace: false,
        });
      },
      (failure) =>
        setEditor((current) =>
          current
            ? {
                ...current,
                validationError: failure.message,
                revisionConflict: failure.revisionConflict,
              }
            : null
        )
    );
    if (succeeded) closeEditor();
  };

  const refreshAfterConflict = async () => {
    closeEditor();
    await refresh();
  };

  const handleRemove = async (item: AgentCommandView) => {
    if (!machineClient) return;
    const result = await ConfirmDialog.show({
      title: t('agentCenter.commands.remove.title', { name: item.name }),
      message: t('agentCenter.commands.remove.message'),
      confirmText: t('agentCenter.commands.actions.remove'),
      cancelText: t('buttons.cancel'),
      variant: 'destructive',
    });
    if (result !== 'confirmed') return;
    await run(`remove:${item.installation_id}`, () =>
      machineClient.removeAgentCommand({
        target: locatorFor(item, normalizedProjectPath),
        expected_revision: item.revision,
      })
    );
  };

  const filteredItems = useMemo(() => {
    const normalizedSearch = search.trim().toLocaleLowerCase();
    const items = providerInventory?.items ?? [];
    if (!normalizedSearch) return items;
    return items.filter((item) => {
      const description = definitionDescription(item.definition) ?? '';
      return [item.name, description]
        .join(' ')
        .toLocaleLowerCase()
        .includes(normalizedSearch);
    });
  }, [providerInventory?.items, search]);

  const canCreate = Boolean(
    machineClient &&
      activeInventory &&
      providerInventory?.installed &&
      providerInventory.capabilities.creatable &&
      providerInventory.capabilities.writable_formats.length > 0 &&
      providerInventory.capabilities.supported_scopes.length > 0
  );
  const supportsProject =
    providerDescriptor?.capabilities.supported_scopes.includes('project') ??
    false;
  const editorBusy = Boolean(editor && busyKey?.startsWith(`${editor.mode}:`));

  return (
    <>
      <SettingsCard
        title={t('agentCenter.commands.manager.title')}
        description={t('agentCenter.commands.manager.description')}
        headerAction={
          <div className="flex gap-half">
            <PrimaryButton
              variant="tertiary"
              value={t('agentCenter.commands.actions.refresh')}
              onClick={() => void refresh()}
              disabled={loading || busyKey !== null || !machineClient}
              actionIcon={loading ? 'spinner' : undefined}
            />
            {providerDescriptor?.capabilities.creatable && (
              <PrimaryButton
                value={t('agentCenter.commands.actions.add')}
                onClick={openAddDialog}
                disabled={!canCreate || busyKey !== null}
              />
            )}
          </div>
        }
      >
        {supportsProject && (
          <div className="space-y-2">
            <label
              htmlFor="agent-commands-project-path"
              className="text-sm font-medium text-normal"
            >
              {t('agentCenter.commands.projectPath.label')}
            </label>
            <div className="flex gap-2">
              <SettingsInput
                id="agent-commands-project-path"
                value={formatPathForDisplay(projectPath)}
                onChange={setProjectPath}
                placeholder={t('agentCenter.commands.projectPath.placeholder')}
              />
              <button
                type="button"
                onClick={() => void refresh()}
                disabled={loading || busyKey !== null || !machineClient}
                className="min-h-11 min-w-11 rounded-sm border border-border px-3 text-low hover:text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:cursor-not-allowed disabled:opacity-40"
                title={t('agentCenter.commands.projectPath.discover')}
                aria-label={t('agentCenter.commands.projectPath.discover')}
              >
                <ArrowClockwiseIcon
                  className="size-icon-xs"
                  aria-hidden="true"
                />
              </button>
            </div>
            {loadedProjectPath !== normalizedProjectPath && !loading && (
              <p className="text-xs text-low" role="status">
                {t('agentCenter.commands.projectPath.refreshRequired')}
              </p>
            )}
          </div>
        )}

        <label className="block space-y-2 text-sm font-medium text-normal">
          <span>{t('agentCenter.commands.search.label')}</span>
          <span className="relative block">
            <MagnifyingGlassIcon
              className="pointer-events-none absolute left-3 top-1/2 size-icon-xs -translate-y-1/2 text-low"
              aria-hidden="true"
            />
            <input
              type="search"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t('agentCenter.commands.search.placeholder')}
              className="min-h-11 w-full rounded-sm border border-border bg-secondary py-2 pl-10 pr-3 text-sm text-high placeholder:text-low focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
            />
          </span>
        </label>

        {error && (
          <div
            className="flex flex-wrap items-center justify-between gap-2 rounded-sm border border-error/50 bg-error/10 p-3 text-sm text-error"
            role="alert"
          >
            <span>{error.message}</span>
            {error.revisionConflict && (
              <button
                type="button"
                onClick={() => void refresh()}
                className="min-h-11 rounded-sm px-3 font-medium underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
              >
                {t('agentCenter.commands.actions.refresh')}
              </button>
            )}
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
              ? t('agentCenter.commands.states.loading')
              : t('agentCenter.commands.states.unavailable')}
          </div>
        ) : !providerInventory ? (
          <div className="py-4 text-sm text-low" role="status">
            {t('agentCenter.commands.states.unsupportedProvider')}
          </div>
        ) : (
          <AgentCommandInventory
            inventory={providerInventory}
            items={filteredItems}
            searchActive={Boolean(search.trim())}
            busyKey={busyKey}
            projectPath={normalizedProjectPath}
            onEdit={(item) => setEditor(editorFromItem(item))}
            onRemove={(item) => void handleRemove(item)}
            onToggle={(item, enabled) => {
              if (!machineClient) return;
              void run(`toggle:${item.installation_id}`, () =>
                machineClient.toggleAgentCommand({
                  target: locatorFor(item, normalizedProjectPath),
                  expected_revision: item.revision,
                  enabled,
                })
              );
            }}
          />
        )}
      </SettingsCard>

      <AgentCommandEditorDialog
        editor={editor}
        inventory={providerInventory}
        projectPath={normalizedProjectPath}
        busy={editorBusy}
        onChange={updateEditor}
        onSubmit={() => void submitEditor()}
        onRequestClose={() => void requestCloseEditor()}
        onRefreshConflict={() => void refreshAfterConflict()}
      />
    </>
  );
}
