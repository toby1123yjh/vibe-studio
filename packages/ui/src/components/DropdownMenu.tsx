import * as React from 'react';
import * as DropdownMenuPrimitive from '@radix-ui/react-dropdown-menu';
import { Check, ChevronRight, Circle } from 'lucide-react';

import { cn } from '../lib/cn';

const DropdownMenu = DropdownMenuPrimitive.Root;
const DropdownMenuTrigger = DropdownMenuPrimitive.Trigger;
const DropdownMenuGroup = DropdownMenuPrimitive.Group;
const DropdownMenuPortal = DropdownMenuPrimitive.Portal;
const DropdownMenuSub = DropdownMenuPrimitive.Sub;
const DropdownMenuRadioGroup = DropdownMenuPrimitive.RadioGroup;

const menuItemClassName = cn(
  'relative flex min-h-8 min-w-0 cursor-default select-none items-center gap-[var(--vk-space-2)]',
  'rounded-[var(--vk-radius-sm)] px-[var(--vk-space-2)] py-[var(--vk-space-1)] outline-none',
  'whitespace-normal break-words text-[length:var(--vk-font-size-sm)] leading-[var(--vk-line-height-sm)] text-[var(--vk-menu-text)]',
  'transition-colors [transition-duration:var(--vk-duration-fast)] [transition-timing-function:var(--vk-ease-standard)]',
  '[@media(pointer:coarse)]:min-h-11 motion-reduce:transition-none',
  'focus:bg-[var(--vk-menu-item-hover)] focus:text-[var(--vk-text-high)]',
  'data-[highlighted]:bg-[var(--vk-menu-item-hover)] data-[highlighted]:text-[var(--vk-text-high)]',
  'data-[selected=true]:bg-[var(--vk-menu-item-selected)] data-[state=checked]:bg-[var(--vk-menu-item-selected)]',
  'data-[disabled]:pointer-events-none data-[disabled]:cursor-not-allowed data-[disabled]:text-[var(--vk-text-disabled)]',
  '[&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0'
);

const menuContentClassName = cn(
  'z-[var(--vk-z-popover)] min-w-40 overflow-x-hidden overflow-y-auto',
  'rounded-[var(--vk-menu-radius)] border border-[var(--vk-menu-border)] bg-[var(--vk-menu-surface)]',
  'p-[var(--vk-space-1)] text-[var(--vk-menu-text)] shadow-[var(--vk-menu-shadow)]',
  'data-[state=open]:animate-in data-[state=closed]:animate-out',
  'data-[state=open]:[animation-duration:var(--vk-duration-normal)] data-[state=closed]:[animation-duration:var(--vk-duration-fast)] motion-reduce:animate-none',
  'data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0',
  'data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95',
  'data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2',
  'data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2'
);

const DropdownMenuSubTrigger = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.SubTrigger>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.SubTrigger> & {
    inset?: boolean;
  }
>(({ className, inset, children, ...props }, ref) => (
  <DropdownMenuPrimitive.SubTrigger
    ref={ref}
    className={cn(
      menuItemClassName,
      'data-[state=open]:bg-[var(--vk-menu-item-active)]',
      inset && 'pl-[var(--vk-space-8)]',
      className
    )}
    {...props}
  >
    <span className="min-w-0 flex-1">{children}</span>
    <ChevronRight aria-hidden="true" className="ml-auto" />
  </DropdownMenuPrimitive.SubTrigger>
));
DropdownMenuSubTrigger.displayName =
  DropdownMenuPrimitive.SubTrigger.displayName;

const DropdownMenuSubContent = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.SubContent>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.SubContent>
>(({ className, ...props }, ref) => (
  <DropdownMenuPrimitive.Portal>
    <DropdownMenuPrimitive.SubContent
      ref={ref}
      className={cn(
        menuContentClassName,
        'origin-[--radix-dropdown-menu-content-transform-origin]',
        className
      )}
      {...props}
    />
  </DropdownMenuPrimitive.Portal>
));
DropdownMenuSubContent.displayName =
  DropdownMenuPrimitive.SubContent.displayName;

const DropdownMenuContent = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Content>
>(({ className, sideOffset = 4, collisionPadding = 8, ...props }, ref) => (
  <DropdownMenuPrimitive.Portal>
    <DropdownMenuPrimitive.Content
      ref={ref}
      sideOffset={sideOffset}
      collisionPadding={collisionPadding}
      className={cn(
        menuContentClassName,
        'max-h-[var(--radix-dropdown-menu-content-available-height)] origin-[--radix-dropdown-menu-content-transform-origin]',
        className
      )}
      {...props}
    />
  </DropdownMenuPrimitive.Portal>
));
DropdownMenuContent.displayName = DropdownMenuPrimitive.Content.displayName;

