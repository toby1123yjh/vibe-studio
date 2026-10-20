import { cn } from '../lib/cn';
import { useTranslation } from 'react-i18next';
import { Tooltip } from './Tooltip';
import {
  ArrowFatLineUpIcon,
  ArrowUpIcon,
  ArrowDownIcon,
  MinusIcon,
} from '@phosphor-icons/react';

export type PriorityLevel = 'urgent' | 'high' | 'medium' | 'low';

export interface PriorityIconProps {
  priority: PriorityLevel | null;
  className?: string;
}

const priorityConfig: Record<
  PriorityLevel,
  { icon: typeof ArrowUpIcon; colorClass: string }
> = {
  urgent: { icon: ArrowFatLineUpIcon, colorClass: 'text-error' },
  high: { icon: ArrowUpIcon, colorClass: 'text-brand' },
  medium: { icon: MinusIcon, colorClass: 'text-low' },
  low: { icon: ArrowDownIcon, colorClass: 'text-success' },
};

export const PriorityIcon = ({ priority, className }: PriorityIconProps) => {
  const { t } = useTranslation('common');
  if (!priority) return null;
  const priorityLabels: Record<PriorityLevel, string> = {
    urgent: t('kanban.priorityUrgent'),
    high: t('kanban.priorityHigh'),
    medium: t('kanban.priorityMedium'),
    low: t('kanban.priorityLow'),
  };
  const label = priorityLabels[priority];
  const { icon: IconComponent, colorClass } = priorityConfig[priority];
  return (
    <Tooltip content={label}>
      <span role="img" aria-label={label} className="inline-flex shrink-0">
        <IconComponent
          className={cn('size-icon-xs', colorClass, className)}
          weight="bold"
          aria-hidden="true"
        />
      </span>
    </Tooltip>
  );
};
