import type { ReactNode } from 'react';
import { ChatDotsIcon } from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { ChatDisclosure } from './ChatDisclosure';
import { ChatElapsedTime } from './ChatElapsedTime';
import { getThinkingPreview } from './ChatThinkingMessage';

export interface ThinkingEntry {
  content: string;
  expansionKey: string;
  startedAt?: string | null;
  endedAt?: string | null;
}

export interface ChatCollapsedThinkingRenderProps {
  content: string;
  workspaceId?: string;
  className?: string;
}

interface ChatCollapsedThinkingProps {
  entries: ThinkingEntry[];
  expanded: boolean;
  isHovered: boolean;
  onToggle: () => void;
  onHoverChange: (hovered: boolean) => void;
  className?: string;
  workspaceId?: string;
  startedAt?: string | null;
  endedAt?: string | null;
  renderMarkdown: (props: ChatCollapsedThinkingRenderProps) => ReactNode;
}

/**
 * A collapsible group for thinking entries in previous conversation turns.
 * When collapsed, shows "Thinking" with the thinking icon.
 * When expanded, shows all thinking entries with their full content.
 */
export function ChatCollapsedThinking({
  entries,
  expanded,
  onToggle,
  onHoverChange,
  className,
  workspaceId,
  startedAt,
  endedAt,
  renderMarkdown,
}: ChatCollapsedThinkingProps) {
  const { t } = useTranslation('common');

  if (entries.length === 0) return null;

  return (
    <ChatDisclosure
      className={className}
      icon={<ChatDotsIcon className="size-icon-base" />}
      label={t('conversation.thinking')}
      summary={!expanded ? getThinkingPreview(entries[0].content) : undefined}
      expanded={expanded}
      onToggle={onToggle}
      onHoverChange={onHoverChange}
      trailing={
        <ChatElapsedTime
          startedAt={startedAt}
          endedAt={endedAt}
          active={false}
        />
      }
    >
      {expanded &&
        entries.map((entry) => (
          <div key={entry.expansionKey}>
            {renderMarkdown({
              content: entry.content,
              workspaceId,
              className: 'text-base text-low',
            })}
          </div>
        ))}
    </ChatDisclosure>
  );
}
