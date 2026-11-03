import * as React from 'react';
import { cva, type VariantProps } from 'class-variance-authority';

import { cn } from '../lib/cn';

const badgeVariants = cva(
  cn(
    'inline-flex max-w-full items-center gap-[var(--vk-space-1)] rounded-[var(--vk-badge-radius)] border',
    'px-[var(--vk-space-2)] py-[var(--vk-space-1)]',
    'break-words text-[length:var(--vk-font-size-2xs)] font-medium leading-[var(--vk-line-height-2xs)]',
    'transition-[background-color,border-color,color,box-shadow] [transition-duration:var(--vk-duration-fast)]',
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--vk-focus-ring)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--vk-surface-primary)]',
    'data-[selected=true]:ring-2 data-[selected=true]:ring-[var(--vk-badge-selected-ring)] data-[selected=true]:ring-offset-1',
    'aria-disabled:pointer-events-none aria-disabled:border-[var(--vk-border-disabled)] aria-disabled:bg-[var(--vk-surface-disabled)] aria-disabled:text-[var(--vk-text-disabled)]'
  ),
  {
    variants: {
      variant: {
        default:
          'border-[var(--vk-badge-border)] bg-[var(--vk-badge-surface)] text-[var(--vk-badge-text)] hover:bg-[var(--vk-surface-hover)] active:bg-[var(--vk-surface-selection)]',
        secondary:
          'border-transparent bg-[var(--vk-surface-secondary)] text-[var(--vk-text-normal)] hover:bg-[var(--vk-surface-hover)] active:bg-[var(--vk-surface-selection)]',
        destructive:
          'border-[var(--vk-status-error)] bg-[var(--vk-status-error-subtle)] text-[var(--vk-status-error-text)]',
        outline:
          'border-[var(--vk-border-interactive)] bg-transparent text-[var(--vk-text-normal)]',
        running:
          'border-[var(--vk-status-running)] bg-[var(--vk-status-running-subtle)] text-[var(--vk-status-running-text)]',
        waiting:
          'border-[var(--vk-status-waiting)] bg-[var(--vk-status-waiting-subtle)] text-[var(--vk-status-waiting-text)]',
        success:
          'border-[var(--vk-status-success)] bg-[var(--vk-status-success-subtle)] text-[var(--vk-status-success-text)]',
        error:
          'border-[var(--vk-status-error)] bg-[var(--vk-status-error-subtle)] text-[var(--vk-status-error-text)]',
        cancelled:
          'border-[var(--vk-status-cancelled)] bg-[var(--vk-status-cancelled-subtle)] text-[var(--vk-status-cancelled-text)]',
        workflow:
          'border-[var(--vk-status-workflow)] bg-[var(--vk-status-workflow-subtle)] text-[var(--vk-status-workflow-text)]',
      },
    },
    defaultVariants: {
      variant: 'default',
    },
  }
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLDivElement>,
    VariantProps<typeof badgeVariants> {
  selected?: boolean;
}

function Badge({ className, variant, selected = false, ...props }: BadgeProps) {
  return (
    <div
      data-selected={selected || undefined}
      className={cn(badgeVariants({ variant }), className)}
      {...props}
    />
  );
}

export { Badge, badgeVariants };
