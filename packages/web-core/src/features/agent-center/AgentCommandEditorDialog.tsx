import { useTranslation } from 'react-i18next';
import { SpinnerIcon } from '@phosphor-icons/react';
import { Button } from '@vibe/ui/components/Button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';
import { Textarea } from '@vibe/ui/components/Textarea';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import type {
  AgentCommandProviderInventoryView,
  AgentCommandScope,
} from 'shared/types';
import type { CommandEditorState } from './agent-command-model';

export function AgentCommandEditorDialog({
  editor,
  inventory,
  projectPath,
  busy,
  onChange,
  onSubmit,
  onRequestClose,
  onRefreshConflict,
}: {
  editor: CommandEditorState | null;
  inventory: AgentCommandProviderInventoryView | undefined;
  projectPath: string;
  busy: boolean;
  onChange: (patch: Partial<CommandEditorState>) => void;
  onSubmit: () => void;
  onRequestClose: () => void;
  onRefreshConflict: () => void;
}) {
  const { t } = useTranslation('common');

  return (
    <Dialog
      open={editor !== null}
      onOpenChange={(open) => {
        if (!open && !busy) onRequestClose();
      }}
      uncloseable={busy}
    >
      <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto sm:max-w-[680px]">
        {editor && inventory && (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              onSubmit();
            }}
          >
            <DialogHeader>
              <DialogTitle>
                {t(
                  editor.mode === 'add'
                    ? 'agentCenter.commands.editor.addTitle'
                    : 'agentCenter.commands.editor.editTitle',
                  { name: editor.name }
                )}
              </DialogTitle>
              <DialogDescription className="text-left">
                {t('agentCenter.commands.editor.description')}
              </DialogDescription>
            </DialogHeader>
            <div className="space-y-4 py-4">
              <div className="rounded-sm border border-border bg-secondary/30 p-3 text-sm text-low">
                {t('agentCenter.commands.formats.label')}:{' '}
                <span className="font-medium text-normal">
                  {t(`agentCenter.commands.formats.${editor.format}`)}
                </span>
              </div>

              {editor.mode === 'add' &&
                inventory.capabilities.supported_scopes.length > 1 && (
                  <label className="block space-y-2 text-sm font-medium text-normal">
                    <span>{t('agentCenter.commands.editor.scope')}</span>
                    <select
                      value={editor.scope}
                      onChange={(event) =>
                        onChange({
                          scope: event.target.value as AgentCommandScope,
                        })
                      }
                      className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
                    >
                      {inventory.capabilities.supported_scopes.map((scope) => (
                        <option key={scope} value={scope}>
                          {t(`agentCenter.commands.scopes.${scope}`)}
                        </option>
                      ))}
                    </select>
                  </label>
                )}

              <label className="block space-y-2 text-sm font-medium text-normal">
                <span>{t('agentCenter.commands.editor.name')}</span>
                <input
                  value={editor.name}
                  disabled={editor.mode === 'edit'}
                  onChange={(event) => onChange({ name: event.target.value })}
                  className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:opacity-60"
                  autoFocus={editor.mode === 'add'}
                />
                <span className="block text-xs font-normal text-low">
                  {t('agentCenter.commands.editor.nameHelp')}
                </span>
              </label>

              <label className="block space-y-2 text-sm font-medium text-normal">
                <span>
                  {t('agentCenter.commands.editor.commandDescription')}
                </span>
                <input
                  value={editor.description}
                  onChange={(event) =>
                    onChange({ description: event.target.value })
                  }
                  className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
                />
              </label>

              {editor.format === 'codex_legacy_markdown' && (
                <label className="block space-y-2 text-sm font-medium text-normal">
                  <span>{t('agentCenter.commands.editor.argumentHint')}</span>
                  <input
                    value={editor.argumentHint}
                    onChange={(event) =>
                      onChange({ argumentHint: event.target.value })
                    }
                    className="min-h-11 w-full rounded-sm border border-border bg-panel px-3 text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
                  />
                </label>
              )}

              <label className="block space-y-2 text-sm font-medium text-normal">
                <span>
                  {t(
                    editor.format === 'gemini_toml'
                      ? 'agentCenter.commands.editor.prompt'
                      : 'agentCenter.commands.editor.body'
                  )}
                </span>
                <Textarea
                  value={editor.body}
                  onChange={(event) => onChange({ body: event.target.value })}
                  rows={14}
                  className="font-ibm-plex-mono text-xs"
                />
              </label>

              {editor.scope === 'project' && (
                <p className="text-xs text-low">
                  {t('agentCenter.commands.editor.projectTarget', {
                    path:
                      formatPathForDisplay(projectPath) ||
                      t('agentCenter.commands.editor.noProjectPath'),
                  })}
                </p>
              )}

              {editor.validationError && (
                <div
                  className="flex flex-wrap items-center justify-between gap-2 text-sm text-error"
                  role="alert"
                >
                  <span>{editor.validationError}</span>
                  {editor.revisionConflict && (
                    <button
                      type="button"
                      className="min-h-11 rounded-sm px-3 font-medium underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
                      onClick={onRefreshConflict}
                    >
                      {t('agentCenter.commands.actions.refresh')}
                    </button>
                  )}
                </div>
              )}
            </div>
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={onRequestClose}
              >
                {t('buttons.cancel')}
              </Button>
              <Button type="submit" disabled={busy}>
                {busy && (
                  <SpinnerIcon
                    className="mr-2 size-icon-xs animate-spin"
                    aria-hidden="true"
                  />
                )}
                {t(
                  editor.mode === 'add'
                    ? 'agentCenter.commands.actions.add'
                    : 'agentCenter.commands.actions.save'
                )}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}
