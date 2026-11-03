import * as React from 'react';
import { Slot } from '@radix-ui/react-slot';
import { cva, type VariantProps } from 'class-variance-authority';
import { LoaderCircle } from 'lucide-react';
import { twMerge } from 'tailwind-merge';

import { cn } from '../lib/cn';

const buttonVariants = cva(
  cn(
    'relative inline-flex max-w-full cursor-pointer touch-manipulation items-center justify-center gap-[var(--vk-button-gap)]',
    'rounded-[var(--vk-button-radius)] border text-center font-medium',
    'whitespace-normal break-words text-[length:var(--vk-font-size-sm)] leading-[var(--vk-line-height-sm)]',
    'transition-[background-color,border-color,color,box-shadow] [transition-duration:var(--vk-duration-fast)] [transition-timing-function:var(--vk-ease-standard)]',
    '[@media(pointer:coarse)]:min-h-11 [@media(pointer:coarse)]:min-w-11 motion-reduce:transition-none',
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--vk-focus-ring)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--vk-surface-primary)]',
    'data-[selected=true]:ring-2 data-[selected=true]:ring-[var(--vk-selection-ring)] data-[selected=true]:ring-offset-1',
    'disabled:pointer-events-none disabled:cursor-not-allowed disabled:border-[var(--vk-button-disabled-border)] disabled:bg-[var(--vk-button-disabled-surface)] disabled:text-[var(--vk-button-disabled-text)]',
    'data-[disabled]:pointer-events-none data-[disabled]:cursor-not-allowed data-[disabled]:border-[var(--vk-button-disabled-border)] data-[disabled]:bg-[var(--vk-button-disabled-surface)] data-[disabled]:text-[var(--vk-button-disabled-text)]'
  ),
  {
    variants: {
      variant: {
        default:
          'border-transparent bg-[var(--vk-button-primary-surface)] text-[var(--vk-button-primary-text)] hover:bg-[var(--vk-button-primary-surface-hover)] active:bg-[var(--vk-button-primary-surface-active)]',
        destructive:
          'border-[var(--vk-status-error)] bg-transparent text-[var(--vk-button-danger-text)] hover:bg-[var(--vk-button-danger-surface-hover)] active:bg-[var(--vk-status-error-subtle)]',
        outline:
          'border-[var(--vk-border-interactive)] bg-transparent text-[var(--vk-text-high)] hover:bg-[var(--vk-button-secondary-surface-hover)] active:bg-[var(--vk-button-secondary-surface-active)]',
        secondary:
          'border-[var(--vk-border-subtle)] bg-[var(--vk-button-secondary-surface)] text-[var(--vk-button-secondary-text)] hover:bg-[var(--vk-button-secondary-surface-hover)] active:bg-[var(--vk-button-secondary-surface-active)]',
        ghost:
          'border-transparent bg-transparent text-[var(--vk-text-normal)] hover:bg-[var(--vk-button-secondary-surface-hover)] hover:text-[var(--vk-text-high)] active:bg-[var(--vk-button-secondary-surface-active)]',
        link: 'border-transparent bg-transparent text-[var(--vk-button-link-text)] underline-offset-4 hover:underline active:text-[var(--vk-brand-hover)]',
        icon: 'border-transparent bg-transparent text-[var(--vk-text-low)] hover:bg-[var(--vk-button-secondary-surface-hover)] hover:text-[var(--vk-text-high)] active:bg-[var(--vk-button-secondary-surface-active)]',
      },
      size: {
        default:
          'min-h-[var(--vk-button-height)] px-[var(--vk-button-padding-inline)] py-[var(--vk-space-1)]',
        xs: 'min-h-8 px-[var(--vk-space-2)] py-[var(--vk-space-1)] text-[length:var(--vk-font-size-2xs)] leading-[var(--vk-line-height-2xs)]',
        sm: 'min-h-8 px-[var(--vk-space-3)] py-[var(--vk-space-1)]',
        lg: 'min-h-[var(--vk-button-height-emphasis)] px-[var(--vk-space-4)] py-[var(--vk-space-2)]',
        icon: 'size-8 shrink-0 p-0',
      },
    },
    compoundVariants: [{ variant: 'icon', class: 'p-0' }],
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  }
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
  loading?: boolean;
  loadingLabel?: string;
  selected?: boolean;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  (
    {
      children,
      className,
      variant,
      size,
      asChild = false,
      disabled = false,
      loading = false,
      loadingLabel,
      selected = false,
      onClickCapture,
      onKeyDownCapture,
      ...props
    },
    ref
  ) => {
    const Comp = asChild ? Slot : 'button';
    const isDisabled = disabled || loading;
    const renderedChildren =
      asChild && isDisabled && React.isValidElement(children)
        ? React.cloneElement(
            children as React.ReactElement<React.HTMLAttributes<HTMLElement>>,
            {
              // Radix Slot composes child handlers before slot handlers. Put
              // the disabled guard on the child as well so a child-owned
              // capture handler cannot run before the Button guard.
              onClickCapture: (event) => {
                event.preventDefault();
                event.stopPropagation();
              },
              onKeyDownCapture: (event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault();
                  event.stopPropagation();
                }
              },
            }
          )
        : children;

    return (
      <Comp
        ref={ref}
        className={twMerge(buttonVariants({ variant, size, className }))}
        disabled={asChild ? undefined : isDisabled}
        aria-busy={loading || undefined}
        aria-disabled={asChild && isDisabled ? true : undefined}
        data-disabled={isDisabled ? '' : undefined}
        data-loading={loading || undefined}
        data-selected={selected || undefined}
        onClickCapture={(event) => {
          if (asChild && isDisabled) {
            event.preventDefault();
            event.stopPropagation();
            return;
          }
          onClickCapture?.(event);
        }}
        onKeyDownCapture={(event) => {
          if (
            asChild &&
            isDisabled &&
            (event.key === 'Enter' || event.key === ' ')
          ) {
            event.preventDefault();
            event.stopPropagation();
            return;
          }
          onKeyDownCapture?.(event);
        }}
        {...props}
      >
        {asChild ? (
          renderedChildren
        ) : (
          <>
            {loading && (
              <LoaderCircle
                aria-hidden="true"
                className="absolute inset-0 m-auto size-4 animate-spin motion-reduce:animate-none"
              />
            )}
            <span
              aria-hidden={loading && loadingLabel ? true : undefined}
              className={cn(
                'inline-flex min-w-0 items-center justify-center gap-[var(--vk-button-gap)]',
                // Keep the label mounted and sized while the spinner replaces
                // it visually, so pending actions do not move adjacent controls.
                loading && 'opacity-0'
              )}
            >
              {children}
            </span>
            {loading && loadingLabel && (
              <span className="sr-only">{loadingLabel}</span>
            )}
          </>
        )}
      </Comp>
    );
  }
);
Button.displayName = 'Button';

export { Button, buttonVariants };
