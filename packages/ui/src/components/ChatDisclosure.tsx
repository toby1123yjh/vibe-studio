import { useId, type ReactNode, type Ref } from 'react';
import { CaretDownIcon } from '@phosphor-icons/react';
import { cn } from '../lib/cn';
import { ActivityText } from './ActivityText';
import { ToolStatusDot, type ToolStatusLike } from './ToolStatusDot';

interface ChatDisclosureRowProps {
  icon: ReactNode;
  label: ReactNode;
  summary?: string;
  summaryRef?: Ref<HTMLSpanElement>;
  status?: ToolStatusLike;
  statusLabel?: ReactNode;
  active?: boolean;
  expanded?: boolean;
  expandable?: boolean;
  controlsId?: string;
  ariaLabel?: string;
  onClick?: () => void;
  onHoverChange?: (hovered: boolean) => void;
  trailing?: ReactNode;
  className?: string;
  wrapLabel?: boolean;
}

/** Shared geometry and semantics for conversation process rows. State and
 * content loading remain with the caller; this primitive never infers liveness.
 */
export function ChatDisclosureRow({
  icon,
  label,
  summary,
  summaryRef,
  status,
  statusLabel,
  active = false,
  expanded,
  expandable = expanded !== undefined,
  controlsId,
  ariaLabel,
  onClick,
  onHoverChange,
  trailing,
  className,
  wrapLabel = false,
}: ChatDisclosureRowProps) {
  const statusId = useId();
  const Element = onClick ? 'button' : 'div';

  return (
    <Element
      {...(onClick ? { type: 'button' as const } : {})}
      className={cn(
        'group/disclosure flex min-h-[32px] w-full min-w-0 items-center gap-half rounded-[var(--vk-radius-sm)] py-half text-left text-base leading-relaxed text-low transition-colors [transition-duration:var(--vk-duration-fast)] motion-reduce:transition-none [@media(pointer:coarse)]:min-h-[44px]',
        onClick &&
          'cursor-pointer hover:text-normal focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand',
        className
      )}
      onClick={onClick}
      onMouseEnter={onHoverChange ? () => onHoverChange(true) : undefined}
      onMouseLeave={onHoverChange ? () => onHoverChange(false) : undefined}
      aria-label={ariaLabel}
      aria-expanded={expandable ? (expanded ?? false) : undefined}
      aria-controls={expandable ? controlsId : undefined}
      aria-describedby={statusLabel ? statusId : undefined}
      data-chat-disclosure-row=""
      data-active={active || undefined}
      data-scroll-anchor-target=""
    >
      <span
        className="relative inline-flex size-icon-base shrink-0 items-center justify-center"
        aria-hidden
      >
        {icon}
        {status && status.status !== 'success' && (
          <span className="pointer-events-none absolute -bottom-0.5 -left-0.5 inline-flex">
            <ToolStatusDot status={status} active={active} />
          </span>
        )}
      </span>
      <ActivityText
        active={active}
        ref={summary ? undefined : summaryRef}
        className={cn(
          'min-w-0',
          wrapLabel ? 'whitespace-pre-wrap break-words' : 'truncate',
          summary ? 'max-w-[50%]' : 'flex-1'
        )}
      >
        {label}
      </ActivityText>
      {summary && (
        <span
          ref={summaryRef}
          title={summary}
          className="min-w-0 flex-1 truncate text-low"
        >
          <span className="mr-half" aria-hidden>
            ·
          </span>
          {summary}
        </span>
      )}
      {statusLabel && (
        <span id={statusId} className="max-w-[42%] shrink-0 truncate text-sm">
          {statusLabel}
        </span>
      )}
      {trailing}
      {expandable && (
        <CaretDownIcon
          aria-hidden
          className={cn(
            'size-icon-sm shrink-0 transition-transform [transition-duration:var(--vk-duration-fast)] motion-reduce:transition-none',
            !expanded && '-rotate-90'
          )}
        />
      )}
    </Element>
  );
}

interface ChatDisclosureProps
  extends Omit<ChatDisclosureRowProps, 'onClick' | 'controlsId'> {
  expanded: boolean;
  onToggle: () => void;
  children: ReactNode;
  bodyClassName?: string;
}

export function ChatDisclosureBody({
  id,
  className,
  children,
}: {
  id?: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      id={id}
      className={cn(
        'ml-[1.75rem] mt-half min-w-0 space-y-base border-l border-border pl-base text-base leading-relaxed text-low',
        className
      )}
    >
      {children}
    </div>
  );
}

/** Closed disclosures do not mount their body, parse Markdown or fetch output. */
export function ChatDisclosure({
  expanded,
  onToggle,
  children,
  bodyClassName,
  className,
  ...row
}: ChatDisclosureProps) {
  const contentId = useId();
  return (
    <div className={cn('min-w-0', className)} data-chat-disclosure="">
      <ChatDisclosureRow
        {...row}
        expanded={expanded}
        controlsId={contentId}
        onClick={onToggle}
      />
      {expanded && (
        <ChatDisclosureBody id={contentId} className={bodyClassName}>
          {children}
        </ChatDisclosureBody>
      )}
    </div>
  );
}
