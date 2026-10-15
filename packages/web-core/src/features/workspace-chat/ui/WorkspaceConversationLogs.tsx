import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { ArrowLeftIcon } from '@phosphor-icons/react';
import { Button } from '@vibe/ui/components/Button';
import { VirtualizedProcessLogs } from '@/shared/components/VirtualizedProcessLogs';
import { useLogStream } from '@/shared/hooks/useLogStream';
import type { LogsPanelContent } from '@/shared/types/actions';

export type ConversationLogPreview = Exclude<
  LogsPanelContent,
  { type: 'terminal' }
>;

/** Reuses the workspace log viewer without opening the full-page Inspector. */
export function WorkspaceConversationLogs({
  content,
  onBack,
}: {
  content: ConversationLogPreview;
  onBack: () => void;
}) {
  const { t } = useTranslation(['common', 'tasks']);
  const processId = content.type === 'process' ? content.processId : '';
  const { logs, error } = useLogStream(processId);
  const toolLogs = useMemo(
    () =>
      content.type === 'tool'
        ? content.content
            .split('\n')
            .map((line) => ({ type: 'STDOUT' as const, content: line }))
        : [],
    [content]
  );
  const title =
    content.type === 'tool'
      ? content.toolName
      : t('preview.logs.label', { ns: 'tasks' });

  return (
    <section
      className="flex h-full min-h-0 min-w-0 flex-col bg-primary"
      aria-label={title}
      data-testid="conversation-log-preview"
      onKeyDown={(event) => {
        if (event.key !== 'Escape' || event.defaultPrevented) return;
        event.preventDefault();
        event.stopPropagation();
        onBack();
      }}
    >
      <div className="flex shrink-0 items-center gap-half border-b border-secondary p-half">
        <Button variant="ghost" size="xs" onClick={onBack}>
          <ArrowLeftIcon className="size-icon-sm" />
          {t('commandBar.back', { ns: 'common' })}
        </Button>
        <span className="min-w-0 truncate text-base font-medium text-high">
          {title}
        </span>
      </div>
      {content.type === 'tool' && content.command && (
        <div className="shrink-0 border-b border-secondary px-base py-half">
          <code className="block max-h-24 overflow-auto whitespace-pre-wrap break-words font-ibm-plex-mono text-sm text-low">
            {content.command}
          </code>
        </div>
      )}
      <div className="min-h-0 flex-1">
        <VirtualizedProcessLogs
          logs={content.type === 'tool' ? toolLogs : logs}
          error={content.type === 'tool' ? null : error}
          searchQuery=""
          matchIndices={[]}
          currentMatchIndex={0}
        />
      </div>
    </section>
  );
}
