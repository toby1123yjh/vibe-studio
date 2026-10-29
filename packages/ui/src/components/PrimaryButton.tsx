import { SpinnerIcon, type Icon } from '@phosphor-icons/react';
import type { ButtonHTMLAttributes, ReactNode } from 'react';
import { cn } from '../lib/cn';

interface PrimaryButtonProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'value' | 'onClick'> {
  variant?: 'default' | 'secondary' | 'tertiary';
  actionIcon?: Icon | 'spinner';
  value?: string;
  onClick?: () => void;
  disabled?: boolean;
  children?: ReactNode;
  className?: string;
}

export function PrimaryButton({
  variant = 'default',
  actionIcon: ActionIcon,
  value,
  onClick,
  disabled,
  children,
  className,
  ...props
}: PrimaryButtonProps) {
  const variantStyles = disabled
    ? 'cursor-not-allowed bg-panel'
    : variant === 'default'
      ? 'bg-brand hover:bg-brand-hover text-on-brand'
      : variant === 'secondary'
        ? 'bg-brand-secondary hover:bg-brand-hover text-on-brand'
        : 'bg-panel hover:bg-secondary text-normal';

  return (
    <button
      className={cn(
        'rounded-sm px-base py-half text-cta min-h-cta flex gap-half items-center',
        variantStyles,
        className
      )}
      onClick={onClick}
      disabled={disabled}
      {...props}
    >
      {value}
      {children}
      {ActionIcon ? (
        <span
          className="inline-flex shrink-0 items-center justify-center"
          data-button-icon="true"
          aria-hidden="true"
        >
          {ActionIcon === 'spinner' ? (
            <SpinnerIcon className="size-icon-sm animate-spin" weight="bold" />
          ) : (
            <ActionIcon className="size-icon-xs" weight="bold" />
          )}
        </span>
      ) : null}
    </button>
  );
}
