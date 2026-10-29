import { type ReactNode, useRef } from 'react';
import { CheckIcon, PaperclipIcon, XIcon } from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { Checkbox } from './Checkbox';
import { ChatBoxBase, VisualVariant, type DropzoneProps } from './ChatBoxBase';
import { DropdownMenuItem, DropdownMenuLabel } from './Dropdown';
import { PrimaryButton } from './PrimaryButton';
import { ChatComposerActivity } from './ChatComposerActivity';
import type { LocalAttachmentMetadata } from './WorkspaceContext';
import { ToolbarDropdown, ToolbarIconButton } from './Toolbar';

export interface EditorProps {
  value: string;
  onChange: (value: string) => void;
  getValue?: () => string;
}

export interface ModelSelectorProps<TExecutorConfig = unknown> {
  onAdvancedSettings: () => void;
  presets: string[];
  selectedPreset: string | null;
  onPresetSelect: (presetId: string | null) => void;
  onOverrideChange: (partial: Partial<TExecutorConfig>) => void;
  executorConfig: TExecutorConfig | null;
  presetOptions: TExecutorConfig | null | undefined;
}

export interface ExecutorProps<TExecutor extends string = string> {
  selected: TExecutor | null;
  options: TExecutor[];
  onChange: (executor: TExecutor) => void;
  afterSelector?: ReactNode;
}

export interface SaveAsDefaultProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  visible: boolean;
}

export interface LinkedTaskBadgeProps {
  simpleId: string;
  title: string;
  onRemove?: () => void;
}

export interface CreateChatBoxEditorRenderProps<
  TExecutor extends string = string,
> {
  value: string;
  onChange: (value: string) => void;
  onCmdEnter: (markdown: string) => void;
  disabled: boolean;
  repoIds?: string[];
  repoId?: string;
  executor: TExecutor | null;
  onPasteFiles?: (files: File[]) => void;
  localAttachments?: LocalAttachmentMetadata[];
}

interface CreateChatBoxProps<TExecutor extends string = string> {
  editor: EditorProps;
  renderEditor: (props: CreateChatBoxEditorRenderProps<TExecutor>) => ReactNode;
  agentIcon?: ReactNode;
  agentBrand?: string;
  onSend: (markdown: string) => void;
  isSending: boolean;
  disabled?: boolean;
  executor: ExecutorProps<TExecutor>;
  formatExecutorLabel?: (executor: TExecutor) => string;
  emptyExecutorLabel?: string;
  saveAsDefault?: SaveAsDefaultProps;
  error?: string | null;
  repoIds?: string[];
  repoId?: string;
  modelSelector?: ReactNode;
  onPasteFiles?: (files: File[]) => void;
  localAttachments?: LocalAttachmentMetadata[];
  dropzone?: DropzoneProps;
  projectSelector: ReactNode;
  sendDisabled?: boolean;
  linkedTask?: LinkedTaskBadgeProps | null;
}

/**
 * Lightweight chat box for create mode.
 * Supports sending and attachments - no queue, stop, or feedback functionality.
 */
function defaultExecutorLabel(executor: string) {
  return executor
    .replace(/[_-]+/g, ' ')
    .toLowerCase()
    .replace(/\b\w/g, (char) => char.toUpperCase());
}

