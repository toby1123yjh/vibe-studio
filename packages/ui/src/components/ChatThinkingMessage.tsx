import { useState, type ReactNode } from 'react';
import { ChatDotsIcon } from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { ChatDisclosure } from './ChatDisclosure';
import { ChatElapsedTime } from './ChatElapsedTime';

export interface ChatThinkingMessageRenderProps {
  content: string;
  workspaceId?: string;
  className?: string;
}

interface ChatThinkingMessageProps {
  content: string;
  label?: string;
  className?: string;
  workspaceId?: string;
  startedAt?: string | null;
  endedAt?: string | null;
  active?: boolean;
  expanded?: boolean;
  onToggle?: () => void;
  renderMarkdown: (props: ChatThinkingMessageRenderProps) => ReactNode;
}

/** Preview only actual reasoning. During streaming, wait for a completed line
 * and advance by paragraph rather than changing the header on every token.
 */
export function getThinkingPreview(content: string, active = false): string {
  const complete = active
    ? content.slice(0, content.lastIndexOf('\n') + 1)
    : content;
  const paragraphs = complete.split(/\r?\n[\t ]*\r?\n/);
  const paragraph = active
    ? [...paragraphs].reverse().find((part) => part.trim())
    : paragraphs.find((part) => part.trim());
  return (paragraph?.trim().split(/\r?\n/, 1)[0] ?? '')
    .replaceAll('**', '')
    .trim();
}

export function ChatThinkingMessage({
  content,
  label,
  className,
  workspaceId,
  startedAt,
  endedAt,
  active = false,
  expanded: controlledExpanded,
  onToggle,
  renderMarkdown,
}: ChatThinkingMessageProps) {
  const { t } = useTranslation('common');
  const [localExpanded, setLocalExpanded] = useState(false);
  const expanded = controlledExpanded ?? localExpanded;

  return (
    <ChatDisclosure
      className={className}
      icon={<ChatDotsIcon className="size-icon-base" />}
      label={label ?? t('conversation.thinking')}
      summary={!expanded ? getThinkingPreview(content, active) : undefined}
      active={active}
      expanded={expanded}
      onToggle={onToggle ?? (() => setLocalExpanded((value) => !value))}
      trailing={
        <ChatElapsedTime
          startedAt={startedAt}
          endedAt={endedAt}
          active={active}
        />
      }
    >
      {expanded &&
        renderMarkdown({
          content,
          workspaceId,
          className: 'text-base text-low',
        })}
    </ChatDisclosure>
  );
}
