import {
  useEffect,
  useId,
  useRef,
  useState,
  type ReactElement,
  type Ref,
} from 'react';
import { useTranslation } from 'react-i18next';
import { CaretDownIcon } from '@phosphor-icons/react';
import { cn } from '../lib/cn';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSearchInput,
  DropdownMenuTrigger,
} from './Dropdown';
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from './Accordion';
import { ModelProviderIcon } from './ModelProviderIcon';
import {
  ModelList,
  matchesModelSearch,
  type ModelListModel,
} from './ModelList';

interface ModelSelectorProvider {
  id: string;
  name: string;
}

interface ModelSelectorConfigLike {
  models: ModelListModel[];
  providers: ModelSelectorProvider[];
  catalog_source?: 'native' | 'observed' | 'configured' | 'builtin';
}

export interface ModelSelectorPopoverProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  trigger: ReactElement;
  config: ModelSelectorConfigLike;
  error?: string | null;
  onRetry?: () => void;
  refreshing?: boolean;
  onManualModelSelect?: (id: string) => void;
  selectedProviderId: string | null;
  selectedModelId: string | null;
  selectedReasoningId: string | null;
  followCliReasoningLabel?: string;
  inheritedModelLabel?: string | null;
  searchQuery: string;
  onSearchChange: (value: string) => void;
  onModelSelect: (id: string, providerId?: string) => void;
  onReasoningSelect: (reasoningId: string | null) => void;
  recentModelEntries?: string[];
  showDefaultOption?: boolean;
  onSelectDefault?: () => void;
  scrollRef?: Ref<HTMLDivElement>;
  expandedProviderId?: string;
  onExpandedProviderIdChange?: (id: string) => void;
  resolvedTheme?: 'light' | 'dark';
}

const MODEL_LIST_PAGE_SIZE = 8;

function getModelKey(model: ModelListModel): string {
  return model.provider_id ? `${model.provider_id}/${model.id}` : model.id;
}

function getModelSortLabel(model: ModelListModel): string {
  return model.display_name ?? (model.name || model.id);
}

function sortModelsAlphabetically(models: ModelListModel[]): ModelListModel[] {
  return [...models].sort((a, b) => {
    const labelComparison = getModelSortLabel(a).localeCompare(
      getModelSortLabel(b),
      undefined,
      {
        numeric: true,
        sensitivity: 'base',
      }
    );
    if (labelComparison !== 0) return labelComparison;

    return getModelKey(a).localeCompare(getModelKey(b), undefined, {
      numeric: true,
      sensitivity: 'base',
    });
  });
}

function getSelectedModel(
  models: ModelListModel[],
  selectedProviderId: string | null,
  selectedModelId: string | null
): ModelListModel | null {
  if (!selectedModelId) return null;
  if (selectedProviderId) {
    return (
      models.find(
        (model) =>
          model.id === selectedModelId &&
          model.provider_id === selectedProviderId
      ) ?? null
    );
  }
  const matches = models.filter((model) => model.id === selectedModelId);
  return matches.length === 1 ? matches[0] : null;
}

function getPopoverWidth(hasProviders: boolean, hasReasoning: boolean): string {
  if (hasProviders) return 'w-[280px]';
  if (hasReasoning) return 'w-[230px]';
  return 'w-[200px]';
}

interface ProviderAccordionProps {
  config: ModelSelectorConfigLike;
  selectedProviderId: string | null;
  selectedModelId: string | null;
  selectedReasoningId: string | null;
  followCliReasoningLabel?: string;
  inheritedModelLabel?: string | null;
  searchQuery: string;
  onModelSelect: (id: string, providerId?: string) => void;
  onReasoningSelect: (reasoningId: string | null) => void;
  showDefaultOption?: boolean;
  onSelectDefault?: () => void;
  scrollRef?: Ref<HTMLDivElement>;
  expandedProviderId: string;
  onExpandedProviderIdChange: (id: string) => void;
  resolvedTheme: 'light' | 'dark';
}

