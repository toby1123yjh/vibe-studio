import { CaretDownIcon, ListMagnifyingGlassIcon } from '@phosphor-icons/react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { cn } from '../lib/cn';
import { ActivityText } from './ActivityText';
import { ChatElapsedTime } from './ChatElapsedTime';
import { ToolStatusDot, type ToolStatusLike } from './ToolStatusDot';

export interface AggregatedEntry {
  summary: string;
  status?: ToolStatusLike;
  active?: boolean;
  expansionKey: string;
  content?: string;
  command?: string;
  startedAt?: string | null;
  endedAt?: string | null;
}

interface ChatAggregatedToolEntriesProps {
  entries: AggregatedEntry[];
  expanded: boolean;
  isHovered: boolean;
  onToggle: () => void;
  onHoverChange: (hovered: boolean) => void;
  summary?: string;
  detail?: string;
  label?: string;
  unit?: string;
  icon?: React.ElementType;
  className?: string;
  forceCollapsible?: boolean;
  startedAt?: string | null;
  endedAt?: string | null;
}

function isEntryActive(entry: AggregatedEntry) {
  return entry.active ?? isActiveStatus(entry.status);
}

function getAggregateLifecycleStatus(entries: AggregatedEntry[]) {
  const activeEntry = entries.find(isEntryActive);
  return activeEntry?.status ?? { status: 'success' };
}

function getEntryDetails(entry: AggregatedEntry) {
  const content = entry.content?.trim();
  if (!content) return null;

  return {
    command: entry.command?.trim() || null,
    content,
  };
}

function ToolEntryDetails({ entry }: { entry: AggregatedEntry }) {
  const details = getEntryDetails(entry);
  if (!details) return null;

  return (
    <div className="mx-base mb-half overflow-hidden rounded-sm border border-border/70 bg-panel/70">
      {details.command && (
        <pre className="overflow-x-auto border-b border-border/70 px-base py-half font-mono text-xs leading-relaxed text-normal whitespace-pre-wrap break-words">
          {details.command}
        </pre>
      )}
      <pre className="max-h-56 overflow-auto px-base py-half font-mono text-xs leading-relaxed text-low whitespace-pre-wrap break-words">
        {details.content}
      </pre>
    </div>
  );
}

function getFallbackSummary({
  entries,
  label,
  unit,
  fallbackSummary,
}: {
  entries: AggregatedEntry[];
  label?: string;
  unit?: string;
  fallbackSummary: string;
}) {
  if (!label || !unit) return fallbackSummary;
  return `${label} ${entries.length} ${unit}`;
}

function isActiveStatus(status?: ToolStatusLike) {
  return status?.status === 'created' || status?.status === 'pending_approval';
}

