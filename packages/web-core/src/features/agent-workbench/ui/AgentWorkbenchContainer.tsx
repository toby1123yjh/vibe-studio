import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from 'react';
import { SidebarSimpleIcon, XIcon } from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';

import { cn } from '@/shared/lib/utils';
import {
  AGENT_WORKBENCH_CONVERSATION_MIN_WIDTH,
  AGENT_WORKBENCH_INSPECTOR_MAX_WIDTH,
  AGENT_WORKBENCH_INSPECTOR_MIN_WIDTH,
  DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES,
  normalizeAgentWorkbenchInspectorPreferences,
} from '../model';
import { AgentWorkbenchInspectorVisibilityProvider } from './AgentWorkbenchInspectorVisibility';
import './agent-workbench.css';

const WIDTH_STORAGE_KEY = 'vk-agent-workbench-inspector-width';
const INSPECTOR_WIDTH_STEP = 16;

interface InspectorPointerResize {
  pointerId: number;
  target: HTMLDivElement;
  startX: number;
  startWidth: number;
  lastWidth: number;
  moved: boolean;
  restoreDocumentStyles: () => void;
}

function persistInspectorWidth(width: number) {
  try {
    window.localStorage.setItem(WIDTH_STORAGE_KEY, String(width));
  } catch {
    // Preference persistence is best effort.
  }
}

function initialInspectorWidth() {
  if (typeof window === 'undefined') {
    return DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES.width;
  }
  try {
    const saved = window.localStorage.getItem(WIDTH_STORAGE_KEY);
    if (saved === null) {
      return DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES.width;
    }
    return normalizeAgentWorkbenchInspectorPreferences({
      width: Number(saved),
    }).width;
  } catch {
    return DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES.width;
  }
}

export interface AgentWorkbenchContainerProps {
  title: string;
  subtitle?: string;
  conversation: ReactNode;
  inspector: ReactNode;
  inspectorVisible: boolean;
  inspectorAvailable?: boolean;
  onInspectorVisibleChange: (visible: boolean) => void;
}

