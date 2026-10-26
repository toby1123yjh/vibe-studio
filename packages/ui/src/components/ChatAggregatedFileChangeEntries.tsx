import {
  useMemo,
  useState,
  type ElementType,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import {
  ArrowSquareUpRightIcon,
  CaretDownIcon,
  FileIcon as DefaultFileIcon,
  FileTextIcon,
  FilesIcon,
} from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { cn } from '../lib/cn';
import { formatPathForDisplay } from '../lib/path';
import { getFileChangeStats } from '../lib/diffStats';
import { ToolStatusDot, type ToolStatusLike } from './ToolStatusDot';
import type { ChatFileEntryDiffInput } from './ChatFileEntry';

export type ChatAggregatedFileChange = {
  action: 'edit' | 'write' | 'delete' | 'rename';
  unified_diff?: string;
  has_line_numbers?: boolean;
  content?: string;
  new_path?: string;
};

export interface AggregatedFileChangeEntry {
  filePath: string;
  change: ChatAggregatedFileChange;
  status: ToolStatusLike | null;
  active?: boolean;
  expansionKey: string;
  fileIcon?: ElementType;
  onOpenInChanges?: () => void;
  onOpenFilePreview?: () => void;
  filePreviewDisabled?: boolean;
  filePreviewTitle?: string;
  onOpenInVSCode?: () => void;
}

function isEntryActive(entry: AggregatedFileChangeEntry) {
  return (
    entry.active ??
    (entry.status?.status === 'created' ||
      entry.status?.status === 'pending_approval')
  );
}

interface ChatAggregatedFileChangeEntriesProps {
  entries: AggregatedFileChangeEntry[];
  expanded: boolean;
  isHovered: boolean;
  onToggle: () => void;
  onHoverChange: (hovered: boolean) => void;
  className?: string;
  isVSCode?: boolean;
  renderDiffBody?: (args: {
    filePath: string;
    change: ChatAggregatedFileChange;
    diffContent?: ChatFileEntryDiffInput;
  }) => ReactNode;
}

const STATUS_PRIORITY: Record<string, number> = {
  failed: 6,
  denied: 5,
  timed_out: 4,
  pending_approval: 3,
  created: 2,
  success: 1,
};

function buildDiffContent(
  change: ChatAggregatedFileChange,
  filePath: string
): ChatFileEntryDiffInput | undefined {
  if (change.action === 'edit' && change.unified_diff) {
    return {
      type: 'unified',
      path: filePath,
      unifiedDiff: change.unified_diff,
      hasLineNumbers: change.has_line_numbers ?? true,
    };
  }

  if (change.action === 'write' && change.content) {
    return {
      type: 'content',
      oldContent: '',
      newContent: change.content,
      newPath: filePath,
    };
  }

  return undefined;
}

function getActionLabel(
  change: ChatAggregatedFileChange,
  t: ReturnType<typeof useTranslation>['t']
) {
  switch (change.action) {
    case 'edit':
      return t('conversation.aggregated.actions.edit');
    case 'write':
      return t('conversation.aggregated.actions.write');
    case 'delete':
      return t('conversation.aggregated.actions.delete');
    case 'rename':
      return change.new_path
        ? t('conversation.aggregated.actions.renameTo', {
            path: formatPathForDisplay(change.new_path),
          })
        : t('conversation.aggregated.actions.rename');
    default:
      return t('conversation.aggregated.actions.change');
  }
}

function getWorstStatus(entries: AggregatedFileChangeEntry[]) {
  return entries.reduce<ToolStatusLike | null>((worst, entry) => {
    if (!entry.status) return worst;
    if (!worst) return entry.status;

    const worstPriority = STATUS_PRIORITY[worst.status] || 0;
    const currentPriority = STATUS_PRIORITY[entry.status.status] || 0;

    return currentPriority > worstPriority ? entry.status : worst;
  }, null);
}

function FileChangeStats({
  additions,
  deletions,
}: {
  additions: number;
  deletions: number;
}) {
  if (additions <= 0 && deletions <= 0) return null;

  return (
    <span className="shrink-0 text-sm tabular-nums">
      {additions > 0 && <span className="text-success">+{additions}</span>}
      {additions > 0 && deletions > 0 && ' '}
      {deletions > 0 && <span className="text-error">-{deletions}</span>}
    </span>
  );
}

function FileChangeRow({
  entry,
  isVSCode,
  renderDiffBody,
}: {
  entry: AggregatedFileChangeEntry;
  isVSCode: boolean;
  renderDiffBody?: (args: {
    filePath: string;
    change: ChatAggregatedFileChange;
    diffContent?: ChatFileEntryDiffInput;
  }) => ReactNode;
}) {
  const { t } = useTranslation('tasks');
  const [expanded, setExpanded] = useState(false);
  const FileIcon = entry.fileIcon ?? DefaultFileIcon;
  const stats = useMemo(() => getFileChangeStats(entry.change), [entry.change]);
  const diffContent = useMemo(
    () => buildDiffContent(entry.change, entry.filePath),
    [entry.change, entry.filePath]
  );
  const hasDiffContent = Boolean(diffContent && renderDiffBody);
  const canToggle = !isVSCode && hasDiffContent;

  const handleClick = () => {
    if (isVSCode) {
      entry.onOpenInVSCode?.();
      return;
    }

    if (canToggle) {
      setExpanded((current) => !current);
    }
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget) return;
    if (event.key !== 'Enter' && event.key !== ' ') return;
    event.preventDefault();
    handleClick();
  };

  return (
    <div className="border-t border-muted/60 first:border-t-0">
      <div
        className={cn(
          'flex min-w-0 items-center gap-base px-base py-2',
          (canToggle || isVSCode) && 'cursor-pointer hover:bg-muted/30'
        )}
        onClick={handleClick}
        onKeyDown={handleKeyDown}
        role={canToggle || isVSCode ? 'button' : undefined}
        tabIndex={canToggle || isVSCode ? 0 : undefined}
        aria-expanded={canToggle ? expanded : undefined}
        data-scroll-anchor-target={canToggle || isVSCode ? '' : undefined}
      >
        <span className="relative inline-flex size-icon-base shrink-0 items-center justify-center">
          {canToggle ? (
            <CaretDownIcon
              className={cn(
                'size-icon-base text-low transition-transform',
                !expanded && '-rotate-90'
              )}
            />
          ) : (
            <FileIcon className="size-icon-base text-low" />
          )}
          {entry.status && (
            <span className="pointer-events-none absolute -bottom-0.5 -right-0.5 inline-flex">
              <ToolStatusDot status={entry.status} active={entry.active} />
            </span>
          )}
        </span>

        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-base">
            <span
              className="truncate text-sm text-normal"
              title={formatPathForDisplay(entry.filePath)}
            >
              {formatPathForDisplay(entry.filePath)}
            </span>
            <span className="shrink-0 text-xs text-low">
              {getActionLabel(entry.change, t)}
            </span>
          </div>
        </div>

        {!isVSCode && entry.onOpenFilePreview && (
          <button
            type="button"
            onClick={(event) => {
              event.stopPropagation();
              entry.onOpenFilePreview?.();
            }}
            disabled={entry.filePreviewDisabled}
            className="shrink-0 rounded p-0.5 text-low transition-colors hover:bg-muted hover:text-normal disabled:pointer-events-none disabled:opacity-50"
            title={entry.filePreviewTitle}
            aria-label={entry.filePreviewTitle}
          >
            <FileTextIcon className="size-icon-xs" />
          </button>
        )}

        {!isVSCode && entry.onOpenInChanges && (
          <button
            type="button"
            onClick={(event) => {
              event.stopPropagation();
              entry.onOpenInChanges?.();
            }}
            className="shrink-0 rounded p-0.5 text-low transition-colors hover:bg-muted hover:text-normal"
            title={t('conversation.viewInChangesPanel')}
            aria-label={t('conversation.viewInChangesPanel')}
          >
            <ArrowSquareUpRightIcon className="size-icon-xs" />
          </button>
        )}

        <FileChangeStats
          additions={stats.additions}
          deletions={stats.deletions}
        />
      </div>

      {!isVSCode &&
        expanded &&
        diffContent &&
        renderDiffBody?.({
          filePath: entry.filePath,
          change: entry.change,
          diffContent,
        })}
    </div>
  );
}

