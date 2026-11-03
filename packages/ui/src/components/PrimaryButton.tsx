import type { Icon } from '@phosphor-icons/react';
import type { ButtonHTMLAttributes, ReactNode } from 'react';
import { Button } from './Button';

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
  return (
    <Button
      variant={
        variant === 'tertiary'
          ? 'ghost'
          : variant === 'secondary'
            ? 'secondary'
            : 'default'
      }
      className={className}
      onClick={onClick}
      disabled={disabled}
      loading={ActionIcon === 'spinner'}
      {...props}
    >
      <span className="min-w-0">
        {value}
        {children}
      </span>
      {ActionIcon && ActionIcon !== 'spinner' ? (
        <span
          className="inline-flex shrink-0 items-center justify-center"
          data-button-icon="true"
          aria-hidden="true"
        >
          <ActionIcon className="size-4" weight="bold" />
        </span>
      ) : null}
    </Button>
  );
}
