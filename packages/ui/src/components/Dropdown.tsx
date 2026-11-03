import * as React from 'react';
import * as DropdownMenuPrimitive from '@radix-ui/react-dropdown-menu';
import {
  CaretDownIcon,
  MagnifyingGlassIcon,
  type Icon,
} from '@phosphor-icons/react';

import { cn } from '../lib/cn';
import { buttonVariants } from './Button';
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem as BaseDropdownMenuItem,
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
} from './DropdownMenu';

interface DropdownMenuTriggerButtonProps
  extends React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Trigger> {
  icon?: Icon;
  label?: string;
  wrapLabel?: boolean;
  showCaret?: boolean;
  size?: 'default' | 'sm';
}

// Some composer triggers share this class directly. Keep that public styling
// contract on the same button foundation as the component itself.
export const dropdownMenuTriggerButtonClassName = buttonVariants({
  variant: 'outline',
  className: 'min-w-0 justify-start',
});

const DropdownMenuTriggerButton = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Trigger>,
  DropdownMenuTriggerButtonProps
>(
  (
    {
      className,
      icon: IconComponent,
      label,
      wrapLabel = false,
      showCaret = true,
      size = 'default',
      children,
      ...props
    },
    ref
  ) => (
    <DropdownMenuPrimitive.Trigger
      ref={ref}
      className={cn(
        dropdownMenuTriggerButtonClassName,
        size === 'sm' && 'px-[var(--vk-space-2)]',
        wrapLabel && 'h-auto max-w-full',
        className
      )}
      {...props}
    >
      {IconComponent && (
        <IconComponent
          aria-hidden="true"
          className="size-4 shrink-0"
          weight="bold"
        />
      )}
      {label && (
        <span
          className={cn(
            'min-w-0 flex-1 text-left',
            wrapLabel
              ? 'whitespace-normal [overflow-wrap:anywhere]'
              : 'truncate'
          )}
        >
          {label}
        </span>
      )}
      {children}
      {showCaret && (
        <CaretDownIcon
          aria-hidden="true"
          className="size-3 shrink-0 text-[var(--vk-text-low)]"
          weight="bold"
        />
      )}
    </DropdownMenuPrimitive.Trigger>
  )
);
DropdownMenuTriggerButton.displayName = 'DropdownMenuTriggerButton';

interface DropdownMenuItemProps
  extends React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Item> {
  icon?: Icon;
  badge?: React.ReactNode;
  variant?: 'default' | 'destructive';
  /** Keep focus in searchable menus while the pointer moves across results. */
  preventFocusOnHover?: boolean;
}

const DropdownMenuItem = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Item>,
  DropdownMenuItemProps
>(
  (
    {
      className,
      icon: IconComponent,
      badge,
      variant = 'default',
      preventFocusOnHover = false,
      onPointerMove,
      onPointerLeave,
      children,
      ...props
    },
    ref
  ) => (
    <BaseDropdownMenuItem
      ref={ref}
      variant={variant === 'destructive' ? 'danger' : 'default'}
      className={cn('cursor-pointer', className)}
      onPointerMove={
        preventFocusOnHover
          ? (event) => {
              event.preventDefault();
              onPointerMove?.(event);
            }
          : onPointerMove
      }
      onPointerLeave={
        preventFocusOnHover
          ? (event) => {
              event.preventDefault();
              onPointerLeave?.(event);
            }
          : onPointerLeave
      }
      {...props}
    >
      {IconComponent && <IconComponent aria-hidden="true" weight="bold" />}
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {badge && (
        <span className="shrink-0 text-right text-[length:var(--vk-font-size-2xs)] text-[var(--vk-menu-text-muted)]">
          {badge}
        </span>
      )}
    </BaseDropdownMenuItem>
  )
);
DropdownMenuItem.displayName = DropdownMenuPrimitive.Item.displayName;

interface DropdownMenuSearchInputProps
  extends Omit<React.InputHTMLAttributes<HTMLInputElement>, 'type'> {
  onValueChange?: (value: string) => void;
}

const DropdownMenuSearchInput = React.forwardRef<
  HTMLInputElement,
  DropdownMenuSearchInputProps
>(({ className, onValueChange, onChange, ...props }, ref) => {
  const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    onChange?.(event);
    onValueChange?.(event.target.value);
  };

  return (
    <div className="flex items-center gap-[var(--vk-space-2)] px-[var(--vk-space-3)] py-[var(--vk-space-2)]">
      <MagnifyingGlassIcon
        aria-hidden="true"
        className="size-4 shrink-0 text-[var(--vk-menu-text-muted)]"
        weight="bold"
      />
      <input
        ref={ref}
        type="text"
        className={cn(
          'min-h-[var(--vk-input-height)] min-w-0 flex-1 rounded-[var(--vk-input-radius)] border-none bg-transparent px-[var(--vk-space-1)] text-[length:var(--vk-font-size-sm)] leading-[var(--vk-line-height-sm)] text-[var(--vk-input-text)] placeholder:text-[var(--vk-input-placeholder)]',
          'outline-none focus-visible:ring-2 focus-visible:ring-[var(--vk-focus-ring)] [@media(pointer:coarse)]:min-h-11',
          className
        )}
        onChange={handleChange}
        autoFocus
        {...props}
      />
    </div>
  );
});
DropdownMenuSearchInput.displayName = 'DropdownMenuSearchInput';

export {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuTriggerButton,
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
  DropdownMenuSearchInput,
};