export function ChatAggregatedToolEntries({
  entries,
  expanded,
  isHovered,
  onToggle,
  onHoverChange,
  summary,
  detail,
  label,
  unit,
  icon: Icon = ListMagnifyingGlassIcon,
  className,
  forceCollapsible = false,
  startedAt,
  endedAt,
}: ChatAggregatedToolEntriesProps) {
  const { t } = useTranslation('tasks');
  const [expandedEntryKey, setExpandedEntryKey] = useState<string | null>(null);

  if (entries.length === 0) return null;

  const isRunning = entries.some(isEntryActive);
  const aggregateStatus = getAggregateLifecycleStatus(entries);
  const headerSummary =
    summary ??
    getFallbackSummary({
      entries,
      label,
      unit,
      fallbackSummary: t('conversation.aggregated.operationCount', {
        count: entries.length,
      }),
    });

  if (entries.length === 1 && !forceCollapsible) {
    const entry = entries[0];
    const hasDetails = Boolean(getEntryDetails(entry));
    const entryExpanded = expandedEntryKey === entry.expansionKey;
    return (
      <div className={cn('flex flex-col', className)}>
        <button
          type="button"
          className={cn(
            'flex min-w-0 items-center gap-base text-left text-sm text-low transition-colors',
            hasDetails && 'cursor-pointer hover:text-normal'
          )}
          onClick={() =>
            setExpandedEntryKey(entryExpanded ? null : entry.expansionKey)
          }
          disabled={!hasDetails}
          aria-expanded={hasDetails ? entryExpanded : undefined}
        >
          <span className="relative inline-flex size-icon-base shrink-0 items-center justify-center">
            <Icon className="size-icon-base" />
            {entry.status && (
              <span className="pointer-events-none absolute -bottom-0.5 -left-0.5 inline-flex">
                <ToolStatusDot
                  status={entry.status}
                  active={isEntryActive(entry)}
                />
              </span>
            )}
          </span>
          <ActivityText active={isRunning} className="min-w-0 flex-1 truncate">
            {headerSummary}
          </ActivityText>
          {detail && (
            <span className="hidden shrink-0 text-xs text-low sm:inline">
              {detail}
            </span>
          )}
          <ChatElapsedTime
            startedAt={entry.startedAt ?? startedAt}
            endedAt={entry.endedAt ?? endedAt}
            active={isEntryActive(entry)}
          />
          {hasDetails && (
            <CaretDownIcon
              className={cn(
                'size-icon-sm shrink-0 transition-transform duration-150',
                !entryExpanded && '-rotate-90'
              )}
            />
          )}
        </button>
        {entryExpanded && <ToolEntryDetails entry={entry} />}
      </div>
    );
  }

  return (
    <div className={cn('flex flex-col', className)}>
      <button
        type="button"
        className="group -mx-half flex min-w-0 items-center gap-base rounded-sm px-half py-0.5 text-left text-sm text-low transition-colors hover:bg-muted/30 hover:text-normal"
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
                'size-icon-base transition-transform duration-150',
                !expanded && '-rotate-90'
              )}
            />
          ) : (
            <Icon className="size-icon-base" />
          )}
          {aggregateStatus && (
            <span className="pointer-events-none absolute -bottom-0.5 -left-0.5 inline-flex">
              <ToolStatusDot status={aggregateStatus} />
            </span>
          )}
        </span>

        <ActivityText active={isRunning} className="min-w-0 flex-1 truncate">
          {headerSummary}
        </ActivityText>
        {detail && (
          <span className="hidden shrink-0 text-xs text-low sm:inline">
            {detail}
          </span>
        )}
        <ChatElapsedTime
          startedAt={startedAt}
          endedAt={endedAt}
          active={isRunning}
        />
      </button>

      {expanded && (
        <div className="ml-6 mt-1 max-h-72 overflow-y-auto rounded-md border border-border bg-muted/10 p-half">
          {entries.map((entry) => {
            const hasDetails = Boolean(getEntryDetails(entry));
            const entryExpanded = expandedEntryKey === entry.expansionKey;

            return (
              <div key={entry.expansionKey}>
                <button
                  type="button"
                  className={cn(
                    'group/item flex w-full min-w-0 items-center gap-base rounded-sm px-base py-half text-left text-sm text-low transition-colors',
                    hasDetails &&
                      'cursor-pointer hover:bg-muted/30 hover:text-normal'
                  )}
                  onClick={() =>
                    setExpandedEntryKey(
                      entryExpanded ? null : entry.expansionKey
                    )
                  }
                  disabled={!hasDetails}
                  aria-expanded={hasDetails ? entryExpanded : undefined}
                >
                  <span className="relative inline-flex size-icon-base shrink-0 items-center justify-center">
                    <Icon className="size-icon-base" />
                    {entry.status && (
                      <span className="pointer-events-none absolute -bottom-0.5 -left-0.5 inline-flex">
                        <ToolStatusDot
                          status={entry.status}
                          active={isEntryActive(entry)}
                        />
                      </span>
                    )}
                  </span>
                  <span className="flex min-w-0 flex-1 items-center gap-base">
                    <span
                      className={cn(
                        'min-w-0 flex-1 truncate',
                        entry.command && 'font-mono text-xs'
                      )}
                    >
                      {entry.summary}
                    </span>
                    <ChatElapsedTime
                      startedAt={entry.startedAt}
                      endedAt={entry.endedAt}
                      active={isEntryActive(entry)}
                    />
                    {hasDetails && (
                      <CaretDownIcon
                        className={cn(
                          'size-icon-sm shrink-0 transition-transform duration-150',
                          !entryExpanded && '-rotate-90'
                        )}
                      />
                    )}
                  </span>
                </button>
                {entryExpanded && <ToolEntryDetails entry={entry} />}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
