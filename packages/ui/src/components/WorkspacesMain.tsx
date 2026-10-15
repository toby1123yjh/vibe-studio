import type { ReactNode, RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { ArrowDownIcon, SpinnerIcon } from '@phosphor-icons/react';
import { cn } from '../lib/cn';
import {
  DegradedState,
  EmptyState,
  ErrorState,
  LoadingState,
} from './StateSurface';
import { Button } from './Button';

export interface WorkspacesMainWorkspace {
  id: string;
}

interface WorkspacesMainProps {
  workspaceWithSession: WorkspacesMainWorkspace | undefined;
  isLoading: boolean;
  workspaceState?: 'loading' | 'empty' | 'error' | 'ready' | 'degraded';
  isRetryingWorkspace?: boolean;
  onRetryWorkspace?: () => void | Promise<void>;
  showLoadingOverlay?: boolean;
  containerRef: RefObject<HTMLElement>;
  conversationContent?: ReactNode;
  chatBoxContent: ReactNode;
  contextBarContent?: ReactNode;
  isAtBottom?: boolean;
  onAtBottomChange?: (atBottom: boolean) => void;
  onScrollToBottom?: (behavior?: 'auto' | 'smooth') => void;
  isMobile?: boolean;
  /** Mobile-only banner rendered between the transcript and the composer. */
  approvalBannerContent?: ReactNode;
  compact?: boolean;
}

export function WorkspacesMain({
  workspaceWithSession,
  isLoading,
  workspaceState = 'ready',
  isRetryingWorkspace = false,
  onRetryWorkspace,
  showLoadingOverlay = false,
  containerRef,
  conversationContent,
  chatBoxContent,
  contextBarContent,
  isAtBottom = true,
  onScrollToBottom,
  isMobile,
  approvalBannerContent,
  compact = false,
}: WorkspacesMainProps) {
  const { t } = useTranslation(['tasks', 'common']);

  // Always render the main structure to prevent chat box flash during workspace transitions
  return (
    <main
      ref={containerRef}
      className={cn(
        'relative flex min-h-0 min-w-0 flex-1 flex-col bg-primary',
        !compact && !isMobile && 'h-full'
      )}
    >
      {/* Conversation content - conditional based on loading/workspace state */}
      {isLoading || workspaceState === 'loading' ? (
        <LoadingState
          className="flex-1"
          title={t('common:workspaces.loadingTitle', {
            defaultValue: 'Loading workspace',
          })}
        />
      ) : workspaceState === 'error' ? (
        <ErrorState
          className="flex-1"
          title={t('common:workspaces.errorTitle', {
            defaultValue: 'Workspace could not be loaded',
          })}
          description={t('common:workspaces.errorDescription', {
            defaultValue: 'Check the connection and try again.',
          })}
          action={
            onRetryWorkspace ? (
              <Button
                className="min-h-11"
                loading={isRetryingWorkspace}
                loadingLabel={t('common:workspaces.retrying', {
                  defaultValue: 'Retrying workspace',
                })}
                onClick={() => void onRetryWorkspace()}
              >
                {t('common:buttons.retry', { defaultValue: 'Retry' })}
              </Button>
            ) : undefined
          }
        />
      ) : workspaceState === 'degraded' && workspaceWithSession ? (
        <>
          <DegradedState
            compact
            className="mx-auto w-chat max-w-full"
            title={t('common:workspaces.degradedTitle', {
              defaultValue: 'Workspace may be out of date',
            })}
            description={t('common:workspaces.degradedDescription', {
              defaultValue:
                'The last loaded workspace remains available while the connection recovers.',
            })}
            action={
              onRetryWorkspace ? (
                <Button
                  className="min-h-11"
                  loading={isRetryingWorkspace}
                  loadingLabel={t('common:workspaces.retrying', {
                    defaultValue: 'Retrying workspace',
                  })}
                  onClick={() => void onRetryWorkspace()}
                >
                  {t('common:buttons.retry', { defaultValue: 'Retry' })}
                </Button>
              ) : undefined
            }
          />
          {conversationContent}
        </>
      ) : workspaceState === 'empty' || !workspaceWithSession ? (
        <EmptyState
          className="flex-1"
          title={t('common:workspaces.selectToStart', {
            defaultValue: 'Select a workspace to start',
          })}
        />
      ) : (
        <>
          {showLoadingOverlay && (
            <div className="absolute inset-0 z-10 flex items-center justify-center bg-primary">
              <SpinnerIcon className="size-6 animate-spin text-low" />
            </div>
          )}
          {conversationContent}
        </>
      )}
      {/* Scroll to bottom button */}
      {workspaceWithSession && !isAtBottom && (
        <div className="flex justify-center pointer-events-none">
          <div className="w-chat max-w-full relative">
            <button
              type="button"
              onClick={() => onScrollToBottom?.('auto')}
              className="absolute bottom-2 right-4 z-10 pointer-events-auto flex items-center justify-center size-8 rounded-full bg-secondary/80 backdrop-blur-sm border border-secondary text-low hover:text-normal hover:bg-secondary shadow-md transition-all"
              aria-label="Scroll to bottom"
              title="Scroll to bottom"
            >
              <ArrowDownIcon className="size-icon-base" weight="bold" />
            </button>
          </div>
        </div>
      )}
      {/* Approval banner (mobile) — sits between the scrolling transcript and
          the composer, inside the keyboard-aware shell so it stays reachable */}
      {approvalBannerContent}
      {/* Chat box - always rendered to prevent flash during workspace switch */}
      {!isLoading &&
        workspaceState !== 'loading' &&
        workspaceState !== 'error' && (
          <div
            className={cn(
              'flex shrink-0 justify-center @container pl-px',
              compact && 'max-h-[55%] overflow-y-auto'
            )}
            data-chatbox-container="true"
          >
            {chatBoxContent}
          </div>
        )}
      {/* Context Bar - floating toolbar */}
      {workspaceWithSession ? contextBarContent : null}
    </main>
  );
}
