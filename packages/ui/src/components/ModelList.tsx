import type { Ref } from 'react';
import { useTranslation } from 'react-i18next';
import { BrainIcon, CaretDownIcon, CheckIcon } from '@phosphor-icons/react';
import { cn } from '../lib/cn';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from './Dropdown';

export interface ModelReasoningOption {
  id: string;
  label: string;
  is_default?: boolean;
}

export interface ModelListModel {
  id: string;
  name: string;
  /** Presentation only. Selection, recency and keys always use id/provider_id. */
  display_name?: string;
  description?: string;
  provider_id?: string | null;
  reasoning_options: ModelReasoningOption[];
}

function toPrettyCase(value: string): string {
  return value
    .split('_')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
    .join(' ');
}

function getReasoningLabel(
  options: ModelReasoningOption[],
  selectedId: string | null
): string | null {
  if (selectedId == null) return null;
  return (
    options.find((option) => option.id === selectedId)?.label ??
    toPrettyCase(selectedId)
  );
}

function getModelKey(model: ModelListModel): string {
  return model.provider_id ? `${model.provider_id}/${model.id}` : model.id;
}

export function matchesModelSearch(
  model: ModelListModel,
  normalizedQuery: string
): boolean {
  return [model.display_name, model.name, getModelKey(model)].some((value) =>
    value?.toLowerCase().includes(normalizedQuery)
  );
}

interface ReasoningDropdownProps {
  options: ModelReasoningOption[];
  selectedId: string | null;
  onSelect: (reasoningId: string | null) => void;
  followCliLabel?: string;
}

