'use client';

import * as React from 'react';
import * as SelectPrimitive from '@radix-ui/react-select';
import { Check, ChevronDown, ChevronUp } from 'lucide-react';

import { cn } from '../lib/cn';

const Select = SelectPrimitive.Root;

const SelectGroup = SelectPrimitive.Group;

const SelectValue = SelectPrimitive.Value;

const SelectTrigger = React.forwardRef<
  React.ElementRef<typeof SelectPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof SelectPrimitive.Trigger>
>(({ className, children, ...props }, ref) => (
  <SelectPrimitive.Trigger
    ref={ref}
    className={cn(
      'flex min-h-[var(--vk-input-height)] w-full min-w-0 cursor-pointer items-center justify-between gap-[var(--vk-space-2)] rounded-[var(--vk-input-radius)] border border-[var(--vk-input-border)] bg-[var(--vk-input-surface)] px-[var(--vk-space-3)] py-[var(--vk-space-1)]',
      'text-[length:var(--vk-font-size-sm)] leading-[var(--vk-line-height-sm)] text-[var(--vk-input-text)] data-[placeholder]:text-[var(--vk-input-placeholder)]',
      'transition-[background-color,border-color,box-shadow] [transition-duration:var(--vk-duration-fast)] [transition-timing-function:var(--vk-ease-standard)] hover:border-[var(--vk-input-border-hover)]',
      'focus-visible:border-[var(--vk-input-border-focus)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--vk-focus-ring)] focus-visible:ring-offset-1 focus-visible:ring-offset-[var(--vk-surface-primary)]',
      'aria-[invalid=true]:border-[var(--vk-input-border-invalid)] disabled:cursor-not-allowed disabled:border-[var(--vk-border-disabled)] disabled:bg-[var(--vk-input-disabled-surface)] disabled:text-[var(--vk-input-disabled-text)]',
      '[@media(pointer:coarse)]:min-h-11 motion-reduce:transition-none [&>span]:min-w-0 [&>span]:truncate',
      className
    )}
    {...props}
  >
    {children}
    <SelectPrimitive.Icon asChild>
      <ChevronDown
        aria-hidden="true"
        className="size-4 shrink-0 text-[var(--vk-text-low)]"
      />
    </SelectPrimitive.Icon>
  </SelectPrimitive.Trigger>
));
SelectTrigger.displayName = SelectPrimitive.Trigger.displayName;

const SelectScrollUpButton = React.forwardRef<
  React.ElementRef<typeof SelectPrimitive.ScrollUpButton>,
  React.ComponentPropsWithoutRef<typeof SelectPrimitive.ScrollUpButton>
>(({ className, ...props }, ref) => (
  <SelectPrimitive.ScrollUpButton
    ref={ref}
    className={cn(
      'flex cursor-default items-center justify-center py-[var(--vk-space-1)] text-[var(--vk-menu-text-muted)]',
      className
    )}
    {...props}
  >
    <ChevronUp aria-hidden="true" className="size-4" />
  </SelectPrimitive.ScrollUpButton>
));
SelectScrollUpButton.displayName = SelectPrimitive.ScrollUpButton.displayName;

const SelectScrollDownButton = React.forwardRef<
  React.ElementRef<typeof SelectPrimitive.ScrollDownButton>,
  React.ComponentPropsWithoutRef<typeof SelectPrimitive.ScrollDownButton>
>(({ className, ...props }, ref) => (
  <SelectPrimitive.ScrollDownButton
    ref={ref}
    className={cn(
      'flex cursor-default items-center justify-center py-[var(--vk-space-1)] text-[var(--vk-menu-text-muted)]',
      className
    )}
    {...props}
  >
    <ChevronDown aria-hidden="true" className="size-4" />
  </SelectPrimitive.ScrollDownButton>
));
SelectScrollDownButton.displayName =
  SelectPrimitive.ScrollDownButton.displayName;

const SelectContent = React.forwardRef<
  React.ElementRef<typeof SelectPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof SelectPrimitive.Content>
