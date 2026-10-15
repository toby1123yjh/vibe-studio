import * as React from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';

import { cn } from '../lib/cn';

export interface FloatingPanelProps
  extends Omit<React.HTMLAttributes<HTMLDivElement>, 'aria-modal'> {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  closeLabel?: string;
  hideCloseButton?: boolean;
  autoFocus?: boolean;
  initialFocusRef?: React.RefObject<HTMLElement>;
  restoreFocus?: boolean;
  portal?: boolean;
  portalContainer?: Element | DocumentFragment | null;
  contentClassName?: string;
}

export const FloatingPanel = React.forwardRef<
  HTMLDivElement,
  FloatingPanelProps
>(
  (
    {
      open,
      onOpenChange,
      closeLabel = 'Close panel',
      hideCloseButton = false,
      autoFocus = false,
      initialFocusRef,
      restoreFocus = true,
      portal = true,
      portalContainer,
      contentClassName,
      children,
      className,
      onClick,
      onBlurCapture,
      onFocusCapture,
      onKeyDown,
      onPointerDown,
      ...props
    },
    ref
  ) => {
    const panelRef = React.useRef<HTMLDivElement | null>(null);
    const closeButtonRef = React.useRef<HTMLButtonElement | null>(null);
    const previouslyFocusedRef = React.useRef<HTMLElement | null>(null);
    const focusWithinPanelRef = React.useRef(false);
    const explicitPanelCloseRef = React.useRef(false);
    const wasOpenRef = React.useRef(false);

    const setPanelRef = React.useCallback(
      (node: HTMLDivElement | null) => {
        panelRef.current = node;
        if (typeof ref === 'function') {
          ref(node);
        } else if (ref) {
          ref.current = node;
        }
      },
      [ref]
    );

    React.useLayoutEffect(() => {
      if (typeof document === 'undefined') {
        return;
      }

      if (open && !wasOpenRef.current) {
        previouslyFocusedRef.current =
          document.activeElement instanceof HTMLElement
            ? document.activeElement
            : null;
        const initialFocusTarget =
          initialFocusRef?.current ??
          (autoFocus ? (closeButtonRef.current ?? panelRef.current) : null);
        initialFocusTarget?.focus({ preventScroll: true });
      } else if (!open && wasOpenRef.current) {
        const previousElement = previouslyFocusedRef.current;
        const shouldRestoreFocus =
          restoreFocus &&
          (explicitPanelCloseRef.current || focusWithinPanelRef.current);
        if (shouldRestoreFocus && previousElement?.isConnected) {
          previousElement.focus({ preventScroll: true });
        }
        previouslyFocusedRef.current = null;
        focusWithinPanelRef.current = false;
        explicitPanelCloseRef.current = false;
      }

      wasOpenRef.current = open;
    }, [autoFocus, initialFocusRef, open, restoreFocus]);

    if (!open) {
      return null;
    }

    const panel = (
      <div
        ref={setPanelRef}
        {...props}
        role="dialog"
        tabIndex={-1}
        className={cn(
          'fixed bottom-[var(--vk-floating-panel-offset)] right-[var(--vk-floating-panel-offset)] top-[var(--vk-floating-panel-offset)]',
          'z-[var(--vk-z-floating)] w-[var(--vk-floating-panel-width)] max-w-[calc(100vw-(var(--vk-floating-panel-offset)*2))]',
          'overflow-hidden rounded-[var(--vk-floating-panel-radius)] border border-[var(--vk-floating-panel-border)]',
          'bg-[var(--vk-floating-panel-surface)] text-[var(--vk-text-normal)] shadow-[var(--vk-floating-panel-shadow)]',
          'outline-none focus-visible:ring-2 focus-visible:ring-[var(--vk-focus-ring)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--vk-surface-canvas)]',
          className
        )}
        onClick={(event) => {
          event.stopPropagation();
          onClick?.(event);
        }}
        onBlurCapture={(event) => {
          const nextFocusedElement = event.relatedTarget;
          focusWithinPanelRef.current =
            nextFocusedElement instanceof Node &&
            event.currentTarget.contains(nextFocusedElement);
          onBlurCapture?.(event);
        }}
        onFocusCapture={(event) => {
          focusWithinPanelRef.current = true;
          onFocusCapture?.(event);
        }}
        onKeyDown={(event) => {
          onKeyDown?.(event);
          if (event.defaultPrevented || event.key !== 'Escape') {
            return;
          }

          event.preventDefault();
          event.stopPropagation();
          explicitPanelCloseRef.current = true;
          onOpenChange(false);
        }}
        onPointerDown={(event) => {
          event.stopPropagation();
          onPointerDown?.(event);
        }}
      >
        {!hideCloseButton && (
          <button
            ref={closeButtonRef}
            type="button"
            aria-label={closeLabel}
            className={cn(
              'absolute right-[var(--vk-space-3)] top-[var(--vk-space-3)] z-[2] inline-flex size-8 items-center justify-center',
              'rounded-[var(--vk-radius-sm)] text-[var(--vk-text-low)] transition-colors duration-[var(--vk-duration-fast)]',
              'hover:bg-[var(--vk-surface-hover)] hover:text-[var(--vk-text-high)] active:bg-[var(--vk-surface-selection)]',
              'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--vk-focus-ring)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--vk-floating-panel-surface)]'
            )}
            onClick={() => {
              explicitPanelCloseRef.current = true;
              onOpenChange(false);
            }}
          >
            <X aria-hidden="true" className="size-4" />
            <span className="sr-only">{closeLabel}</span>
          </button>
        )}

        <div
          className={cn(
            'h-full min-h-0 overflow-y-auto overscroll-contain',
            contentClassName
          )}
        >
          {children}
        </div>
      </div>
    );

    if (!portal || typeof document === 'undefined') {
      return panel;
    }

    return createPortal(panel, portalContainer ?? document.body);
  }
);
FloatingPanel.displayName = 'FloatingPanel';

