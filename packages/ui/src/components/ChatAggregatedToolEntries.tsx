import { ListMagnifyingGlassIcon } from '@phosphor-icons/react';
import { useState, type ElementType } from 'react';
import { useTranslation } from 'react-i18next';
import { ChatDisclosure, ChatDisclosureRow } from './ChatDisclosure';
import { ChatElapsedTime } from './ChatElapsedTime';
import type { ToolStatusLike } from './ToolStatusDot';

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
  icon?: ElementType;
  className?: string;
  forceCollapsible?: boolean;
  startedAt?: string | null;
  endedAt?: string | null;
}

function isEntryActive(entry: AggregatedEntry) {
  return (
    entry.active ??
    (entry.status?.status === 'created' ||
      entry.status?.status === 'pending_approval')
  );
}

function ToolEntryDetails({ entry }: { entry: AggregatedEntry }) {
  return (
    <div className="min-w-0 overflow-hidden rounded-[var(--vk-radius-sm)] bg-secondary">
      {entry.command?.trim() && (
        <pre className="overflow-x-auto border-b border-border px-base py-half font-ibm-plex-mono text-base leading-relaxed text-normal whitespace-pre-wrap break-words">
          {entry.command}
        </pre>
      )}
      <pre className="max-h-80 overflow-auto px-base py-half font-ibm-plex-mono text-base leading-relaxed text-low whitespace-pre-wrap break-words">
        {entry.content}
      </pre>
    </div>
  );
}

function ToolEntry({
  entry,
  icon: Icon,
  expanded,
  onToggle,
}: {
  entry: AggregatedEntry;
  icon: ElementType;
  expanded: boolean;
  onToggle: () => void;
}) {
  const row = {
    icon: <Icon className="size-icon-base" />,
    label: entry.summary,
    active: isEntryActive(entry),
    status: entry.status,
    trailing: (
      <ChatElapsedTime
        startedAt={entry.startedAt}
        endedAt={entry.endedAt}
        active={isEntryActive(entry)}
      />
    ),
  };
  return entry.content?.trim() ? (
    <ChatDisclosure {...row} expanded={expanded} onToggle={onToggle}>
      {expanded && <ToolEntryDetails entry={entry} />}
    </ChatDisclosure>
  ) : (
    <ChatDisclosureRow {...row} />
  );
}

export function ChatAggregatedToolEntries({
  entries,
  expanded,
  onToggle,
  onHoverChange,
  summary,
  detail,
  label,
  icon: Icon = ListMagnifyingGlassIcon,
  className,
  forceCollapsible = false,
  startedAt,
  endedAt,
}: ChatAggregatedToolEntriesProps) {
  const { t } = useTranslation('common');
  const [expandedEntries, setExpandedEntries] = useState<Set<string>>(
    () => new Set()
  );
  const toggleEntry = (key: string) =>
    setExpandedEntries((previous) => {
      const next = new Set(previous);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  if (entries.length === 0) return null;

  const latest = [...entries].reverse();
  const running = latest.find(isEntryActive);
  const failed = latest.find((entry) =>
    ['failed', 'denied', 'timed_out'].includes(entry.status?.status ?? '')
  );
  const selected = running ?? failed ?? latest[0];
  const statusLabel = failed
    ? t('conversation.toolGroupFailed', { defaultValue: 'Tool error' })
    : selected.status?.status === 'unconfirmed'
      ? t('conversation.toolResultUnconfirmed')
      : undefined;

  if (entries.length === 1 && !forceCollapsible) {
    const entry = entries[0];
    return (
      <div className={className}>
        <ToolEntry
          entry={entry}
          icon={Icon}
          expanded={expandedEntries.has(entry.expansionKey)}
          onToggle={() => toggleEntry(entry.expansionKey)}
        />
      </div>
    );
  }

  return (
    <ChatDisclosure
      className={className}
      icon={<Icon className="size-icon-base" />}
      label={
        summary ??
        label ??
        t('conversation.toolGroupLabel', {
          defaultValue: 'Tool activity',
        })
      }
      summary={detail ?? selected.summary}
      active={Boolean(running)}
      status={selected.status}
      statusLabel={statusLabel}
      expanded={expanded}
      onToggle={onToggle}
      onHoverChange={onHoverChange}
      bodyClassName="space-y-half"
      trailing={
        <ChatElapsedTime
          startedAt={startedAt}
          endedAt={endedAt}
          active={Boolean(running)}
        />
      }
    >
      {expanded &&
        entries.map((entry) => (
          <ToolEntry
            key={entry.expansionKey}
            entry={entry}
            icon={Icon}
            expanded={expandedEntries.has(entry.expansionKey)}
            onToggle={() => toggleEntry(entry.expansionKey)}
          />
        ))}
    </ChatDisclosure>
  );
}
