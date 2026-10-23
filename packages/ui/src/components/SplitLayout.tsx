import * as React from 'react';

import { cn } from '../lib/cn';

export type SplitLayoutOrientation = 'horizontal' | 'vertical';
export type SplitLayoutSecondaryPlacement = 'start' | 'end';

export interface SplitLayoutProps
  extends Omit<React.HTMLAttributes<HTMLDivElement>, 'children'> {
  primary: React.ReactNode;
  secondary: React.ReactNode;
  secondarySize: number;
  onSecondarySizeChange: (size: number) => void;
  orientation?: SplitLayoutOrientation;
  secondaryPlacement?: SplitLayoutSecondaryPlacement;
  minSecondarySize?: number;
  maxSecondarySize?: number;
  resizeStep?: number;
  resizable?: boolean;
  separatorLabel?: string;
  primaryClassName?: string;
  secondaryClassName?: string;
  separatorClassName?: string;
  secondaryId?: string;
  onResizeStart?: () => void;
  onResizeEnd?: (size: number) => void;
}

interface PointerResizeState {
  pointerId: number;
  startCoordinate: number;
  startSize: number;
  lastSize: number;
}

function clampSize(size: number, minSize: number, maxSize: number) {
  return Math.min(Math.max(size, minSize), maxSize);
}

