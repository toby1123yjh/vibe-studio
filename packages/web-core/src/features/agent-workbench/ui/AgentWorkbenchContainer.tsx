import {
  useCallback,
  useEffect,
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
  AGENT_WORKBENCH_INSPECTOR_MAX_WIDTH,
  AGENT_WORKBENCH_INSPECTOR_MIN_WIDTH,
  DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES,
  normalizeAgentWorkbenchInspectorPreferences,
} from '../model';
import { AgentWorkbenchInspectorVisibilityProvider } from './AgentWorkbenchInspectorVisibility';
import './agent-workbench.css';

const WIDTH_STORAGE_KEY = 'vk-agent-workbench-inspector-width';
const INSPECTOR_WIDTH_STEP = 16;

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
  onInspectorVisibleChange: (visible: boolean) => void;
}

export function AgentWorkbenchContainer({
  title,
  subtitle,
  conversation,
  inspector,
  inspectorVisible,
  onInspectorVisibleChange,
}: AgentWorkbenchContainerProps) {
  const { t } = useTranslation('common');
  const rootRef = useRef<HTMLDivElement>(null);
  const openButtonRef = useRef<HTMLButtonElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const previousVisibleRef = useRef(inspectorVisible);
  const [inspectorWidth, setInspectorWidth] = useState(initialInspectorWidth);

  const updateInspectorWidth = useCallback((width: number) => {
    const normalizedWidth = normalizeAgentWorkbenchInspectorPreferences({
      width,
    }).width;
    setInspectorWidth(normalizedWidth);
    try {
      window.localStorage.setItem(WIDTH_STORAGE_KEY, String(normalizedWidth));
    } catch {
      // Preference persistence is best effort.
    }
  }, []);

  useEffect(() => {
    if (previousVisibleRef.current === inspectorVisible) return;
    previousVisibleRef.current = inspectorVisible;
    requestAnimationFrame(() => {
      (inspectorVisible ? closeButtonRef : openButtonRef).current?.focus();
    });
  }, [inspectorVisible]);

  const startResize = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      event.preventDefault();
      const onMove = (moveEvent: PointerEvent) => {
        const root = rootRef.current;
        if (!root) return;
        const next = normalizeAgentWorkbenchInspectorPreferences({
          width: root.getBoundingClientRect().right - moveEvent.clientX,
        }).width;
        setInspectorWidth(next);
      };
      const onEnd = () => {
        window.removeEventListener('pointermove', onMove);
        window.removeEventListener('pointerup', onEnd);
        setInspectorWidth((width) => {
          try {
            window.localStorage.setItem(WIDTH_STORAGE_KEY, String(width));
          } catch {
            // Preference persistence is best effort.
          }
          return width;
        });
      };
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', onEnd, { once: true });
    },
    []
  );

  const resizeWithKeyboard = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      let nextWidth: number | null = null;
      switch (event.key) {
        case 'ArrowLeft':
          nextWidth = inspectorWidth + INSPECTOR_WIDTH_STEP;
          break;
        case 'ArrowRight':
          nextWidth = inspectorWidth - INSPECTOR_WIDTH_STEP;
          break;
        case 'Home':
          nextWidth = AGENT_WORKBENCH_INSPECTOR_MIN_WIDTH;
          break;
        case 'End':
          nextWidth = AGENT_WORKBENCH_INSPECTOR_MAX_WIDTH;
          break;
        default:
          return;
      }
      event.preventDefault();
      updateInspectorWidth(nextWidth);
    },
    [inspectorWidth, updateInspectorWidth]
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
              <p className="truncate text-xs text-muted-foreground">
                {subtitle}
              </p>
            )}
          </div>
          {!inspectorVisible && (
            <button
              ref={openButtonRef}
              type="button"
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
        className={cn(
          'vk-agent-workbench__inspector relative shrink-0 overflow-hidden transition-[width] duration-150 motion-reduce:transition-none',
          !inspectorVisible && 'pointer-events-none invisible'
        )}
        style={{ width: inspectorVisible ? inspectorWidth : 0 }}
        data-visible={inspectorVisible}
        aria-hidden={!inspectorVisible}
      >
        <div
          role="separator"
          aria-label={t('agentWorkbench.inspector.resize', {
            defaultValue: 'Resize Inspector',
          })}
          aria-orientation="vertical"
          aria-valuemin={AGENT_WORKBENCH_INSPECTOR_MIN_WIDTH}
          aria-valuemax={AGENT_WORKBENCH_INSPECTOR_MAX_WIDTH}
          aria-valuenow={inspectorWidth}
          tabIndex={inspectorVisible ? 0 : -1}
          className="vk-agent-workbench__separator absolute inset-y-0 left-0 z-10 w-1 cursor-col-resize bg-transparent hover:bg-brand/50 focus-visible:bg-brand focus-visible:outline-none"
          onPointerDown={startResize}
          onKeyDown={resizeWithKeyboard}
        />
        <button
          ref={closeButtonRef}
          type="button"
          aria-label={t('agentWorkbench.inspector.close', {
            defaultValue: 'Close Inspector',
          })}
          className="absolute right-2 top-2 z-20 inline-flex size-[44px] items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
          onClick={() => onInspectorVisibleChange(false)}
        >
          <XIcon aria-hidden size={16} />
        </button>
        <div className="h-full min-h-0">
          <AgentWorkbenchInspectorVisibilityProvider visible={inspectorVisible}>
            {inspector}
          </AgentWorkbenchInspectorVisibilityProvider>
        </div>
      </div>
    </div>
  );
}