export function CreateChatBox<TExecutor extends string = string>({
  editor,
  renderEditor,
  agentIcon,
  agentBrand,
  onSend,
  isSending,
  disabled = false,
  executor,
  formatExecutorLabel = defaultExecutorLabel,
  emptyExecutorLabel = 'Select Executor',
  saveAsDefault,
  error,
  repoIds,
  repoId,
  modelSelector,
  onPasteFiles,
  localAttachments,
  dropzone,
  projectSelector,
  sendDisabled = false,
  linkedTask,
}: CreateChatBoxProps<TExecutor>) {
  const { t } = useTranslation(['common', 'tasks']);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const isDisabled = disabled;
  const canSend =
    editor.value.trim().length > 0 &&
    !isDisabled &&
    !isSending &&
    !sendDisabled;

  const handleCmdEnter = (markdown: string) => {
    if (markdown.trim() && !isDisabled && !isSending && !sendDisabled) {
      onSend(markdown);
    }
  };

  const handleAttachClick = () => {
    fileInputRef.current?.click();
  };

  const handleFileInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files || []);
    if (files.length > 0 && onPasteFiles) {
      onPasteFiles(files);
    }
    e.target.value = '';
  };

  const executorLabel = executor.selected
    ? formatExecutorLabel(executor.selected)
    : emptyExecutorLabel;

  return (
    <ChatBoxBase
      editor={renderEditor({
        value: editor.value,
        onChange: editor.onChange,
        onCmdEnter: handleCmdEnter,
        disabled: isDisabled,
        repoIds,
        repoId,
        executor: executor.selected ?? null,
        onPasteFiles,
        localAttachments,
      })}
      error={error}
      visualVariant={VisualVariant.NORMAL}
      dropzone={dropzone}
      modelSelector={modelSelector}
      status={
        isSending ? (
          <ChatComposerActivity
            state="sending"
            label={t('tasks:conversation.actions.sending')}
          />
        ) : undefined
      }
      headerLeft={
        <>
          <span
            data-agent-brand={agentBrand}
            className="flex min-w-0 items-center gap-2"
          >
            {agentIcon}
            <ToolbarDropdown
              className="vk-agent-brand-label"
              label={executorLabel}
              disabled={isDisabled}
            >
              <DropdownMenuLabel>
                {t('tasks:conversation.executors')}
              </DropdownMenuLabel>
              {executor.options.map((exec) => (
                <DropdownMenuItem
                  key={exec}
                  icon={executor.selected === exec ? CheckIcon : undefined}
                  onClick={() => executor.onChange(exec)}
                >
                  {formatExecutorLabel(exec)}
                </DropdownMenuItem>
              ))}
            </ToolbarDropdown>
          </span>
          {executor.afterSelector}
          {saveAsDefault?.visible && (
            <label className="flex items-center gap-1.5 text-sm text-low cursor-pointer ml-2">
              <Checkbox
                checked={saveAsDefault.checked}
                onCheckedChange={saveAsDefault.onChange}
                className="h-3.5 w-3.5"
                disabled={isDisabled}
              />
              <span>{t('tasks:conversation.saveAsDefault')}</span>
            </label>
          )}
        </>
      }
      context={
        <>
          {projectSelector}
          {linkedTask && (
            <div
              className="inline-flex min-w-0 items-center gap-half border-l border-border pl-base text-sm text-low"
              title={linkedTask.title}
            >
              <span className="truncate font-mono text-sm text-normal">
                {linkedTask.simpleId}
              </span>
              {linkedTask.onRemove && (
                <button
                  type="button"
                  onClick={linkedTask.onRemove}
                  disabled={isDisabled}
                  className="inline-flex min-h-8 min-w-8 shrink-0 items-center justify-center rounded-sm text-low hover:text-error focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand disabled:cursor-not-allowed disabled:opacity-50"
                  aria-label={t('tasks:conversation.composer.removeTask', {
                    task: linkedTask.simpleId,
                  })}
                >
                  <XIcon className="size-icon-xs" weight="bold" />
                </button>
              )}
            </div>
          )}
        </>
      }
      footerLeft={
        <>
          <ToolbarIconButton
            type="button"
            icon={PaperclipIcon}
            aria-label={t('tasks:taskFormDialog.attachFile')}
            title={t('tasks:taskFormDialog.attachFile')}
            onClick={handleAttachClick}
            disabled={isDisabled}
            className="min-h-8 min-w-8 shrink-0 rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand focus-visible:ring-inset [@media(pointer:coarse)]:min-h-11 [@media(pointer:coarse)]:min-w-11"
          />
          <input
            ref={fileInputRef}
            type="file"
            multiple
            className="hidden"
            onChange={handleFileInputChange}
          />
        </>
      }
      footerRight={
        <PrimaryButton
          onClick={() => handleCmdEnter(editor.getValue?.() ?? editor.value)}
          disabled={!canSend}
          className="min-w-20 justify-center"
          value={t('tasks:conversation.workspace.create')}
          aria-label={
            isSending
              ? t('tasks:conversation.workspace.creating')
              : t('tasks:conversation.workspace.create')
          }
        />
      }
    />
  );
}
