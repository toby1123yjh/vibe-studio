import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
  PencilSimpleIcon,
  ArrowUUpLeftIcon,
  CaretDownIcon,
} from '@phosphor-icons/react';
import { cn } from '../lib/cn';
import { Tooltip } from './Tooltip';

export interface ChatUserMessageRenderProps {
  content: string;
  workspaceId?: string;
}

interface ChatUserMessageProps {
  content: string;
  expanded?: boolean;
  onToggle?: () => void;
  className?: string;
  workspaceId?: string;
  onEdit?: () => void;
  onReset?: () => void;
  isGreyed?: boolean;
  renderMarkdown: (props: ChatUserMessageRenderProps) => ReactNode;
}

export function ChatUserMessage({
  content,
  expanded = true,
  onToggle,
  className,
  workspaceId,
  onEdit,
  onReset,
  isGreyed,
  renderMarkdown,
}: ChatUserMessageProps) {
  const { t } = useTranslation('tasks');

  const actionClassName =
    'flex size-8 items-center justify-center rounded-[var(--vk-radius-sm)] text-low hover:bg-secondary hover:text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand transition-colors duration-[var(--vk-duration-fast)] motion-reduce:transition-none [@media(pointer:coarse)]:size-11';
  const bubbleClassName =
    'min-w-0 rounded-[var(--vk-radius-lg)] bg-secondary px-plusfifty py-base text-normal [overflow-wrap:anywhere] [&_img]:max-w-full [&_pre]:max-w-full [&_pre]:overflow-x-auto [&_pre]:[overflow-wrap:normal] [&_table]:block [&_table]:max-w-full [&_table]:overflow-x-auto';
  const actions =
    !isGreyed && (onEdit || onReset || (expanded && onToggle)) ? (
      <div className="flex items-center justify-end gap-half pt-half transition-opacity duration-[var(--vk-duration-fast)] motion-reduce:transition-none [@media(hover:hover)]:pointer-events-none [@media(hover:hover)]:opacity-0 group-hover/user-message:pointer-events-auto group-hover/user-message:opacity-100 group-focus-within/user-message:pointer-events-auto group-focus-within/user-message:opacity-100">
        {onReset && (
          <Tooltip content={t('conversation.actions.resetTooltip')}>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                onReset();
              }}
              className={actionClassName}
              aria-label={t('conversation.actions.reset')}
            >
              <ArrowUUpLeftIcon className="size-icon-xs" aria-hidden />
            </button>
          </Tooltip>
        )}
        {onEdit && (
          <Tooltip content={t('conversation.actions.edit')}>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                onEdit();
              }}
              className={actionClassName}
              aria-label={t('conversation.actions.edit')}
            >
              <PencilSimpleIcon className="size-icon-xs" aria-hidden />
            </button>
          </Tooltip>
        )}
        {expanded && onToggle && (
          <button
            type="button"
            onClick={onToggle}
            className={actionClassName}
            aria-label={t('conversation.you')}
            aria-expanded={expanded}
          >
            <CaretDownIcon className="size-icon-xs" aria-hidden />
          </button>
        )}
      </div>
    ) : undefined;

  return (
    <div
      role="group"
      aria-label={t('conversation.you')}
      className={cn(
        'group/user-message relative ml-auto w-fit min-w-0 max-w-[min(90%,72ch)]',
        isGreyed && 'pointer-events-none opacity-50',
        className
      )}
    >
      {expanded ? (
        <div className={bubbleClassName}>
          {renderMarkdown({ content, workspaceId })}
        </div>
      ) : onToggle ? (
        <button
          type="button"
          onClick={onToggle}
          aria-label={t('conversation.you')}
          aria-expanded={expanded}
          disabled={isGreyed}
          className={cn(
            bubbleClassName,
            'flex max-w-full items-center gap-base text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand'
          )}
        >
          <span className="min-w-0 truncate">{content}</span>
          <CaretDownIcon
            className="size-icon-xs shrink-0 -rotate-90"
            aria-hidden
          />
        </button>
      ) : null}
      {actions}
    </div>
  );
}