export const SplitLayout = React.forwardRef<HTMLDivElement, SplitLayoutProps>(
  (
    {
      primary,
      secondary,
      secondarySize,
      onSecondarySizeChange,
      orientation = 'horizontal',
      secondaryPlacement = 'end',
      minSecondarySize = 240,
      maxSecondarySize = 640,
      resizeStep = 16,
      resizable = true,
      separatorLabel = 'Resize secondary panel',
      primaryClassName,
      secondaryClassName,
      separatorClassName,
      secondaryId: providedSecondaryId,
      onResizeStart,
      onResizeEnd,
      className,
      ...props
    },
    ref
  ) => {
    const generatedSecondaryId = React.useId();
    const secondaryId = providedSecondaryId ?? generatedSecondaryId;
    const pointerResizeState = React.useRef<PointerResizeState | null>(null);
    const lowerBound = Math.min(minSecondarySize, maxSecondarySize);
    const upperBound = Math.max(minSecondarySize, maxSecondarySize);
    const boundedSize = clampSize(secondarySize, lowerBound, upperBound);
    const placementFactor = secondaryPlacement === 'start' ? 1 : -1;
    const isHorizontal = orientation === 'horizontal';

    const requestSize = React.useCallback(
      (nextSize: number) => {
        const boundedNextSize = clampSize(nextSize, lowerBound, upperBound);
        if (boundedNextSize !== boundedSize) {
          onSecondarySizeChange(boundedNextSize);
        }
        return boundedNextSize;
      },
      [boundedSize, lowerBound, onSecondarySizeChange, upperBound]
    );

    const handlePointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
      if (!resizable || event.button !== 0) {
        return;
      }

      event.preventDefault();
      event.currentTarget.blur();
      event.currentTarget.setPointerCapture(event.pointerId);
      pointerResizeState.current = {
        pointerId: event.pointerId,
        startCoordinate: isHorizontal ? event.clientX : event.clientY,
        startSize: boundedSize,
        lastSize: boundedSize,
      };
      onResizeStart?.();
    };

    const handlePointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
      const resizeState = pointerResizeState.current;
      if (!resizeState || resizeState.pointerId !== event.pointerId) {
        return;
      }

      const coordinate = isHorizontal ? event.clientX : event.clientY;
      const delta = coordinate - resizeState.startCoordinate;
      resizeState.lastSize = requestSize(
        resizeState.startSize + delta * placementFactor
      );
    };

    const finishPointerResize = (event: React.PointerEvent<HTMLDivElement>) => {
      const resizeState = pointerResizeState.current;
      if (!resizeState || resizeState.pointerId !== event.pointerId) {
        return;
      }

      pointerResizeState.current = null;
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }

      const coordinate = isHorizontal ? event.clientX : event.clientY;
      const delta = coordinate - resizeState.startCoordinate;
      const finalSize = clampSize(
        resizeState.startSize + delta * placementFactor,
        lowerBound,
        upperBound
      );
      resizeState.lastSize = finalSize;
      requestSize(finalSize);
      onResizeEnd?.(finalSize);
    };

    const cancelPointerResize = (event: React.PointerEvent<HTMLDivElement>) => {
      const resizeState = pointerResizeState.current;
      if (!resizeState || resizeState.pointerId !== event.pointerId) {
        return;
      }

      pointerResizeState.current = null;
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
      onResizeEnd?.(resizeState.lastSize);
    };

    const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (!resizable) {
        return;
      }

      if (event.key === 'Home') {
        event.preventDefault();
        requestSize(lowerBound);
        return;
      }

      if (event.key === 'End') {
        event.preventDefault();
        requestSize(upperBound);
        return;
      }

      const decreaseKey = isHorizontal ? 'ArrowLeft' : 'ArrowUp';
      const increaseKey = isHorizontal ? 'ArrowRight' : 'ArrowDown';
      if (event.key !== decreaseKey && event.key !== increaseKey) {
        return;
      }

      event.preventDefault();
      const coordinateDirection = event.key === increaseKey ? 1 : -1;
      requestSize(
        boundedSize + coordinateDirection * placementFactor * resizeStep
      );
    };

    const secondaryStyle: React.CSSProperties = isHorizontal
      ? { width: `${boundedSize}px` }
      : { height: `${boundedSize}px` };

    return (
      <div
        ref={ref}
        data-orientation={orientation}
        data-secondary-placement={secondaryPlacement}
        className={cn(
          'flex min-h-0 min-w-0 overflow-hidden',
          isHorizontal ? 'flex-row' : 'flex-col',
          className
        )}
        {...props}
      >
        <div
          className={cn(
            'min-h-0 min-w-0 flex-1 overflow-hidden',
            secondaryPlacement === 'start' ? 'order-2' : 'order-0',
            primaryClassName
          )}
        >
          {primary}
        </div>

        <div
          role="separator"
          aria-controls={secondaryId}
          aria-disabled={resizable ? undefined : true}
          aria-label={separatorLabel}
          aria-orientation={isHorizontal ? 'vertical' : 'horizontal'}
          aria-valuemax={upperBound}
          aria-valuemin={lowerBound}
          aria-valuenow={boundedSize}
          tabIndex={resizable ? 0 : undefined}
          data-resizable={resizable || undefined}
          className={cn(
            'group relative order-1 shrink-0 touch-none select-none bg-transparent outline-none',
            'after:absolute after:bg-[var(--vk-split-divider)] after:transition-colors after:duration-[var(--vk-duration-fast)]',
            'focus-visible:after:bg-[var(--vk-split-divider-active)]',
            isHorizontal
              ? 'w-[var(--vk-split-divider-size)] cursor-col-resize after:inset-y-0 after:left-1/2 after:w-px after:-translate-x-1/2'
              : 'h-[var(--vk-split-divider-size)] cursor-row-resize after:inset-x-0 after:top-1/2 after:h-px after:-translate-y-1/2',
            !resizable && 'cursor-default',
            separatorClassName
          )}
          onKeyDown={handleKeyDown}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={finishPointerResize}
          onPointerCancel={cancelPointerResize}
          onLostPointerCapture={cancelPointerResize}
        />

        <div
          id={secondaryId}
          data-pane="secondary"
          className={cn(
            'min-h-0 min-w-0 shrink-0 overflow-hidden',
            secondaryPlacement === 'start' ? 'order-0' : 'order-2',
            secondaryClassName
          )}
          style={secondaryStyle}
        >
          {secondary}
        </div>
      </div>
    );
  }
);
SplitLayout.displayName = 'SplitLayout';