function ProviderAccordion({
  config,
  selectedProviderId,
  selectedModelId,
  selectedReasoningId,
  followCliReasoningLabel,
  inheritedModelLabel,
  searchQuery,
  onModelSelect,
  onReasoningSelect,
  showDefaultOption = false,
  onSelectDefault,
  scrollRef,
  expandedProviderId,
  onExpandedProviderIdChange,
  resolvedTheme,
}: ProviderAccordionProps) {
  const { t } = useTranslation('common');
  const normalizedSearch = searchQuery.trim().toLowerCase();
  const selectedModel = getSelectedModel(
    config.models,
    selectedProviderId,
    selectedModelId
  );

  const modelsByProvider = new Map<string, ModelListModel[]>();
  for (const model of config.models) {
    if (!model.provider_id) continue;
    const list = modelsByProvider.get(model.provider_id) ?? [];
    list.push(model);
    modelsByProvider.set(model.provider_id, list);
  }

  const isDefaultSelected = selectedModelId === null;
  const providers = config.providers;

  return (
    <div
      ref={scrollRef}
      className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden"
    >
      <div className="flex flex-col py-half">
        <Accordion
          type="single"
          collapsible
          value={expandedProviderId}
          onValueChange={onExpandedProviderIdChange}
        >
          {providers.map((provider) => {
            const providerModels = sortModelsAlphabetically(
              modelsByProvider.get(provider.id) ?? []
            );
            const isSelectedProvider =
              Boolean(selectedModelId) &&
              selectedModel?.provider_id === provider.id;

            if (
              normalizedSearch &&
              !providerModels.some((model) =>
                matchesModelSearch(model, normalizedSearch)
              )
            ) {
              return null;
            }

            return (
              <AccordionItem key={provider.id} value={provider.id}>
                <AccordionTrigger
                  sticky={provider.id === expandedProviderId}
                  className={cn(
                    'group gap-2 px-base py-half rounded-sm',
                    'text-sm font-medium text-low',
                    'hover:bg-secondary/60 transition-colors',
                    'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand'
                  )}
                >
                  <ModelProviderIcon
                    providerId={provider.id}
                    theme={resolvedTheme}
                  />
                  <span className="flex-1 text-left truncate">
                    {provider.name}
                  </span>
                  <CaretDownIcon
                    className={cn(
                      'size-icon-2xs text-low transition-transform',
                      'group-data-[state=open]:rotate-180'
                    )}
                    weight="bold"
                  />
                </AccordionTrigger>
                <AccordionContent>
                  <div className="pl-1">
                    <ModelList
                      models={providerModels}
                      selectedModelId={
                        isSelectedProvider ? selectedModelId : null
                      }
                      searchQuery={searchQuery}
                      onSelect={onModelSelect}
                      reasoningOptions={
                        isSelectedProvider
                          ? (selectedModel?.reasoning_options ?? [])
                          : []
                      }
                      selectedReasoningId={
                        isSelectedProvider ? selectedReasoningId : null
                      }
                      onReasoningSelect={onReasoningSelect}
                      followCliReasoningLabel={followCliReasoningLabel}
                      justifyEnd={false}
                    />
                  </div>
                </AccordionContent>
              </AccordionItem>
            );
          })}
        </Accordion>
        {showDefaultOption && (
          <div
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
                  ? t('modelSelector.followCliValue', {
                      value: inheritedModelLabel,
                    })
                  : t('modelSelector.followCliConfig')}
              </span>
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

export function ModelSelectorPopover({
  isOpen,
  onOpenChange,
  trigger,
  config,
  error,
  onRetry,
  refreshing = false,
  onManualModelSelect,
  selectedProviderId,
  selectedModelId,
  selectedReasoningId,
  followCliReasoningLabel,
  inheritedModelLabel,
  searchQuery,
  onSearchChange,
  onModelSelect,
  onReasoningSelect,
  showDefaultOption = false,
  onSelectDefault,
  scrollRef,
  expandedProviderId = '',
  onExpandedProviderIdChange,
  resolvedTheme = 'light',
}: ModelSelectorPopoverProps) {
  const { t } = useTranslation('common');
  const manualId = useId();
  const manualToggleRef = useRef<HTMLButtonElement>(null);
  const manualInputRef = useRef<HTMLInputElement>(null);
  const [isManualOpen, setIsManualOpen] = useState(false);
  const [manualModelId, setManualModelId] = useState('');
  const [manualError, setManualError] = useState(false);

  useEffect(() => {
    if (isOpen) return;
    setIsManualOpen(false);
    setManualModelId('');
    setManualError(false);
  }, [isOpen]);

  useEffect(() => {
    if (isManualOpen) manualInputRef.current?.focus();
  }, [isManualOpen]);

  const closeManualEntry = () => {
    setIsManualOpen(false);
    setManualModelId('');
    setManualError(false);
    manualToggleRef.current?.focus();
  };
  const applyManualModel = () => {
    if (!manualModelId.trim()) {
      setManualError(true);
      manualInputRef.current?.focus();
      return;
    }
    // Only reject blank input. Provider-owned executable IDs stay verbatim.
    onManualModelSelect?.(manualModelId);
    onOpenChange(false);
  };
  const models = config.models;
  const hasProviders = config.providers.length > 1;
  const hasReasoning = models.some(
    (model) => model.reasoning_options.length > 0
  );
  const popoverWidth = onManualModelSelect
    ? 'w-[280px] max-w-[calc(100vw-24px)]'
    : getPopoverWidth(hasProviders, hasReasoning);
  const popoverHeightClass = hasProviders ? 'h-[280px]' : '';

  let showSearch = true;
  let content: ReactElement;

  if (hasProviders) {
    content = (
      <ProviderAccordion
        config={config}
        selectedProviderId={selectedProviderId}
        selectedModelId={selectedModelId}
        selectedReasoningId={selectedReasoningId}
        followCliReasoningLabel={followCliReasoningLabel}
        inheritedModelLabel={inheritedModelLabel}
        searchQuery={searchQuery}
        onModelSelect={onModelSelect}
        onReasoningSelect={onReasoningSelect}
        showDefaultOption={showDefaultOption}
        onSelectDefault={onSelectDefault}
        scrollRef={scrollRef}
        expandedProviderId={expandedProviderId}
        onExpandedProviderIdChange={onExpandedProviderIdChange ?? (() => {})}
        resolvedTheme={resolvedTheme}
      />
    );
  } else {
    const sortedModels = sortModelsAlphabetically(models);
    const selectedModel = getSelectedModel(
      models,
      selectedProviderId,
      selectedModelId
    );
    showSearch = models.length > MODEL_LIST_PAGE_SIZE;

    content = (
      <ModelList
        models={sortedModels}
        selectedModelId={selectedModelId}
        searchQuery={searchQuery}
        onSelect={onModelSelect}
        reasoningOptions={selectedModel?.reasoning_options ?? []}
        selectedReasoningId={selectedReasoningId}
        followCliReasoningLabel={followCliReasoningLabel}
        inheritedModelLabel={inheritedModelLabel}
        onReasoningSelect={onReasoningSelect}
        justifyEnd
        className="max-h-[233px]"
        showDefaultOption={showDefaultOption}
        onSelectDefault={onSelectDefault}
        scrollRef={scrollRef}
      />
    );
  }

  return (
    <DropdownMenu open={isOpen} onOpenChange={onOpenChange}>
      <DropdownMenuTrigger asChild>{trigger}</DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        sideOffset={8}
        data-model-selector-popover
        className={cn(
          'p-0 overflow-hidden flex flex-col',
          popoverWidth,
          popoverHeightClass
        )}
        onInteractOutside={(event) => {
          const target = event.target as HTMLElement | null;
          if (target?.closest('[data-model-selector-dropdown]')) {
            event.preventDefault();
          }
        }}
        onEscapeKeyDown={(event) => {
          // Escape belongs to this picker, never to the surrounding chat.
          event.stopPropagation();
          if (isManualOpen) {
            event.preventDefault();
            closeManualEntry();
          }
        }}
      >
        <div className="flex flex-1 flex-col min-h-0 overflow-hidden">
          {error && (
            <div className="shrink-0 border-b border-border bg-error/10 px-base py-half">
              <p
                role="alert"
                className="text-sm text-error whitespace-normal [overflow-wrap:anywhere]"
              >
                {error}
              </p>
              {onRetry && (
                <button
                  type="button"
                  onClick={onRetry}
                  disabled={refreshing}
                  className="mt-half rounded-sm text-sm text-normal underline underline-offset-2 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand disabled:opacity-50"
                >
                  {t('modelSelector.refreshDiscovery')}
                </button>
              )}
            </div>
          )}
          <DropdownMenuLabel>{t('modelSelector.model')}</DropdownMenuLabel>
          {config.catalog_source === 'builtin' && (
            <p role="status" className="px-base pb-half text-sm text-low">
              {t('modelSelector.builtinCatalog')}
            </p>
          )}
          {config.catalog_source === 'configured' && (
            <p role="status" className="px-base pb-half text-sm text-low">
              {t('modelSelector.configuredCatalog')}
            </p>
          )}
          <div className="flex flex-col flex-1 min-h-0 min-w-0">
            {content}
            {showSearch && (
              <div className="border-t border-border">
                <DropdownMenuSearchInput
                  placeholder="Filter by name or ID..."
                  value={searchQuery}
                  onValueChange={onSearchChange}
                />
              </div>
            )}
            {onManualModelSelect && (
              <div className="shrink-0 border-t border-border px-base py-half">
                <button
                  ref={manualToggleRef}
                  type="button"
                  aria-expanded={isManualOpen}
                  aria-controls={`${manualId}-entry`}
                  onClick={() => {
                    setIsManualOpen((open) => !open);
                    setManualModelId(
                      selectedProviderId
                        ? `${selectedProviderId}/${selectedModelId ?? ''}`
                        : (selectedModelId ?? '')
                    );
                    setManualError(false);
                  }}
                  className="rounded-sm text-sm text-normal focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand"
                >
                  {t('modelSelector.enterModelId')}
                </button>
                {isManualOpen && (
                  <div
                    id={`${manualId}-entry`}
                    role="group"
                    aria-labelledby={`${manualId}-label`}
                    className="mt-half flex flex-col gap-half"
                    onKeyDown={(event) => {
                      if (event.key !== 'Tab') event.stopPropagation();
                    }}
                  >
                    <label
                      id={`${manualId}-label`}
                      htmlFor={manualId}
                      className="text-sm text-normal"
                    >
                      {t('modelSelector.modelId')}
                    </label>
                    <input
                      ref={manualInputRef}
                      id={manualId}
                      type="text"
                      value={manualModelId}
                      onChange={(event) => {
                        setManualModelId(event.target.value);
                        setManualError(false);
                      }}
                      onKeyDown={(event) => {
                        if (
                          event.key === 'Enter' &&
                          !event.nativeEvent.isComposing
                        ) {
                          event.preventDefault();
                          event.stopPropagation();
                          applyManualModel();
                        }
                      }}
                      aria-invalid={manualError || undefined}
                      aria-describedby={`${manualId}-help${manualError ? ` ${manualId}-error` : ''}`}
                      autoComplete="off"
                      spellCheck={false}
                      className="min-w-0 rounded-sm border border-border bg-secondary px-half py-half text-sm text-normal focus:outline-none focus:ring-1 focus:ring-brand"
                    />
                    <p
                      id={`${manualId}-help`}
                      className="text-sm text-low whitespace-normal"
                    >
                      {t('modelSelector.manualModelHint')}
                    </p>
                    {manualError && (
                      <p
                        id={`${manualId}-error`}
                        role="alert"
                        className="text-sm text-error"
                      >
                        {t('modelSelector.modelIdRequired')}
                      </p>
                    )}
                    <div className="flex justify-end gap-base">
                      <button
                        type="button"
                        onClick={closeManualEntry}
                        className="rounded-sm text-sm text-low focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand"
                      >
                        {t('buttons.cancel')}
                      </button>
                      <button
                        type="button"
                        onClick={applyManualModel}
                        className="rounded-sm px-half py-half text-sm text-normal bg-secondary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand"
                      >
                        {t('modelSelector.useModel')}
                      </button>
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
