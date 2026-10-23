import { forwardRef } from 'react';
import {
  ListMagnifyingGlassIcon,
  TerminalWindowIcon,
  FileTextIcon,
  GlobeIcon,
} from '@phosphor-icons/react';
import { cn } from '../lib/cn';
import { ActivityText } from './ActivityText';
import { ChatElapsedTime } from './ChatElapsedTime';
import { ToolStatusDot, type ToolStatusLike } from './ToolStatusDot';

interface ChatToolSummaryProps {
  summary: string;
  className?: string;
  expanded?: boolean;
  onToggle?: () => void;
  status?: ToolStatusLike;
  active?: boolean;
  onViewContent?: () => void;
  toolName?: string;
  isTruncated?: boolean;
  /** The action type for determining the icon */
  actionType?: string;
  startedAt?: string | null;
  endedAt?: string | null;
}

export const ChatToolSummary = forwardRef<
  HTMLSpanElement,
  ChatToolSummaryProps
>(function ChatToolSummary(
  {
    summary,
    className,
    expanded,
    onToggle,
    status,
    active,
    onViewContent,
    toolName,
    isTruncated,
    actionType,
    startedAt,
    endedAt,
  },
  ref
) {
  // Can expand if text is truncated and onToggle is provided
  const canExpand = isTruncated && onToggle;
  const isClickable = Boolean(onViewContent || canExpand);
  const isActive =
    active ??
    (status?.status === 'created' || status?.status === 'pending_approval');

  const handleClick = () => {
    if (onViewContent) {
      onViewContent();
    } else if (canExpand) {
      onToggle();
    }
  };

  // Determine icon based on action type or tool name
  const getIcon = () => {
    if (toolName === 'Bash') return TerminalWindowIcon;
    switch (actionType) {
      case 'file_read':
        return FileTextIcon;
      case 'search':
        return ListMagnifyingGlassIcon;
      case 'web_fetch':
        return GlobeIcon;
      default:
        return ListMagnifyingGlassIcon;
    }
  };
  const Icon = getIcon();

  return (
    <div
      className={cn(
        '-mx-half flex min-w-0 items-center gap-base rounded-sm px-half py-0.5 text-sm text-low transition-colors',
        isClickable &&
          'cursor-pointer hover:bg-muted/30 hover:text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand',
        className
      )}
      onClick={isClickable ? handleClick : undefined}
      role={isClickable ? 'button' : undefined}
      tabIndex={isClickable ? 0 : undefined}
      aria-expanded={canExpand && !onViewContent ? expanded : undefined}
      onKeyDown={(event) => {
        if (!isClickable || (event.key !== 'Enter' && event.key !== ' '))
          return;
        event.preventDefault();
        handleClick();
      }}
    >
      <span className="relative inline-flex size-icon-base shrink-0 items-center justify-center">
        <Icon className="size-icon-base" />
        {status && (
          <span className="pointer-events-none absolute -bottom-0.5 -left-0.5 inline-flex">
            <ToolStatusDot status={status} active={isActive} />
          </span>
        )}
      </span>
      <ActivityText
        active={isActive}
        ref={ref}
        className={cn(
          'min-w-0 flex-1',
          !expanded && 'truncate',
          expanded && 'whitespace-pre-wrap break-all'
        )}
      >
        {summary}
      </ActivityText>
      <ChatElapsedTime
        startedAt={startedAt}
        endedAt={endedAt}
        active={isActive}
      />
    </div>
  );
});