export function ChatAggregatedFileChangeEntries({
  entries,
  expanded,
  isHovered,
  onToggle,
  onHoverChange,
  className,
  isVSCode = false,
  renderDiffBody,
}: ChatAggregatedFileChangeEntriesProps) {
  const { t } = useTranslation('tasks');
  const aggregateStatus = useMemo(() => getWorstStatus(entries), [entries]);
  const isAggregateActive = useMemo(
    () => entries.some(isEntryActive),
    [entries]
  );
  const totals = useMemo(() => {
    const changedFileCount = new Set(entries.map((entry) => entry.filePath))
      .size;
    let additions = 0;
    let deletions = 0;
    const actionCounts = {
      write: 0,
      edit: 0,
      delete: 0,
      rename: 0,
    };

    for (const entry of entries) {
      const stats = getFileChangeStats(entry.change);
      additions += stats.additions;
      deletions += stats.deletions;
      actionCounts[entry.change.action] += 1;
    }

    return {
      changedFileCount,
      additions,
      deletions,
      actionCounts,
    };
  }, [entries]);
  const isDenied = aggregateStatus?.status === 'denied';
  const summaryParts = [
    totals.actionCounts.write > 0 &&
      t('conversation.aggregated.changeSummary.write', {
        count: totals.actionCounts.write,
      }),
    totals.actionCounts.edit > 0 &&
      t('conversation.aggregated.changeSummary.edit', {
        count: totals.actionCounts.edit,
      }),
    totals.actionCounts.delete > 0 &&
      t('conversation.aggregated.changeSummary.delete', {
        count: totals.actionCounts.delete,
      }),
    totals.actionCounts.rename > 0 &&
      t('conversation.aggregated.changeSummary.rename', {
        count: totals.actionCounts.rename,
      }),
  ].filter(Boolean);
  const previewTitle = t('conversation.openFilePreview');

  if (entries.length === 0) return null;

  return (
    <div
      className={cn(
        'overflow-hidden rounded-sm border',
        isDenied && 'border-error bg-error/10',
        className
      )}
    >
      <button
        type="button"
        className={cn(
          'flex w-full min-w-0 items-center gap-base px-base py-2 text-left transition-colors',
          isDenied ? 'bg-error/20' : 'bg-panel hover:bg-muted/30'
        )}
        onClick={onToggle}
        onMouseEnter={() => onHoverChange(true)}
        onMouseLeave={() => onHoverChange(false)}
        aria-expanded={expanded}
        data-scroll-anchor-target=""
      >
        <span className="relative inline-flex size-icon-base shrink-0 items-center justify-center">
          {isHovered ? (
            <CaretDownIcon
              className={cn(
                'size-icon-base text-low transition-transform',
                !expanded && '-rotate-90'
              )}
            />
          ) : (
            <FilesIcon className="size-icon-base text-low" />
          )}
          {aggregateStatus && (
            <span className="pointer-events-none absolute -bottom-0.5 -right-0.5 inline-flex">
              <ToolStatusDot
                status={aggregateStatus}
                active={isAggregateActive}
              />
            </span>
          )}
        </span>

        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-base">
            <span className="truncate text-sm text-normal">
              {t('conversation.aggregated.changedFiles', {
                count: totals.changedFileCount,
              })}
            </span>
            {summaryParts.length > 0 && (
              <span className="hidden shrink-0 text-xs text-low sm:inline">
                {summaryParts.join(' · ')}
              </span>
            )}
          </div>
        </div>

        <FileChangeStats
          additions={totals.additions}
          deletions={totals.deletions}
        />
      </button>

      {expanded && (
        <div className="border-t">
          {entries.map((entry) => (
            <FileChangeRow
              key={entry.expansionKey}
              entry={{
                ...entry,
                filePreviewTitle: entry.filePreviewTitle ?? previewTitle,
              }}
              isVSCode={isVSCode}
              renderDiffBody={renderDiffBody}
            />
          ))}
        </div>
      )}
    </div>
  );
}
