import {
  useEffect,
  useRef,
  type ClipboardEvent,
  type KeyboardEvent,
} from 'react';
import type { LocalAttachmentMetadata } from '@vibe/ui/components/WorkspaceContext';
import type { SendMessageShortcut } from 'shared/types';
import { useTranslation } from 'react-i18next';

export interface AgentWorkbenchComposerEditorProps {
  focusKey: string;
  placeholder: string;
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void;
  disabled: boolean;
  sendShortcut?: SendMessageShortcut;
  onPasteFiles: (files: File[]) => void;
  localAttachments?: LocalAttachmentMetadata[];
}

export function AgentWorkbenchComposerEditor({
  focusKey,
  placeholder,
  value,
  onChange,
  onSubmit,
  disabled,
  sendShortcut = 'Enter',
  onPasteFiles,
  localAttachments,
}: AgentWorkbenchComposerEditorProps) {
  const { t } = useTranslation('common');
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    textareaRef.current?.focus({ preventScroll: true });
  }, [focusKey]);

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing || event.key !== 'Enter') return;

    const hasModifier = event.metaKey || event.ctrlKey;
    const shouldSubmit =
      sendShortcut === 'Enter'
        ? !event.shiftKey && !hasModifier
        : hasModifier && !event.shiftKey;
    if (shouldSubmit) {
      event.preventDefault();
      onSubmit();
    }
  };

  const handlePaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = Array.from(event.clipboardData.files);
    if (files.length === 0) return;
    event.preventDefault();
    onPasteFiles(files);
  };

  return (
    <div className="vk-agent-workbench-composer flex min-h-double max-h-[50vh] flex-col gap-1 overflow-hidden">
      <textarea
        ref={textareaRef}
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        rows={3}
        className="min-h-double w-full flex-1 resize-none bg-transparent px-1 py-2 text-sm leading-6 outline-none placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50"
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={handleKeyDown}
        onPaste={handlePaste}
      />
      {(localAttachments?.length ?? 0) > 0 && (
        <p className="px-1 text-xs text-muted-foreground">
          {t('agentWorkbench.composer.attachmentsReady', {
            count: localAttachments?.length ?? 0,
            defaultValue:
              localAttachments?.length === 1
                ? '{{count}} attachment ready'
                : '{{count}} attachments ready',
          })}
        </p>
      )}
    </div>
  );
}
