import * as React from 'react';
import { twMerge } from 'tailwind-merge';

import { cn } from '../lib/cn';

export interface InputProps
  extends React.InputHTMLAttributes<HTMLInputElement> {
  onCommandEnter?: (event: React.KeyboardEvent<HTMLInputElement>) => void;
  onCommandShiftEnter?: (event: React.KeyboardEvent<HTMLInputElement>) => void;
  invalid?: boolean;
}

const Input = React.forwardRef<HTMLInputElement, InputProps>(
  (
    {
      className,
      type,
      onKeyDown,
      onCommandEnter,
      onCommandShiftEnter,
      invalid,
      'aria-invalid': ariaInvalid,
      ...props
    },
    ref
  ) => {
    const isInvalid =
      invalid ?? (ariaInvalid === true || ariaInvalid === 'true');

    const handleKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
      if (event.key === 'Escape') {
        event.currentTarget.blur();
      }

      if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
        if (event.metaKey && event.shiftKey) {
          onCommandShiftEnter?.(event);
        } else {
          onCommandEnter?.(event);
        }
      }

      onKeyDown?.(event);
    };

    return (
      <input
        ref={ref}
        type={type}
        onKeyDown={handleKeyDown}
        aria-invalid={isInvalid || undefined}
        data-invalid={isInvalid || undefined}
        className={twMerge(
          cn(
            'flex min-h-[var(--vk-input-height)] w-full min-w-0 rounded-[var(--vk-input-radius)] border',
            'border-[var(--vk-input-border)] bg-[var(--vk-input-surface)] px-[var(--vk-space-3)] py-[var(--vk-space-1)]',
            'text-[length:var(--vk-font-size-sm)] leading-[var(--vk-line-height-sm)] text-[var(--vk-input-text)]',
            'placeholder:text-[var(--vk-input-placeholder)] file:border-0 file:bg-transparent file:text-[length:var(--vk-font-size-sm)] file:font-medium',
            'transition-[background-color,border-color,box-shadow] [transition-duration:var(--vk-duration-fast)] [transition-timing-function:var(--vk-ease-standard)]',
            '[@media(pointer:coarse)]:min-h-11 motion-reduce:transition-none',
            'hover:border-[var(--vk-input-border-hover)]',
            'focus-visible:border-[var(--vk-input-border-focus)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--vk-focus-ring)] focus-visible:ring-offset-1 focus-visible:ring-offset-[var(--vk-surface-primary)]',
            'data-[invalid=true]:border-[var(--vk-input-border-invalid)] data-[invalid=true]:focus-visible:ring-[var(--vk-status-error)]',
            'read-only:cursor-default read-only:bg-[var(--vk-surface-primary)]',
            'disabled:cursor-not-allowed disabled:border-[var(--vk-border-disabled)] disabled:bg-[var(--vk-input-disabled-surface)] disabled:text-[var(--vk-input-disabled-text)] disabled:placeholder:text-[var(--vk-input-disabled-text)]',
            className
          )
        )}
        {...props}
      />
    );
  }
);

Input.displayName = 'Input';

export { Input };