export function AgentWorkbenchContainer({
  title,
  subtitle,
  conversation,
  inspector,
  inspectorVisible,
  inspectorAvailable = true,
  onInspectorVisibleChange,
}: AgentWorkbenchContainerProps) {
  const { t } = useTranslation('common');
  const inspectorId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const openButtonRef = useRef<HTMLButtonElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const visible = inspectorAvailable && inspectorVisible;
  const previousVisibleRef = useRef(visible);
  const resizeRef = useRef<InspectorPointerResize | null>(null);
  const [inspectorWidth, setInspectorWidth] = useState(initialInspectorWidth);
  const [geometry, setGeometry] = useState({ width: 0, mobile: false });
  const overlay =
    geometry.mobile ||
    geometry.width <
      AGENT_WORKBENCH_INSPECTOR_MIN_WIDTH +
        AGENT_WORKBENCH_CONVERSATION_MIN_WIDTH;
  const maxWidth = Math.min(
    AGENT_WORKBENCH_INSPECTOR_MAX_WIDTH,
    Math.max(
      0,
      geometry.width - (overlay ? 0 : AGENT_WORKBENCH_CONVERSATION_MIN_WIDTH)
    )
  );
  const minWidth = Math.min(AGENT_WORKBENCH_INSPECTOR_MIN_WIDTH, maxWidth);
  const width = Math.min(maxWidth, Math.max(minWidth, inspectorWidth));
  const boundsRef = useRef({ minWidth, maxWidth });
  boundsRef.current = { minWidth, maxWidth };
  const resizable = visible && !geometry.mobile && maxWidth > minWidth;

  const updateInspectorWidth = useCallback((width: number) => {
    const bounds = boundsRef.current;
    const next = Math.min(bounds.maxWidth, Math.max(bounds.minWidth, width));
    setInspectorWidth(next);
    persistInspectorWidth(next);
  }, []);

  const finishResize = useCallback(() => {
    const resize = resizeRef.current;
    if (!resize) return;
    resizeRef.current = null;
    if (resize.target.hasPointerCapture(resize.pointerId)) {
      resize.target.releasePointerCapture(resize.pointerId);
    }
    resize.restoreDocumentStyles();
    if (resize.moved) persistInspectorWidth(resize.lastWidth);
  }, []);

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const measure = () => {
      const next = {
        width: root.clientWidth,
        mobile: window.matchMedia('(width < 768px)').matches,
      };
      setGeometry((current) =>
        current.width === next.width && current.mobile === next.mobile
          ? current
          : next
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    window.addEventListener('resize', measure);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', measure);
      finishResize();
    };
  }, [finishResize]);

  useEffect(() => {
    finishResize();
  }, [finishResize, geometry.width, geometry.mobile, visible]);

  useEffect(() => {
    if (previousVisibleRef.current === visible) return;
    previousVisibleRef.current = visible;
    const frame = requestAnimationFrame(() => {
      (visible ? closeButtonRef : openButtonRef).current?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [visible]);

  const startResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!resizable || event.button !== 0 || resizeRef.current) return;
    event.preventDefault();
    event.currentTarget.focus({ preventScroll: true });
    event.currentTarget.setPointerCapture(event.pointerId);
    const documentStyle = document.documentElement.style;
    const previousCursor = documentStyle.cursor;
    const previousUserSelect = documentStyle.userSelect;
    documentStyle.cursor = 'col-resize';
    documentStyle.userSelect = 'none';
    resizeRef.current = {
      pointerId: event.pointerId,
      target: event.currentTarget,
      startX: event.clientX,
      startWidth: width,
      lastWidth: width,
      moved: false,
      restoreDocumentStyles: () => {
        documentStyle.cursor = previousCursor;
        documentStyle.userSelect = previousUserSelect;
      },
    };
  };

  const moveResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    const resize = resizeRef.current;
    if (!resize || resize.pointerId !== event.pointerId) return;
    const bounds = boundsRef.current;
    const next = Math.min(
      bounds.maxWidth,
      Math.max(
        bounds.minWidth,
        resize.startWidth + resize.startX - event.clientX
      )
    );
    if (next === resize.lastWidth) return;
    resize.lastWidth = next;
    resize.moved = true;
    setInspectorWidth(next);
  };

  const endResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (resizeRef.current?.pointerId !== event.pointerId) return;
    if (event.type === 'pointerup') moveResize(event);
    finishResize();
  };

  const resizeWithKeyboard = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      if (!resizable) return;
      let nextWidth: number | null = null;
      switch (event.key) {
        case 'ArrowLeft':
          nextWidth = width + INSPECTOR_WIDTH_STEP;
          break;
        case 'ArrowRight':
          nextWidth = width - INSPECTOR_WIDTH_STEP;
          break;
        case 'Home':
          nextWidth = minWidth;
          break;
        case 'End':
          nextWidth = maxWidth;
          break;
        default:
          return;
      }
      event.preventDefault();
      updateInspectorWidth(nextWidth);
    },
    [maxWidth, minWidth, resizable, width, updateInspectorWidth]
  );

  return (
    <div
      ref={rootRef}
      className="vk-agent-workbench flex h-full min-h-0 min-w-0"
    >
      <section className="vk-agent-workbench__conversation flex min-w-0 flex-1 flex-col bg-background">
        <header className="flex min-h-12 shrink-0 items-center gap-3 border-b px-4">
          <div className="min-w-0 flex-1">
            <h1 className="truncate text-sm font-medium">{title}</h1>
            {subtitle && (
              <p
                className="truncate text-xs text-muted-foreground"
                title={subtitle}
              >
                {subtitle}
              </p>
            )}
          </div>
          {inspectorAvailable && !visible && (
            <button
              ref={openButtonRef}
              type="button"
              aria-controls={inspectorId}
              aria-expanded={false}
              className="inline-flex min-h-[44px] items-center gap-2 rounded px-2 text-xs text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
              onClick={() => onInspectorVisibleChange(true)}
            >
              <SidebarSimpleIcon aria-hidden size={16} />
              {t('agentWorkbench.inspector.open', {
                defaultValue: 'Inspector',
              })}
            </button>
          )}
        </header>
        <div className="flex min-h-0 flex-1 flex-col">{conversation}</div>
      </section>

      <div
        id={inspectorId}
        className={cn(
          'vk-agent-workbench__inspector relative flex min-h-0 min-w-0 shrink-0 flex-col overflow-hidden bg-secondary',
          visible && 'border-l',
          !visible && 'pointer-events-none invisible'
        )}
        style={{ width: visible ? width : 0 }}
        data-visible={visible}
        data-overlay={overlay}
        aria-hidden={!visible}
      >
        <div
          role="separator"
          aria-label={t('agentWorkbench.inspector.resize', {
            defaultValue: 'Resize Inspector',
          })}
          aria-orientation="vertical"
          aria-controls={inspectorId}
          aria-valuemin={minWidth}
          aria-valuemax={maxWidth}
          aria-valuenow={width}
          aria-disabled={!resizable || undefined}
          tabIndex={resizable ? 0 : -1}
          className="vk-agent-workbench__separator absolute inset-y-0 left-0 z-10 w-1 cursor-col-resize touch-none select-none bg-transparent hover:bg-brand/50 focus-visible:bg-brand focus-visible:outline-none"
          onPointerDown={startResize}
          onPointerMove={moveResize}
          onPointerUp={endResize}
          onPointerCancel={endResize}
          onLostPointerCapture={endResize}
          onKeyDown={resizeWithKeyboard}
        />
        <header className="vk-agent-workbench__inspector-header flex min-h-12 shrink-0 items-center gap-2 border-b pl-4 pr-1">
          <h2 className="min-w-0 flex-1 truncate text-sm font-medium">
            {t('agentWorkbench.inspector.label', { defaultValue: 'Inspector' })}
          </h2>
          <button
            ref={closeButtonRef}
            type="button"
            aria-label={t('agentWorkbench.inspector.close', {
              defaultValue: 'Close Inspector',
            })}
            className="inline-flex size-[44px] shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
            onClick={() => onInspectorVisibleChange(false)}
          >
            <XIcon aria-hidden size={16} />
          </button>
        </header>
        <div className="min-h-0 min-w-0 flex-1">
          <AgentWorkbenchInspectorVisibilityProvider visible={visible}>
            {inspector}
          </AgentWorkbenchInspectorVisibilityProvider>
        </div>
      </div>
    </div>
  );
}
