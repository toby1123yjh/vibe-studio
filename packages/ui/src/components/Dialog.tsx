import * as React from 'react';
import * as DialogPrimitive from '@radix-ui/react-dialog';
import { X } from 'lucide-react';

import { cn } from '../lib/cn';

const Dialog = DialogPrimitive.Root;
const DialogTrigger = DialogPrimitive.Trigger;
const DialogClose = DialogPrimitive.Close;

function DialogPortal({
  children,
  ...props
}: DialogPrimitive.DialogPortalProps) {
  return <DialogPrimitive.Portal {...props}>{children}</DialogPrimitive.Portal>;
}

const DialogOverlay = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Overlay
    ref={ref}
    data-tauri-drag-region
    className={cn(
      'fixed inset-0 z-[var(--vk-z-overlay)] bg-[var(--vk-dialog-scrim)] backdrop-blur-[var(--vk-overlay-backdrop-blur)]',
      'vk-dialog-overlay-motion',
      className
    )}
    {...props}
  />
));
DialogOverlay.displayName = DialogPrimitive.Overlay.displayName;

type DialogContentProps = React.ComponentPropsWithoutRef<
  typeof DialogPrimitive.Content
> & {
  hideCloseButton?: boolean;
  closeLabel?: string;
};

const DialogContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  DialogContentProps
>(
  (
    {
      className,
      children,
      hideCloseButton = false,
      closeLabel = 'Close',
      ...props
    },
    ref
  ) => (
    <DialogPortal>
      <DialogOverlay />
      <DialogPrimitive.Content
        ref={ref}
        className={cn(
          'fixed left-1/2 top-1/2 z-[var(--vk-z-dialog)] max-h-[min(85vh,720px)] w-[calc(100%-2rem)] max-w-lg',
          '-translate-x-1/2 -translate-y-1/2 vk-dialog-centered overflow-y-auto',
          'rounded-[var(--vk-dialog-radius)] border border-[var(--vk-dialog-border)] bg-[var(--vk-dialog-surface)] text-[var(--vk-text-normal)] shadow-[var(--vk-dialog-shadow)]',
          'break-words outline-none',
          'vk-dialog-content-motion',
          className
        )}
        {...props}
      >
        {children}
        {!hideCloseButton && (
          <DialogPrimitive.Close
            aria-label={closeLabel}
            className={cn(
              'absolute right-[var(--vk-space-3)] top-[var(--vk-space-3)] inline-flex size-8 items-center justify-center',
              'rounded-[var(--vk-radius-sm)] text-[var(--vk-text-low)] transition-colors [transition-duration:var(--vk-duration-fast)]',
              'hover:bg-[var(--vk-dialog-close-hover)] hover:text-[var(--vk-text-high)] active:bg-[var(--vk-surface-selection)]',
              'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--vk-focus-ring)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--vk-dialog-surface)]',
              'disabled:pointer-events-none disabled:text-[var(--vk-text-disabled)]'
            )}
          >
            <X aria-hidden="true" className="size-4" />
            <span className="sr-only">{closeLabel}</span>
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Content>
    </DialogPortal>
  )
);
DialogContent.displayName = DialogPrimitive.Content.displayName;

const DialogHeader = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      'flex min-w-0 flex-col gap-[var(--vk-space-1)] pr-[var(--vk-space-8)] text-left',
      className
    )}
    {...props}
  />
);
DialogHeader.displayName = 'DialogHeader';

const DialogFooter = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      'flex flex-col-reverse gap-[var(--vk-space-2)] sm:flex-row sm:flex-wrap sm:justify-end',
      className
    )}
    {...props}
  />
);
DialogFooter.displayName = 'DialogFooter';

const DialogTitle = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Title>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Title>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Title
    ref={ref}
    className={cn(
      'min-w-0 break-words text-[length:var(--vk-font-size-md)] font-semibold leading-[var(--vk-line-height-md)] text-[var(--vk-text-high)]',
      className
    )}
    {...props}
  />
));
DialogTitle.displayName = DialogPrimitive.Title.displayName;

const DialogDescription = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Description>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Description>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Description
    ref={ref}
    className={cn(
      'min-w-0 break-words text-[length:var(--vk-font-size-sm)] leading-[var(--vk-line-height-sm)] text-[var(--vk-text-normal)]',
      className
    )}
    {...props}
  />
));
DialogDescription.displayName = DialogPrimitive.Description.displayName;

export {
  Dialog,
  DialogPortal,
  DialogOverlay,
  DialogTrigger,
  DialogClose,
  DialogContent,
  DialogHeader,
  DialogFooter,
  DialogTitle,
  DialogDescription,
};