>(({ className, children, position = 'popper', ...props }, ref) => (
  <SelectPrimitive.Portal>
    <SelectPrimitive.Content
      ref={ref}
      className={cn(
        'relative z-[var(--vk-z-popover)] max-h-[var(--radix-select-content-available-height)] min-w-40 max-w-[calc(100vw-2rem)] overflow-x-hidden overflow-y-auto rounded-[var(--vk-menu-radius)] border border-[var(--vk-menu-border)] bg-[var(--vk-menu-surface)] text-[var(--vk-menu-text)] shadow-[var(--vk-menu-shadow)]',
        'data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 origin-[--radix-select-content-transform-origin]',
        'data-[state=open]:[animation-duration:var(--vk-duration-normal)] data-[state=closed]:[animation-duration:var(--vk-duration-fast)] motion-reduce:animate-none',
        position === 'popper' &&
          'data-[side=bottom]:translate-y-1 data-[side=left]:-translate-x-1 data-[side=right]:translate-x-1 data-[side=top]:-translate-y-1',
        className
      )}
      position={position}
      {...props}
    >
      <SelectScrollUpButton />
      <SelectPrimitive.Viewport
        className={cn(
          'p-[var(--vk-space-1)]',
          position === 'popper' &&
            'h-[var(--radix-select-trigger-height)] w-full min-w-[var(--radix-select-trigger-width)]'
        )}
      >
        {children}
      </SelectPrimitive.Viewport>
      <SelectScrollDownButton />
    </SelectPrimitive.Content>
  </SelectPrimitive.Portal>
));
SelectContent.displayName = SelectPrimitive.Content.displayName;

const SelectLabel = React.forwardRef<
  React.ElementRef<typeof SelectPrimitive.Label>,
  React.ComponentPropsWithoutRef<typeof SelectPrimitive.Label>
>(({ className, ...props }, ref) => (
  <SelectPrimitive.Label
    ref={ref}
    className={cn(
      'py-[var(--vk-space-1)] pl-[var(--vk-space-8)] pr-[var(--vk-space-2)] text-[length:var(--vk-font-size-2xs)] font-medium leading-[var(--vk-line-height-2xs)] text-[var(--vk-menu-text-muted)]',
      className
    )}
    {...props}
  />
));
SelectLabel.displayName = SelectPrimitive.Label.displayName;

const SelectItem = React.forwardRef<
  React.ElementRef<typeof SelectPrimitive.Item>,
  React.ComponentPropsWithoutRef<typeof SelectPrimitive.Item>
>(({ className, children, ...props }, ref) => (
  <SelectPrimitive.Item
    ref={ref}
    className={cn(
      'relative flex min-h-8 w-full min-w-0 cursor-default select-none items-center rounded-[var(--vk-radius-sm)] py-[var(--vk-space-1)] pl-[var(--vk-space-8)] pr-[var(--vk-space-2)] text-[length:var(--vk-font-size-sm)] leading-[var(--vk-line-height-sm)] outline-none',
      'transition-colors [transition-duration:var(--vk-duration-fast)] focus:bg-[var(--vk-menu-item-hover)] focus:text-[var(--vk-text-high)] data-[highlighted]:bg-[var(--vk-menu-item-hover)] data-[highlighted]:text-[var(--vk-text-high)] data-[state=checked]:bg-[var(--vk-menu-item-selected)]',
      'data-[disabled]:pointer-events-none data-[disabled]:cursor-not-allowed data-[disabled]:text-[var(--vk-text-disabled)] [@media(pointer:coarse)]:min-h-11 motion-reduce:transition-none',
      className
    )}
    {...props}
  >
    <span className="absolute left-[var(--vk-space-2)] flex size-4 items-center justify-center">
      <SelectPrimitive.ItemIndicator>
        <Check aria-hidden="true" className="size-4" />
      </SelectPrimitive.ItemIndicator>
    </span>

    <SelectPrimitive.ItemText className="min-w-0 [overflow-wrap:anywhere]">
      {children}
    </SelectPrimitive.ItemText>
  </SelectPrimitive.Item>
));
SelectItem.displayName = SelectPrimitive.Item.displayName;

const SelectSeparator = React.forwardRef<
  React.ElementRef<typeof SelectPrimitive.Separator>,
  React.ComponentPropsWithoutRef<typeof SelectPrimitive.Separator>
>(({ className, ...props }, ref) => (
  <SelectPrimitive.Separator
    ref={ref}
    className={cn(
      '-mx-[var(--vk-space-1)] my-[var(--vk-space-1)] h-px bg-[var(--vk-menu-separator)]',
      className
    )}
    {...props}
  />
));
SelectSeparator.displayName = SelectPrimitive.Separator.displayName;

export {
  Select,
  SelectGroup,
  SelectValue,
  SelectTrigger,
  SelectContent,
  SelectLabel,
  SelectItem,
  SelectSeparator,
  SelectScrollUpButton,
  SelectScrollDownButton,
};
