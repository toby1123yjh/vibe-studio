import { WarningCircleIcon } from '@phosphor-icons/react';
import { cn } from '../lib/cn';
import { ChatDisclosureRow } from './ChatDisclosure';

interface ChatErrorMessageProps {
  content: string;
  className?: string;
  expanded?: boolean;
  onToggle?: () => void;
}

export function ChatErrorMessage({
  content,
  className,
  expanded,
  onToggle,
}: ChatErrorMessageProps) {
  return (
    <ChatDisclosureRow
      icon={<WarningCircleIcon className="size-icon-base" />}
      label={content}
      className={cn('text-error hover:text-error', className)}
      onClick={onToggle}
      expanded={onToggle ? Boolean(expanded) : undefined}
      wrapLabel={expanded || !onToggle}
    />
  );
}
