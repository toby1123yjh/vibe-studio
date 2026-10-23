import * as React from 'react';

import { cn } from '../lib/cn';

export interface FloatingPanelResizeOptions {
  label: string;
  preferenceKey: string;
  defaultWidth: number;
  minWidth: number;
  maxWidth: number;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max);
}

function readPreferredWidth(options: FloatingPanelResizeOptions) {
  try {
    const stored = localStorage.getItem(options.preferenceKey);
    const width = stored?.trim() ? Number(stored) : NaN;
    if (Number.isFinite(width) && width > 0) {
      return clamp(width, options.minWidth, options.maxWidth);
    }
  } catch {
    // Storage can be unavailable in embedded or private browser contexts.
  }
  return options.defaultWidth;
}

interface PointerResize {
  pointerId: number;
  target: HTMLDivElement;
  startX: number;
  startWidth: number;
  lastWidth: number;
  moved: boolean;
  preferenceKey: string;
  restoreDocumentStyles: () => void;
}

/** Right-anchored sizing only. The caller retains its children and positioning. */
export function useFloatingPanelResize(
  panelRef: React.RefObject<HTMLElement | null>,
  options: FloatingPanelResizeOptions | undefined,
  open = true
) {
  const [preferredWidth, setPreferredWidth] = React.useState(() =>
    options ? readPreferredWidth(options) : 0
  );
  const [dragWidth, setDragWidth] = React.useState<number | null>(null);
  const [geometry, setGeometry] = React.useState({
    availableWidth: Infinity,
    mobile: false,
  });
  const pointerRef = React.useRef<PointerResize | null>(null);
  const frameRef = React.useRef<number | null>(null);
  const enabled = open && options !== undefined;
  const preferenceKey = options?.preferenceKey;
  const defaultWidth = options?.defaultWidth;
  const minWidth = options?.minWidth;
  const maxWidth = options?.maxWidth;
  const lowerBound = Math.min(options?.minWidth ?? 0, geometry.availableWidth);
  const upperBound = Math.min(options?.maxWidth ?? 0, geometry.availableWidth);
  const width = geometry.mobile
    ? geometry.availableWidth
    : clamp(dragWidth ?? preferredWidth, lowerBound, upperBound);
  const resizable = enabled && !geometry.mobile && upperBound > lowerBound;
  const currentRef = React.useRef({ options, lowerBound, upperBound, width });
  currentRef.current = { options, lowerBound, upperBound, width };

  const commit = React.useCallback(
    (nextWidth: number, capturedKey?: string) => {
      const currentOptions = currentRef.current.options;
      if (!currentOptions) return;
      const key = capturedKey ?? currentOptions.preferenceKey;
      if (key === currentOptions.preferenceKey) setPreferredWidth(nextWidth);
      try {
        localStorage.setItem(key, String(nextWidth));
      } catch {
        // Keep resizing usable even when persistence is unavailable.
      }
    },
    []
  );

  const finish = React.useCallback(
    (persist: boolean) => {
      const pointer = pointerRef.current;
      if (!pointer) return;
      pointerRef.current = null;
      if (frameRef.current !== null) {
        cancelAnimationFrame(frameRef.current);
        frameRef.current = null;
      }
      if (pointer.target.hasPointerCapture(pointer.pointerId)) {
        pointer.target.releasePointerCapture(pointer.pointerId);
      }
      pointer.restoreDocumentStyles();
      if (persist && pointer.moved) {
        commit(pointer.lastWidth, pointer.preferenceKey);
      }
      setDragWidth(null);
    },
    [commit]
  );

  React.useLayoutEffect(() => {
    const currentOptions = currentRef.current.options;
    if (!enabled || !currentOptions || !panelRef.current) return;
    setPreferredWidth(readPreferredWidth(currentOptions));

    const panel = panelRef.current;
    const parent = panel.offsetParent;
    const measure = () => {
      const styles = getComputedStyle(panel);
      const inset = Math.max(0, parseFloat(styles.right) || 0);
      const parentWidth =
        styles.position !== 'fixed' && parent instanceof HTMLElement
          ? parent.clientWidth
          : window.innerWidth;
      const nextGeometry = {
        availableWidth: Math.max(0, parentWidth - inset * 2),
        mobile: window.matchMedia('(width < 768px)').matches,
      };
      setGeometry((previous) =>
        previous.availableWidth === nextGeometry.availableWidth &&
        previous.mobile === nextGeometry.mobile
          ? previous
          : nextGeometry
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    if (parent instanceof HTMLElement) observer.observe(parent);
    window.addEventListener('resize', measure);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', measure);
      finish(true);
    };
  }, [
    defaultWidth,
    enabled,
    finish,
    maxWidth,
    minWidth,
    panelRef,
    preferenceKey,
  ]);

  React.useEffect(() => {
    if (!resizable) finish(true);
  }, [finish, resizable]);

  const requestDragWidth = (clientX: number) => {
    const pointer = pointerRef.current;
    if (!pointer) return;
    const current = currentRef.current;
    const nextWidth = Math.round(
      clamp(
        pointer.startWidth + pointer.startX - clientX,
        current.lowerBound,
        current.upperBound
      )
    );
    if (nextWidth === pointer.lastWidth) return;
    pointer.lastWidth = nextWidth;
    pointer.moved = true;
    if (frameRef.current === null) {
      frameRef.current = requestAnimationFrame(() => {
        frameRef.current = null;
        if (pointerRef.current) setDragWidth(pointerRef.current.lastWidth);
      });
    }
  };

  const handleProps: React.HTMLAttributes<HTMLDivElement> | null =
    resizable && options
      ? {
          role: 'separator',
          'aria-label': options.label,
          'aria-orientation': 'vertical',
          'aria-valuemin': Math.round(lowerBound),
          'aria-valuemax': Math.round(upperBound),
          'aria-valuenow': Math.round(width),
          tabIndex: 0,
          onPointerDown(event) {
            if (event.button !== 0 || pointerRef.current) return;
            event.preventDefault();
            event.stopPropagation();
            event.currentTarget.blur();
            event.currentTarget.setPointerCapture(event.pointerId);
            const documentStyle = document.documentElement.style;
            const previousCursor = documentStyle.cursor;
            const previousUserSelect = documentStyle.userSelect;
            documentStyle.cursor = 'col-resize';
            documentStyle.userSelect = 'none';
            pointerRef.current = {
              pointerId: event.pointerId,
              target: event.currentTarget,
              startX: event.clientX,
              startWidth: width,
              lastWidth: width,
              moved: false,
              preferenceKey: options.preferenceKey,
              restoreDocumentStyles: () => {
                documentStyle.cursor = previousCursor;
                documentStyle.userSelect = previousUserSelect;
              },
            };
          },
          onPointerMove(event) {
            if (pointerRef.current?.pointerId === event.pointerId) {
              requestDragWidth(event.clientX);
            }
          },
          onPointerUp(event) {
            if (pointerRef.current?.pointerId !== event.pointerId) return;
            requestDragWidth(event.clientX);
            finish(true);
          },
          onPointerCancel(event) {
            if (pointerRef.current?.pointerId === event.pointerId) finish(true);
          },
          onLostPointerCapture(event) {
            if (pointerRef.current?.pointerId === event.pointerId) finish(true);
          },
          onKeyDown(event) {
            let nextWidth: number;
            switch (event.key) {
              case 'ArrowLeft':
                nextWidth = width + (event.shiftKey ? 64 : 16);
                break;
              case 'ArrowRight':
                nextWidth = width - (event.shiftKey ? 64 : 16);
                break;
              case 'Home':
                nextWidth = lowerBound;
                break;
              case 'End':
                nextWidth = upperBound;
                break;
              default:
                return;
            }
            event.preventDefault();
            event.stopPropagation();
            commit(clamp(nextWidth, lowerBound, upperBound));
          },
        }
      : null;

  return {
    style:
      enabled && Number.isFinite(width)
        ? ({ width: `${width}px` } satisfies React.CSSProperties)
        : undefined,
    handleProps,
  };
}

export function FloatingPanelResizeHandle({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      {...props}
      className={cn(
        'absolute inset-y-0 left-0 z-[3] w-3 cursor-col-resize touch-none select-none outline-none',
        'after:absolute after:inset-y-3 after:left-0 after:w-0.5 after:rounded-full after:bg-transparent after:transition-colors motion-reduce:after:transition-none',
        'focus-visible:after:bg-[var(--vk-split-divider-active)]',
        className
      )}
    />
  );
}