function ReasoningDropdown({
  options,
  selectedId,
  onSelect,
  followCliLabel,
}: ReasoningDropdownProps) {
  const { t } = useTranslation('common');
  if (!options.length) return null;

  const selectedLabel =
    getReasoningLabel(options, selectedId) ??
    followCliLabel ??
    t('modelSelector.followCliConfig');
  const isDefaultSelected = selectedId === null;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className={cn(
            'inline-flex items-center gap-1 rounded-sm border border-border',
            'bg-secondary/60 px-1.5 py-0.5 text-[10px] font-semibold text-low',
            'hover:border-brand/40 hover:text-normal transition-colors',
            'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand'
          )}
        >
          <BrainIcon className="size-icon-xs" weight="fill" />
          <span
            className="max-w-[90px] whitespace-normal [overflow-wrap:anywhere]"
            title={selectedLabel}
          >
            {selectedLabel}
          </span>
          <CaretDownIcon className="size-icon-2xs" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        data-model-selector-dropdown
        className="min-w-[160px]"
      >
        <DropdownMenuItem
          icon={isDefaultSelected ? CheckIcon : undefined}
          onClick={() => onSelect(null)}
        >
          {followCliLabel ?? t('modelSelector.followCliConfig')}
        </DropdownMenuItem>
        {options.map((option) => (
          <DropdownMenuItem
            key={option.id}
            icon={option.id === selectedId ? CheckIcon : undefined}
            onClick={() => onSelect(option.id)}
          >
            {option.label}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export interface ModelListProps {
  models: ModelListModel[];
  selectedModelId: string | null;
  searchQuery: string;
  onSelect: (id: string, providerId?: string) => void;
  reasoningOptions: ModelReasoningOption[];
  selectedReasoningId: string | null;
  onReasoningSelect: (reasoningId: string | null) => void;
  followCliReasoningLabel?: string;
  inheritedModelLabel?: string | null;
  justifyEnd?: boolean;
  className?: string;
  showDefaultOption?: boolean;
  onSelectDefault?: () => void;
  scrollRef?: Ref<HTMLDivElement>;
}

export function ModelList({
  models,
  selectedModelId,
  searchQuery,
  onSelect,
  reasoningOptions,
  selectedReasoningId,
  onReasoningSelect,
  followCliReasoningLabel,
  inheritedModelLabel,
  justifyEnd = false,
  className,
  showDefaultOption = false,
  onSelectDefault,
  scrollRef,
}: ModelListProps) {
  const { t } = useTranslation('common');
  const normalizedSearch = searchQuery.trim().toLowerCase();

  const filteredModels = normalizedSearch
    ? models.filter((model) => matchesModelSearch(model, normalizedSearch))
    : models;

  const showEmptyState = filteredModels.length === 0 && !showDefaultOption;
  const isDefaultSelected = selectedModelId === null;

  const defaultRow = showDefaultOption ? (
    <div
      key="__default__"
      className={cn(
        'group flex items-center rounded-sm mx-half',
        'transition-colors duration-100',
        'focus-within:bg-secondary',
        isDefaultSelected
          ? 'bg-secondary text-high'
          : cn('text-normal', 'hover:bg-secondary/60')
      )}
    >
      <button
        type="button"
        onClick={() => onSelectDefault?.()}
        className={cn(
          'flex-1 min-w-0 py-half pl-base pr-half text-left',
          'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand'
        )}
      >
        <span
          className={cn(
            'block text-sm whitespace-normal [overflow-wrap:anywhere]',
            isDefaultSelected && 'font-semibold'
          )}
        >
          {inheritedModelLabel
            ? t('modelSelector.followCliValue', { value: inheritedModelLabel })
            : t('modelSelector.followCliConfig')}
        </span>
      </button>
    </div>
  ) : null;

  return (
    <div
      ref={scrollRef}
      className={cn(
        'flex-1 min-h-0 overflow-y-auto overflow-x-hidden',
        className
      )}
    >
      {showEmptyState ? (
        <div className="flex h-full items-center justify-center px-base text-sm text-low">
          {normalizedSearch ? 'No matches.' : 'No models available.'}
        </div>
      ) : (
        <div
          className={cn(
            'flex min-h-full flex-col',
            justifyEnd && 'justify-end'
          )}
        >
          {filteredModels.map((model) => {
            const modelKey = getModelKey(model);
            const displayName = model.display_name ?? modelKey;
            const isSelected =
              selectedModelId !== null && model.id === selectedModelId;
            const isReasoningConfigurable = model.reasoning_options.length > 0;
            const showReasoningSelector =
              isSelected &&
              isReasoningConfigurable &&
              reasoningOptions.length > 0;

            return (
              <div
                key={`${model.provider_id ?? 'default'}/${model.id}`}
                data-model-key={modelKey}
                data-model-id={model.id}
                data-provider-id={model.provider_id ?? ''}
                className={cn(
                  'group flex items-center rounded-sm mx-half',
                  'transition-colors duration-100',
                  'focus-within:bg-secondary',
                  isSelected
                    ? 'bg-secondary text-high'
                    : cn('text-normal', 'hover:bg-secondary/60')
                )}
              >
                <button
                  type="button"
                  onClick={() =>
                    onSelect(model.id, model.provider_id ?? undefined)
                  }
                  className={cn(
                    'flex-1 min-w-0 py-half pl-base pr-half text-left',
                    'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand'
                  )}
                >
                  <span
                    className={cn(
                      'block text-sm whitespace-normal [overflow-wrap:anywhere]',
                      isSelected && 'font-semibold'
                    )}
                    title={displayName}
                  >
                    {displayName}
                  </span>
                  {model.description && (
                    <span className="block text-sm text-low whitespace-normal [overflow-wrap:anywhere]">
                      {model.description}
                    </span>
                  )}
                </button>
                <div className="flex items-center justify-end gap-half pr-base">
                  {showReasoningSelector && (
                    <ReasoningDropdown
                      options={reasoningOptions}
                      selectedId={selectedReasoningId}
                      onSelect={onReasoningSelect}
                      followCliLabel={followCliReasoningLabel}
                    />
                  )}
                  {!showReasoningSelector && isReasoningConfigurable ? (
                    <span
                      className={cn(
                        'inline-flex items-center justify-center',
                        'size-5 rounded-sm bg-border/80 text-normal',
                        'dark:bg-secondary/70'
                      )}
                      title="Reasoning supported"
                    >
                      <BrainIcon className="size-icon-xs" weight="fill" />
                    </span>
                  ) : null}
                </div>
              </div>
            );
          })}
          {defaultRow}
        </div>
      )}
    </div>
  );
}
