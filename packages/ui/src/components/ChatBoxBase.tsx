import { type ReactNode } from 'react';
import { ImageIcon } from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { cn } from '../lib/cn';
import { Toolbar } from './Toolbar';

export enum VisualVariant {
  NORMAL = 'NORMAL',
  FEEDBACK = 'FEEDBACK',
  EDIT = 'EDIT',
  PLAN = 'PLAN',
}

export interface DropzoneProps {
  getRootProps: () => Record<string, unknown>;
  getInputProps: () => Record<string, unknown>;
  isDragActive: boolean;
}

interface ChatBoxBaseProps {
  editor: ReactNode;
  error?: string | null;
  /** Optional context actions beside the agent and model controls. */
  headerRight?: ReactNode;
  /** Agent identity or selector. */
  headerLeft?: ReactNode;
  /** Conditional task/file status and project context. */
  status?: ReactNode;
  context?: ReactNode;
  hint?: ReactNode;
  footerLeft?: ReactNode;
  footerRight: ReactNode;
  modelSelector?: ReactNode;
  banner?: ReactNode;
  visualVariant: VisualVariant;
  dropzone?: DropzoneProps;
  compact?: boolean;
}

/** Shared message, configuration and action layout for both chat entry points. */
export function ChatBoxBase({
  editor,
  error,
  headerRight,
  headerLeft,
  status,
  context,
  hint,
  footerLeft,
  footerRight,
  modelSelector,
  banner,
  visualVariant,
  dropzone,
  compact = false,
}: ChatBoxBaseProps) {
  const { t } = useTranslation(['common', 'tasks']);
  const isDragActive = dropzone?.isDragActive ?? false;

  return (
    <div
      {...(dropzone?.getRootProps() ?? {})}
      data-chat-composer="true"
      className={cn(
        '@container/chat-composer relative flex min-h-0 min-w-0 max-w-full max-h-[min(70cqh,32rem)] flex-col rounded-[var(--vk-radius-md)] border border-border bg-secondary focus-within:border-brand/50',
        compact ? 'w-full' : 'w-chat',
        (visualVariant === VisualVariant.FEEDBACK ||
          visualVariant === VisualVariant.EDIT ||
          visualVariant === VisualVariant.PLAN) &&
          'border-brand bg-brand/10'
      )}
    >
      {dropzone && <input {...dropzone.getInputProps()} />}
      {isDragActive && (
        <div className="absolute inset-0 z-50 flex items-center justify-center rounded-sm border-2 border-dashed border-brand bg-primary/80 backdrop-blur-sm pointer-events-none animate-in fade-in-0 duration-150">
          <div className="text-center">
            <div className="mx-auto mb-2 w-10 h-10 rounded-full bg-brand/10 flex items-center justify-center">
              <ImageIcon className="h-5 w-5 text-brand" />
            </div>
            <p className="text-sm font-medium text-high">
              {t('tasks:dropzone.dropImagesHere')}
            </p>
            <p className="text-xs text-low mt-0.5">
              {t('tasks:dropzone.supportedFormats')}
            </p>
          </div>
        </div>
      )}
      {error && (
        <div
          className="max-h-24 shrink-0 overflow-y-auto rounded-t-[inherit] bg-error/10 border-b px-plusfifty py-base"
          role="alert"
        >
          <p className="text-error text-sm [overflow-wrap:anywhere]">{error}</p>
        </div>
      )}
      <div className="min-h-0 max-h-[35cqh] shrink overflow-y-auto empty:hidden">
        {banner}
      </div>

      <div
        className="flex min-w-0 shrink-0 flex-wrap items-center gap-base px-plusfifty pt-base text-sm text-low empty:hidden"
        data-composer-status="true"
      >
        {status}
      </div>
      <div
        className="flex min-w-0 shrink-0 flex-wrap items-center gap-base px-plusfifty pt-base empty:hidden"
        data-composer-context="true"
      >
        {context}
      </div>
      {/* Scrolling message content never takes the send controls with it. */}
      <div
        className="min-h-8 min-w-0 shrink overflow-x-hidden overflow-y-auto px-plusfifty py-plusfifty [overflow-wrap:anywhere]"
        data-composer-message="true"
      >
        {editor}
      </div>
      <div
        className="grid min-w-0 shrink-0 grid-cols-[minmax(0,1fr)_auto] items-end gap-x-base gap-y-half px-base pb-base"
        data-composer-actions="true"
      >
        <div
          className="flex min-w-0 flex-wrap items-center gap-x-base gap-y-half"
          data-composer-configuration="true"
        >
          <Toolbar className="min-w-0 max-w-full flex-wrap !gap-x-half !gap-y-half empty:hidden">
            {footerLeft}
          </Toolbar>
          {visualVariant === VisualVariant.NORMAL && (
            <>
              <div className="flex min-w-0 max-w-full flex-wrap items-center gap-x-base gap-y-half text-sm empty:hidden">
                {headerLeft}
              </div>
              {modelSelector && (
                <div className="flex min-w-0 max-w-full flex-wrap items-center gap-half [&_button]:max-w-full">
                  {modelSelector}
                </div>
              )}
              <Toolbar className="min-w-0 max-w-full flex-wrap gap-half empty:hidden">
                {headerRight}
              </Toolbar>
            </>
          )}
        </div>
        <div
          className="flex min-w-0 max-w-full flex-wrap justify-end gap-half [&_button]:min-h-8 [&_button]:justify-center [&_button]:whitespace-normal [&_button]:break-words [@media(pointer:coarse)]:[&_button]:min-h-11"
          data-composer-primary-actions="true"
        >
          {footerRight}
        </div>
      </div>
      <div className="min-w-0 shrink-0 px-plusfifty pb-base text-sm text-low empty:hidden">
        {hint}
      </div>
    </div>
  );
}
