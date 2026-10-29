import { forwardRef, type ReactNode } from 'react';
import {
  ListMagnifyingGlassIcon,
  TerminalWindowIcon,
  FileTextIcon,
  GlobeIcon,
} from '@phosphor-icons/react';
import { ChatDisclosureRow } from './ChatDisclosure';
import { ChatElapsedTime } from './ChatElapsedTime';
import type { ToolStatusLike } from './ToolStatusDot';

interface ChatToolSummaryProps {
  summary: string;
  detail?: string;
  className?: string;
  expanded?: boolean;
  controlsId?: string;
  onToggle?: () => void;
  status?: ToolStatusLike;
  statusLabel?: ReactNode;
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
    detail,
    className,
    expanded,
    controlsId,
    onToggle,
    status,
    statusLabel,
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
  const canExpand = Boolean(isTruncated && onToggle);
  const isClickable = Boolean(onViewContent || canExpand);
  const isActive =
    active ??
    (status?.status === 'created' || status?.status === 'pending_approval');

  const Icon =
    toolName === 'Bash' || actionType === 'command_run'
      ? TerminalWindowIcon
      : actionType === 'file_read'
        ? FileTextIcon
        : actionType === 'web_fetch'
          ? GlobeIcon
          : ListMagnifyingGlassIcon;

  return (
    <ChatDisclosureRow
      className={className}
      icon={<Icon className="size-icon-base" />}
      label={summary}
      ariaLabel={summary}
      summary={detail}
      summaryRef={ref}
      wrapLabel={expanded && !detail}
      status={status}
      statusLabel={statusLabel}
      active={isActive}
      expanded={canExpand && !onViewContent ? expanded : undefined}
      expandable={isClickable}
      controlsId={controlsId}
      onClick={isClickable ? (onViewContent ?? onToggle) : undefined}
      trailing={
        <ChatElapsedTime
          startedAt={startedAt}
          endedAt={endedAt}
          active={isActive}
        />
      }
    />
  );
});
