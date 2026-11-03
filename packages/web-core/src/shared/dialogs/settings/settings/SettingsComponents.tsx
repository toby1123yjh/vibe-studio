import type { ReactNode } from 'react';
import { cn } from '@/shared/lib/utils';
import { useTranslation } from 'react-i18next';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  DropdownMenuTriggerButton,
} from '@vibe/ui/components/Dropdown';
import { Button } from '@vibe/ui/components/Button';
import { Input } from '@vibe/ui/components/Input';
import type { Icon } from '@phosphor-icons/react';

// ============================================================================
// Two-Column Picker Components
// ============================================================================

// TwoColumnPicker - Container for the two-column layout
interface TwoColumnPickerProps {
  children: ReactNode;
}

export function TwoColumnPicker({ children }: TwoColumnPickerProps) {
  return (
    <div className="flex flex-col md:flex-row border border-border rounded-sm overflow-hidden">
      {children}
    </div>
  );
}

// TwoColumnPickerColumn - A single column within the picker
interface TwoColumnPickerColumnProps {
  label: string;
  headerAction?: ReactNode;
  isFirst?: boolean;
  children: ReactNode;
}

export function TwoColumnPickerColumn({
  label,
  headerAction,
  isFirst,
  children,
}: TwoColumnPickerColumnProps) {
  return (
    <div
      className={cn(
        'flex-1',
        isFirst && 'border-b md:border-b-0 md:border-r border-border'
      )}
    >
      <div className="h-9 px-base border-b border-border bg-secondary/50 flex items-center justify-between">
        <span className="text-sm font-medium text-low tracking-wide">
          {label}
        </span>
        {headerAction}
      </div>
      <div className="max-h-32 md:h-32 overflow-y-auto bg-panel">
        {children}
      </div>
    </div>
  );
}

// TwoColumnPickerItem - A selectable row within a column
interface TwoColumnPickerItemProps {
  selected?: boolean;
  onClick?: () => void;
  leading?: ReactNode;
  trailing?: ReactNode;
  children: ReactNode;
}

export function TwoColumnPickerItem({
  selected,
  onClick,
  leading,
  trailing,
  children,
}: TwoColumnPickerItemProps) {
  return (
    <div
      className={cn(
        'group flex items-center gap-half px-base py-half cursor-pointer transition-colors',
        'hover:bg-secondary',
        selected && 'bg-brand/10 text-brand'
      )}
      onClick={onClick}
    >
      {leading}
      <span
        className={cn(
          'text-sm truncate flex-1',
          selected ? 'text-brand font-medium' : 'text-normal'
        )}
      >
        {children}
      </span>
      {trailing}
    </div>
  );
}

// TwoColumnPickerBadge - A small badge/tag for items
interface TwoColumnPickerBadgeProps {
  variant?: 'default' | 'brand';
  children: ReactNode;
}

export function TwoColumnPickerBadge({
  variant = 'default',
  children,
}: TwoColumnPickerBadgeProps) {
  return (
    <span
      className={cn(
        'text-xs px-half rounded font-medium shrink-0',
        variant === 'brand' ? 'bg-brand/15 text-brand' : 'bg-secondary text-low'
      )}
    >
      {children}
    </span>
  );
}

// TwoColumnPickerEmpty - Empty state message for a column
interface TwoColumnPickerEmptyProps {
  children: ReactNode;
}

export function TwoColumnPickerEmpty({ children }: TwoColumnPickerEmptyProps) {
  return (
    <div className="px-base py-plusfifty text-sm text-low text-center">
      {children}
    </div>
  );
}

// ============================================================================
// Settings Card Components
// ============================================================================

// SettingsCard - A card container for a settings subsection
export function SettingsCard({
  title,
  description,
  children,
  headerAction,
}: {
  title: string;
  description?: ReactNode;
  children: React.ReactNode;
  headerAction?: React.ReactNode;
}) {
  return (
    <div className="vk-settings-card grid min-w-0 gap-4 border-b border-[var(--vk-border-subtle)] py-5 first:pt-0 last:border-b-0 last:pb-0">
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 flex-1 basis-48">
          <h3 className="text-[14px] font-medium leading-5 text-[var(--vk-text-high)]">
            {title}
          </h3>
          {description && (
            <p className="mt-1 max-w-[65ch] text-[13px] leading-relaxed text-[var(--vk-text-low)]">
              {description}
            </p>
          )}
        </div>
        {headerAction && (
          <div className="max-w-full shrink-0">{headerAction}</div>
        )}
      </div>
      <div className="min-w-0 space-y-5">{children}</div>
    </div>
  );
}

// SettingsField - A labeled field wrapper
export function SettingsField({
  label,
  description,
  error,
  children,
}: {
  label: string;
  description?: React.ReactNode;
  error?: string | null;
  children: React.ReactNode;
}) {
  return (
    <div className="vk-settings-field min-w-0 space-y-2">
      {label && (
        <label className="text-[13px] font-medium leading-5 text-[var(--vk-text-normal)]">
          {label}
        </label>
      )}
      {children}
      {error && (
        <p
          role="alert"
          className="text-[13px] leading-5 text-[var(--vk-status-error-text)]"
        >
          {error}
        </p>
      )}
      {description && !error && (
        <p className="max-w-[65ch] text-[13px] leading-relaxed text-[var(--vk-text-low)]">
          {description}
        </p>
      )}
    </div>
  );
}