export function FloatingPanelHeader({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        'sticky top-0 z-[1] min-w-0 border-b border-[var(--vk-border-subtle)] bg-[var(--vk-floating-panel-surface)]',
        'px-[var(--vk-space-4)] py-[var(--vk-space-3)] pr-[var(--vk-space-12)]',
        className
      )}
      {...props}
    />
  );
}

export const FloatingPanelTitle = React.forwardRef<
  HTMLHeadingElement,
  React.HTMLAttributes<HTMLHeadingElement>
>(({ className, ...props }, ref) => (
  <h2
    ref={ref}
    className={cn(
      'min-w-0 break-words text-[length:var(--vk-font-size-md)] font-semibold leading-[var(--vk-line-height-md)] text-[var(--vk-text-high)]',
      className
    )}
    {...props}
  />
));
FloatingPanelTitle.displayName = 'FloatingPanelTitle';

export function FloatingPanelDescription({
  className,
  ...props
}: React.HTMLAttributes<HTMLParagraphElement>) {
  return (
    <p
      className={cn(
        'mt-[var(--vk-space-1)] min-w-0 break-words text-[length:var(--vk-font-size-sm)] leading-[var(--vk-line-height-sm)] text-[var(--vk-text-low)]',
        className
      )}
      {...props}
    />
  );
}

export function FloatingPanelBody({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        'min-w-0 break-words px-[var(--vk-space-4)] py-[var(--vk-space-4)]',
        className
      )}
      {...props}
    />
  );
}

export function FloatingPanelFooter({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        'sticky bottom-0 flex min-w-0 flex-wrap justify-end gap-[var(--vk-space-2)]',
        'border-t border-[var(--vk-border-subtle)] bg-[var(--vk-floating-panel-surface)] px-[var(--vk-space-4)] py-[var(--vk-space-3)]',
        className
      )}
      {...props}
    />
  );
}
