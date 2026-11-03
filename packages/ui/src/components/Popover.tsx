import * as PopoverPrimitive from '@radix-ui/react-popover';
import {
  forwardRef,
  type ComponentPropsWithoutRef,
  type ElementRef,
} from 'react';

import { cn } from '../lib/cn';

const Popover = PopoverPrimitive.Root;
const PopoverTrigger = PopoverPrimitive.Trigger;
const PopoverAnchor = PopoverPrimitive.Anchor;
const PopoverClose = PopoverPrimitive.Close;

const PopoverContent = forwardRef<
  ElementRef<typeof PopoverPrimitive.Content>,
  ComponentPropsWithoutRef<typeof PopoverPrimitive.Content>
>(
  (
    {
      className,
      align = 'center',
      sideOffset = 4,
      collisionPadding = 8,
      ...props
    },
    ref
  ) => (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Content
        ref={ref}
        align={align}
        sideOffset={sideOffset}
        collisionPadding={collisionPadding}
        className={cn(
          'z-[var(--vk-z-popover)] max-h-[var(--radix-popover-content-available-height)] w-72 max-w-[calc(100vw-2rem)] overflow-x-hidden overflow-y-auto',
          'rounded-[var(--vk-popover-radius)] border border-[var(--vk-popover-border)] bg-[var(--vk-popover-surface)]',
          'p-[var(--vk-space-3)] text-[length:var(--vk-font-size-sm)] leading-[var(--vk-line-height-sm)] text-[var(--vk-text-normal)] shadow-[var(--vk-popover-shadow)]',
          'break-words outline-none',
          'data-[state=open]:animate-in data-[state=closed]:animate-out',
          'data-[state=open]:[animation-duration:var(--vk-duration-normal)] data-[state=closed]:[animation-duration:var(--vk-duration-fast)] motion-reduce:animate-none',
          'data-[state=open]:fade-in-0 data-[state=closed]:fade-out-0',
          'data-[state=open]:zoom-in-95 data-[state=closed]:zoom-out-95',
          'data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2',
          'data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2',
          'origin-[--radix-popover-content-transform-origin]',
          className
        )}
        {...props}
      />
    </PopoverPrimitive.Portal>
  )
);
PopoverContent.displayName = PopoverPrimitive.Content.displayName;

export { Popover, PopoverTrigger, PopoverContent, PopoverAnchor, PopoverClose };
