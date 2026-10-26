import type { KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import {
  CaretDownIcon,
  ArrowSquareUpRightIcon,
  FileIcon as DefaultFileIcon,
  FileTextIcon,
} from '@phosphor-icons/react';
import { cn } from '../lib/cn';
import { formatPathForDisplay } from '../lib/path';
import { ToolStatusDot, type ToolStatusLike } from './ToolStatusDot';

export type ChatFileEntryDiffInput =
  | {
      type: 'content';
      oldContent: string;
      newContent: string;
      oldPath?: string;
      newPath: string;
    }
  | {
      type: 'unified';
      path: string;
      unifiedDiff: string;
      hasLineNumbers?: boolean;
    };

interface ChatFileEntryProps {
  filename: string;
  additions?: number;
  deletions?: number;
  expanded?: boolean;
  onToggle?: () => void;
  className?: string;
  status?: ToolStatusLike;
  active?: boolean;
  /** Optional diff content for expanded view */
  diffContent?: ChatFileEntryDiffInput;
  /** Optional callback to open file in changes panel */
  onOpenInChanges?: () => void;
  /** Optional callback to open the current workspace file preview */
  onOpenFilePreview?: () => void;
  /** Optional disabled state for the file preview action */
  filePreviewDisabled?: boolean;
  /** Optional title / aria-label for the file preview action */
  filePreviewTitle?: string;
  /** Optional file icon override from the app layer */
  fileIcon?: React.ElementType;
  /** Whether host app is running inside VSCode iframe */
  isVSCode?: boolean;
  /** Optional VSCode file opener from the app layer */
  onOpenInVSCode?: (filename: string) => void;
  /** Optional diff renderer from the app layer */
  renderDiffBody?: (diffContent: ChatFileEntryDiffInput) => React.ReactNode;
}

export function ChatFileEntry({
  filename,
  additions,
  deletions,
  expanded = false,
  onToggle,
  className,
  status,
  active,
  diffContent,
  onOpenInChanges,
  onOpenFilePreview,
  filePreviewDisabled = false,
  filePreviewTitle,
  fileIcon,
  isVSCode = false,
  onOpenInVSCode,
  renderDiffBody,
}: ChatFileEntryProps) {
  const { t } = useTranslation('tasks');
  const hasStats = additions !== undefined || deletions !== undefined;
  const FileIcon = fileIcon ?? DefaultFileIcon;
  const isDenied = status?.status === 'denied';
  const hasDiffContent = Boolean(diffContent && renderDiffBody);
  const previewTitle = filePreviewTitle ?? t('conversation.openFilePreview');
  const displayFilename = formatPathForDisplay(filename);

  const handleClick = () => {
    if (isVSCode) {
      onOpenInVSCode?.(filename);
      return;
    }
    onToggle?.();
  };
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget) return;
    if (event.key !== 'Enter' && event.key !== ' ') return;
    event.preventDefault();
    handleClick();
  };
  const isInteractive = Boolean(onToggle || isVSCode);

  // If we have diff content, wrap in a container with the diff body
  if (hasDiffContent) {
    return (
      <div
        className={cn(
          'rounded-sm border overflow-hidden',
          isDenied && 'border-error bg-error/10',
          className
        )}
      >
        {/* Header */}
        <div
          className={cn(
            'flex items-center p-base w-full',
            isDenied ? 'bg-error/20' : 'bg-panel',
            isInteractive && 'cursor-pointer'
          )}
          onClick={handleClick}
          onKeyDown={handleKeyDown}
          role={isInteractive ? 'button' : undefined}
          aria-expanded={onToggle ? expanded : undefined}
          tabIndex={isInteractive ? 0 : undefined}
          data-scroll-anchor-target={isInteractive ? '' : undefined}
        >
          <div className="flex-1 flex items-center gap-base min-w-0">
            <span className="relative inline-flex size-icon-base shrink-0 items-center justify-center">
              <FileIcon className="size-icon-base" />
              {status && (
                <span className="pointer-events-none absolute -bottom-0.5 -right-0.5 inline-flex">
                  <ToolStatusDot status={status} active={active} />
                </span>
              )}
            </span>
            <span
              className="text-sm text-normal truncate"
              title={displayFilename}
            >
              {displayFilename}
            </span>
            {onOpenFilePreview && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  onOpenFilePreview();
                }}
                disabled={filePreviewDisabled}
                className="shrink-0 p-0.5 rounded hover:bg-muted text-low hover:text-normal transition-colors disabled:pointer-events-none disabled:opacity-50"
                title={previewTitle}
                aria-label={previewTitle}
              >
                <FileTextIcon className="size-icon-xs" />
              </button>
            )}
            {onOpenInChanges && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  onOpenInChanges();
                }}
                className="shrink-0 p-0.5 rounded hover:bg-muted text-low hover:text-normal transition-colors"
                title={t('conversation.viewInChangesPanel')}
                aria-label={t('conversation.viewInChangesPanel')}
              >
                <ArrowSquareUpRightIcon className="size-icon-xs" />
              </button>
            )}
            {hasStats && (
              <span className="text-sm shrink-0">
                {additions !== undefined && additions > 0 && (
                  <span className="text-success">+{additions}</span>
                )}
                {additions !== undefined && deletions !== undefined && ' '}
                {deletions !== undefined && deletions > 0 && (
                  <span className="text-error">-{deletions}</span>
                )}
              </span>
            )}
          </div>
          {!isVSCode && onToggle && (
            <CaretDownIcon
              className={cn(
                'size-icon-xs shrink-0 text-low transition-transform',
                !expanded && '-rotate-90'
              )}
            />
          )}
        </div>

        {/* Diff body - shown when expanded */}
        {!isVSCode && expanded && diffContent && renderDiffBody?.(diffContent)}
      </div>
    );
  }

  // Original header-only rendering (no diff content)
  return (
    <div
      className={cn(
        'flex items-center border rounded-sm p-base w-full',
        isDenied ? 'bg-error/20 border-error' : 'bg-panel',
        isInteractive && 'cursor-pointer',
        className
      )}
      onClick={handleClick}
      onKeyDown={handleKeyDown}
      role={isInteractive ? 'button' : undefined}
      aria-expanded={onToggle ? expanded : undefined}
      tabIndex={isInteractive ? 0 : undefined}
      data-scroll-anchor-target={isInteractive ? '' : undefined}
    >
      <div className="flex-1 flex items-center gap-base min-w-0">
        <span className="relative inline-flex size-icon-base shrink-0 items-center justify-center">
          <FileIcon className="size-icon-base" />
          {status && (
            <span className="pointer-events-none absolute -bottom-0.5 -right-0.5 inline-flex">
              <ToolStatusDot status={status} active={active} />
            </span>
          )}
        </span>
        <span className="text-sm text-normal truncate" title={displayFilename}>
          {displayFilename}
        </span>
        {onOpenFilePreview && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onOpenFilePreview();
            }}
            disabled={filePreviewDisabled}
            className="shrink-0 p-0.5 rounded hover:bg-muted text-low hover:text-normal transition-colors disabled:pointer-events-none disabled:opacity-50"
            title={previewTitle}
            aria-label={previewTitle}
          >
            <FileTextIcon className="size-icon-xs" />
          </button>
        )}
        {onOpenInChanges && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onOpenInChanges();
            }}
            className="shrink-0 p-0.5 rounded hover:bg-muted text-low hover:text-normal transition-colors"
            title={t('conversation.viewInChangesPanel')}
            aria-label={t('conversation.viewInChangesPanel')}
          >
            <ArrowSquareUpRightIcon className="size-icon-xs" />
          </button>
        )}
        {hasStats && (
          <span className="text-sm shrink-0">
            {additions !== undefined && additions > 0 && (
              <span className="text-success">+{additions}</span>
            )}
            {additions !== undefined && deletions !== undefined && ' '}
            {deletions !== undefined && deletions > 0 && (
              <span className="text-error">-{deletions}</span>
            )}
          </span>
        )}
      </div>
      {!isVSCode && onToggle && (
        <CaretDownIcon
          className={cn(
            'size-icon-xs shrink-0 text-low transition-transform',
            !expanded && '-rotate-90'
          )}
        />
      )}
    </div>
  );
}