type DropdownMenuItemProps = React.ComponentPropsWithoutRef<
  typeof DropdownMenuPrimitive.Item
> & {
  inset?: boolean;
  selected?: boolean;
  variant?: 'default' | 'danger';
};

const DropdownMenuItem = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Item>,
  DropdownMenuItemProps
>(
  (
    { className, inset, selected = false, variant = 'default', ...props },
    ref
  ) => (
    <DropdownMenuPrimitive.Item
      ref={ref}
      data-selected={selected || undefined}
      className={cn(
        menuItemClassName,
        inset && 'pl-[var(--vk-space-8)]',
        variant === 'danger' &&
          'text-[var(--vk-menu-item-danger)] focus:text-[var(--vk-menu-item-danger)]',
        className
      )}
      {...props}
    />
  )
);
DropdownMenuItem.displayName = DropdownMenuPrimitive.Item.displayName;

const DropdownMenuCheckboxItem = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.CheckboxItem>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.CheckboxItem>
>(({ className, children, checked, ...props }, ref) => (
  <DropdownMenuPrimitive.CheckboxItem
    ref={ref}
    className={cn(menuItemClassName, 'pl-[var(--vk-space-8)]', className)}
    checked={checked}
    {...props}
  >
    <span className="absolute left-[var(--vk-space-2)] flex size-4 items-center justify-center">
      <DropdownMenuPrimitive.ItemIndicator>
        <Check aria-hidden="true" className="size-4" />
      </DropdownMenuPrimitive.ItemIndicator>
    </span>
    <span className="min-w-0 flex-1">{children}</span>
  </DropdownMenuPrimitive.CheckboxItem>
));
DropdownMenuCheckboxItem.displayName =
  DropdownMenuPrimitive.CheckboxItem.displayName;

const DropdownMenuRadioItem = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.RadioItem>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.RadioItem>
>(({ className, children, ...props }, ref) => (
  <DropdownMenuPrimitive.RadioItem
    ref={ref}
    className={cn(menuItemClassName, 'pl-[var(--vk-space-8)]', className)}
    {...props}
  >
    <span className="absolute left-[var(--vk-space-2)] flex size-4 items-center justify-center">
      <DropdownMenuPrimitive.ItemIndicator>
        <Circle aria-hidden="true" className="size-2 fill-current" />
      </DropdownMenuPrimitive.ItemIndicator>
    </span>
    <span className="min-w-0 flex-1">{children}</span>
  </DropdownMenuPrimitive.RadioItem>
));
DropdownMenuRadioItem.displayName = DropdownMenuPrimitive.RadioItem.displayName;

const DropdownMenuLabel = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Label>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Label> & {
    inset?: boolean;
  }
>(({ className, inset, ...props }, ref) => (
  <DropdownMenuPrimitive.Label
    ref={ref}
    className={cn(
      'px-[var(--vk-space-2)] py-[var(--vk-space-1)] text-[length:var(--vk-font-size-2xs)] font-medium leading-[var(--vk-line-height-2xs)] text-[var(--vk-menu-text-muted)]',
      inset && 'pl-[var(--vk-space-8)]',
      className
    )}
    {...props}
  />
));
DropdownMenuLabel.displayName = DropdownMenuPrimitive.Label.displayName;

const DropdownMenuSeparator = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Separator>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Separator>
>(({ className, ...props }, ref) => (
  <DropdownMenuPrimitive.Separator
    ref={ref}
    className={cn(
      '-mx-[var(--vk-space-1)] my-[var(--vk-space-1)] h-px bg-[var(--vk-menu-separator)]',
      className
    )}
    {...props}
  />
));
DropdownMenuSeparator.displayName = DropdownMenuPrimitive.Separator.displayName;

const DropdownMenuShortcut = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLSpanElement>) => (
  <span
    className={cn(
      'ml-auto shrink-0 pl-[var(--vk-space-4)] font-mono text-[length:var(--vk-font-size-2xs)] leading-[var(--vk-line-height-2xs)] text-[var(--vk-menu-text-muted)]',
      className
    )}
    {...props}
  />
);
DropdownMenuShortcut.displayName = 'DropdownMenuShortcut';

export {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuCheckboxItem,
  DropdownMenuRadioItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuGroup,
  DropdownMenuPortal,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuRadioGroup,
};