// SettingsCheckbox - A checkbox with label and optional description
export function SettingsCheckbox({
  id,
  label,
  description,
  checked,
  onChange,
  disabled,
}: {
  id: string;
  label: string;
  description?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex items-center gap-3">
      <input
        type="checkbox"
        id={id}
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        disabled={disabled}
        className={cn(
          'mt-0.5 h-4 w-4 rounded border-border bg-secondary text-brand focus:ring-brand focus:ring-offset-0',
          disabled && 'opacity-50 cursor-not-allowed'
        )}
      />
      <div className="space-y-0.5">
        <label
          htmlFor={id}
          className={cn(
            'text-sm font-medium text-normal cursor-pointer',
            disabled && 'opacity-50 cursor-not-allowed'
          )}
        >
          {label}
        </label>
        {description && <p className="text-sm text-low">{description}</p>}
      </div>
    </div>
  );
}

// SettingsSelect - A dropdown select component
export function SettingsSelect<T extends string>({
  value,
  options,
  actions,
  onChange,
  placeholder,
  disabled,
  className,
}: {
  value: T | undefined;
  options: { value: T; label: string }[];
  actions?: { label: string; icon?: Icon; onClick: () => void }[];
  onChange: (value: T) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
}) {
  const selectedOption = options.find((opt) => opt.value === value);
  const hasMenuItems = options.length > 0 || (actions?.length ?? 0) > 0;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <DropdownMenuTriggerButton
          label={selectedOption?.label || placeholder}
          className={cn('w-full justify-between', className)}
          disabled={disabled || !hasMenuItems}
        />
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-[var(--radix-dropdown-menu-trigger-width)]">
        {options.map((option) => (
          <DropdownMenuItem
            key={option.value}
            onClick={() => onChange(option.value)}
          >
            {option.label}
          </DropdownMenuItem>
        ))}
        {actions?.map((action) => (
          <DropdownMenuItem
            key={action.label}
            icon={action.icon}
            onClick={action.onClick}
          >
            {action.label}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// SettingsInput - A text input field
export function SettingsInput({
  id,
  value,
  onChange,
  placeholder,
  error,
  disabled,
}: {
  id?: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  error?: boolean;
  disabled?: boolean;
}) {
  return (
    <Input
      id={id}
      type="text"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      disabled={disabled}
      invalid={error}
    />
  );
}

// SettingsTextarea - A multi-line text input
export function SettingsTextarea({
  value,
  onChange,
  placeholder,
  disabled,
  rows = 4,
  monospace = false,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  rows?: number;
  monospace?: boolean;
}) {
  return (
    <textarea
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      disabled={disabled}
      rows={rows}
      className={cn(
        'w-full min-w-0 rounded-[var(--vk-input-radius)] border border-[var(--vk-input-border)] bg-[var(--vk-input-surface)] px-3 py-2 text-[13px] leading-relaxed text-[var(--vk-input-text)]',
        'placeholder:text-[var(--vk-input-placeholder)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--vk-focus-ring)]',
        'resize-y',
        monospace && 'font-mono',
        disabled && 'opacity-50 cursor-not-allowed'
      )}
    />
  );
}

// SettingsSaveBar - A sticky save bar for unsaved changes
export function SettingsSaveBar({
  show,
  saving,
  saveDisabled,
  unsavedMessage,
  onSave,
  onDiscard,
}: {
  show: boolean;
  saving: boolean;
  saveDisabled?: boolean;
  unsavedMessage?: string;
  onSave: () => void;
  onDiscard?: () => void;
}) {
  const { t } = useTranslation(['settings', 'common']);

  if (!show) {
    return null;
  }

  return (
    <div className="vk-settings-save-bar sticky bottom-4 z-[var(--vk-z-sticky)] mt-6 rounded-[var(--vk-radius-md)] border border-[var(--vk-border-subtle)] bg-[var(--vk-surface-elevated)] p-3 shadow-[var(--vk-shadow-popover)]">
      <div
        className={cn(
          'flex flex-wrap items-center gap-3',
          onDiscard ? 'justify-between' : 'justify-end'
        )}
      >
        {onDiscard && (
          <span className="min-w-0 text-[13px] leading-5 text-[var(--vk-text-low)]">
            {unsavedMessage ?? t('settings.common.unsavedChanges')}
          </span>
        )}
        <div className="ml-auto flex max-w-full flex-wrap justify-end gap-2">
          {onDiscard && (
            <Button
              type="button"
              variant="ghost"
              onClick={onDiscard}
              disabled={saving}
            >
              {t('common:buttons.discard')}
            </Button>
          )}
          <Button
            type="button"
            onClick={onSave}
            disabled={saveDisabled}
            loading={saving}
          >
            {t('common:buttons.save')}
          </Button>
        </div>
      </div>
    </div>
  );
}
