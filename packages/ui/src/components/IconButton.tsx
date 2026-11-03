import { cn } from '../lib/cn';
import type { Icon } from '@phosphor-icons/react';
import { Button } from './Button';

interface IconButtonProps {
  icon: Icon;
  iconClassName?: string;
  onClick?: () => void;
  disabled?: boolean;
  variant?: 'default' | 'tertiary';
  'aria-label': string;
  title?: string;
  className?: string;
}

export function IconButton({
  icon: IconComponent,
  iconClassName,
  onClick,
  disabled,
  variant = 'default',
  'aria-label': ariaLabel,
  title,
  className,
}: IconButtonProps) {
  return (
    <Button
      type="button"
      variant={variant === 'tertiary' ? 'secondary' : 'icon'}
      size="icon"
      className={className}
      onClick={onClick}
      disabled={disabled}
      aria-label={ariaLabel}
      title={title}
    >
      <IconComponent
        aria-hidden="true"
        className={cn('size-4', iconClassName)}
        weight="bold"
      />
    </Button>
  );
}
