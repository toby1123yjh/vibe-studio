import { Fragment, useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useLexicalComposerContext } from '@lexical/react/LexicalComposerContext';
import {
  LexicalTypeaheadMenuPlugin,
  MenuOption,
} from '@lexical/react/LexicalTypeaheadMenuPlugin';
import {
  $createTextNode,
  KEY_ESCAPE_COMMAND,
  COMMAND_PRIORITY_NORMAL,
} from 'lexical';
import {
  CheckCircleIcon,
  InfoIcon,
  SparkleIcon,
  TerminalIcon,
  WarningCircleIcon,
  WrenchIcon,
  type Icon,
} from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { useTypeaheadOpen } from './TypeaheadOpenContext';
import { TypeaheadMenu } from './TypeaheadMenu';

type SlashCommandSource =
  | 'builtin'
  | 'skill'
  | 'plugin'
  | 'custom'
  | 'fallback';

type SlashCommandSupportLevel =
  | 'product'
  | 'native'
  | 'skill'
  | 'custom'
  | 'diagnostic'
  | 'unsupported'
  | 'fallback';

type SlashCommandGroupKey =
  | 'product'
  | 'skill'
  | 'native'
  | 'custom'
  | 'diagnostic'
  | 'fallback'
  | 'unsupported'
  | 'other';

type SlashCommandGroupMeta = {
  label: string;
  badge: string;
  title: string;
  Icon: Icon;
};

export type SlashCommandDescriptionLike = {
  name: string;
  description?: string | null;
  source?: SlashCommandSource | null;
  support_level?: SlashCommandSupportLevel | null;
};

class SlashCommandOption extends MenuOption {
  command: SlashCommandDescriptionLike;

  constructor(command: SlashCommandDescriptionLike) {
    super(`slash-command-${command.name}`);
    this.command = command;
  }
}

const SLASH_COMMAND_GROUP_ORDER: SlashCommandGroupKey[] = [
  'product',
  'skill',
  'native',
  'custom',
  'diagnostic',
  'fallback',
  'unsupported',
  'other',
];

const SLASH_COMMAND_GROUP_META: Record<
  SlashCommandGroupKey,
  SlashCommandGroupMeta
> = {
  product: {
    label: '常用命令',
    badge: 'VS',
    title: 'Vibe Studio 认可的产品入口',
    Icon: CheckCircleIcon,
  },
  skill: {
    label: '技能',
    badge: 'Skill',
    title: '来自智能体技能',
    Icon: SparkleIcon,
  },
  native: {
    label: '原生命令',
    badge: '原生',
    title: '当前智能体后端的原生命令',
    Icon: TerminalIcon,
  },
  custom: {
    label: '自定义命令',
    badge: 'Custom',
    title: '来自自定义命令或 plugin',
    Icon: WrenchIcon,
  },
  diagnostic: {
    label: '诊断',
    badge: 'Diag',
    title: '诊断或调试命令',
    Icon: WarningCircleIcon,
  },
  fallback: {
    label: '备用命令',
    badge: 'Fallback',
    title: '来自 fallback 列表，可信度低于实时发现',
    Icon: InfoIcon,
  },
  unsupported: {
    label: '未适配',
    badge: '未适配',
    title: '未作为常规产品能力适配',
    Icon: WarningCircleIcon,
  },
  other: {
    label: '命令',
    badge: 'Agent',
    title: '当前智能体后端提供的命令',
    Icon: TerminalIcon,
  },
};

function filterSlashCommands(
  all: SlashCommandDescriptionLike[],
  query: string
): SlashCommandDescriptionLike[] {
  const q = query.trim().toLowerCase();
  if (!q) return all;

  const startsWith = all.filter((c) => c.name.toLowerCase().startsWith(q));
  const includes = all.filter(
    (c) => !startsWith.includes(c) && c.name.toLowerCase().includes(q)
  );
  return [...startsWith, ...includes];
}

function getSlashCommandGroupKey(
  command: SlashCommandDescriptionLike
): SlashCommandGroupKey {
  const level =
    command.support_level ??
    (command.source === 'skill'
      ? 'skill'
      : command.source === 'plugin' || command.source === 'custom'
        ? 'custom'
        : command.source === 'fallback'
          ? 'fallback'
          : null);

  switch (level) {
    case 'product':
      return 'product';
    case 'native':
      return 'native';
    case 'skill':
      return 'skill';
    case 'custom':
      return 'custom';
    case 'diagnostic':
      return 'diagnostic';
    case 'unsupported':
      return 'unsupported';
    case 'fallback':
      return 'fallback';
    default:
      return 'other';
  }
}

function getSlashCommandGroupMeta(command: SlashCommandDescriptionLike) {
  return SLASH_COMMAND_GROUP_META[getSlashCommandGroupKey(command)];
}

function getSlashCommandGroupPriority(command: SlashCommandDescriptionLike) {
  return SLASH_COMMAND_GROUP_ORDER.indexOf(getSlashCommandGroupKey(command));
}

function orderSlashCommandsForDisplay(
  commands: SlashCommandDescriptionLike[]
): SlashCommandDescriptionLike[] {
  return [...commands].sort((a, b) => {
    const priorityDiff =
      getSlashCommandGroupPriority(a) - getSlashCommandGroupPriority(b);
    if (priorityDiff !== 0) return priorityDiff;

    return commands.indexOf(a) - commands.indexOf(b);
  });
}

function groupSlashCommandOptions(options: SlashCommandOption[]) {
  return SLASH_COMMAND_GROUP_ORDER.map((key) => ({
    key,
    meta: SLASH_COMMAND_GROUP_META[key],
    items: options
      .map((option, index) => ({ option, index }))
      .filter(({ option }) => getSlashCommandGroupKey(option.command) === key),
  })).filter((group) => group.items.length > 0);
}

export function SlashCommandTypeaheadPlugin({
  enabled,
  commands,
  isInitialized,
  isDiscovering,
}: {
  enabled: boolean;
  commands: SlashCommandDescriptionLike[];
  isInitialized: boolean;
  isDiscovering: boolean;
}) {
  const [editor] = useLexicalComposerContext();
  const { t } = useTranslation('common');
  const { setIsOpen } = useTypeaheadOpen();
  const [options, setOptions] = useState<SlashCommandOption[]>([]);
  const [activeQuery, setActiveQuery] = useState<string | null>(null);
  const closeTypeahead = useCallback(() => {
    editor.dispatchCommand(KEY_ESCAPE_COMMAND, new KeyboardEvent('keydown'));
  }, [editor]);

  const isLoading = !isInitialized && enabled;

  const updateOptions = useCallback(
    (query: string | null) => {
      setActiveQuery(query);

      if (!enabled || query === null) {
        setOptions([]);
        return;
      }

      const filtered = orderSlashCommandsForDisplay(
        filterSlashCommands(commands, query)
      ).slice(0, 20);
      setOptions(filtered.map((c) => new SlashCommandOption(c)));
    },
    [enabled, commands]
  );

  const hasVisibleResults = useMemo(() => {
    if (!enabled || activeQuery === null) return false;
    if (isLoading || isDiscovering) return true;
    if (!activeQuery.trim()) return true;
    return options.length > 0;
  }, [enabled, activeQuery, isDiscovering, isLoading, options.length]);

  useEffect(() => {
    if (activeQuery === null) return;
    updateOptions(activeQuery);
  }, [activeQuery, updateOptions]);

  return (
    <LexicalTypeaheadMenuPlugin<SlashCommandOption>
      commandPriority={COMMAND_PRIORITY_NORMAL}
      triggerFn={(text) => {
        const match = /^(\s*)\/([^\s/]*)$/.exec(text);
        if (!match) return null;

        const slashOffset = match[1].length;
        return {
          leadOffset: slashOffset,
          matchingString: match[2],
          replaceableString: match[0].slice(slashOffset),
        };
      }}
      options={options}
      onQueryChange={updateOptions}
      onOpen={() => setIsOpen(true)}
      onClose={() => setIsOpen(false)}
      onSelectOption={(option, nodeToReplace, closeMenu) => {
        editor.update(() => {
          if (!nodeToReplace) return;

          const textToInsert = `/${option.command.name}`;
          const commandNode = $createTextNode(textToInsert);
          nodeToReplace.replace(commandNode);

          const spaceNode = $createTextNode(' ');
          commandNode.insertAfter(spaceNode);
          spaceNode.select(1, 1);
        });

        closeMenu();
      }}
      menuRenderFn={(
        anchorRef,
        { selectedIndex, selectOptionAndCleanUp, setHighlightedIndex }
      ) => {
        if (!anchorRef.current) return null;
        if (!enabled) return null;
        if (!hasVisibleResults) return null;

        const isEmpty = !isLoading && !isDiscovering && commands.length === 0;
        const showLoadingRow = isLoading || isDiscovering;
        const loadingText = isLoading ? '正在加载命令...' : '正在发现命令...';
        const optionGroups = groupSlashCommandOptions(options);

        return createPortal(
          <TypeaheadMenu
            anchorEl={anchorRef.current}
            editorEl={editor.getRootElement()}
            onClickOutside={closeTypeahead}
          >
            <div className="w-[32rem] max-w-[calc(100vw-2rem)]">
              <TypeaheadMenu.Header>
                <TerminalIcon className="size-icon-xs" weight="bold" />
                {t('typeahead.commands')}
              </TypeaheadMenu.Header>

              {isEmpty ? (
                <TypeaheadMenu.Empty>
                  {t('typeahead.noCommands')}
                </TypeaheadMenu.Empty>
              ) : options.length === 0 && !showLoadingRow ? null : (
                <TypeaheadMenu.ScrollArea>
                  {showLoadingRow && (
                    <div className="px-base py-half text-sm text-low select-none">
                      {loadingText}
                    </div>
                  )}
                  {optionGroups.map((group, groupIndex) => (
                    <Fragment key={group.key}>
                      {groupIndex > 0 && <TypeaheadMenu.Divider />}
                      <TypeaheadMenu.SectionHeader>
                        {group.meta.label}
                      </TypeaheadMenu.SectionHeader>
                      {group.items.map(({ option, index }) => {
                        const details = option.command.description ?? null;
                        const meta = getSlashCommandGroupMeta(option.command);
                        const CommandIcon = meta.Icon;

                        return (
                          <TypeaheadMenu.Item
                            key={option.key}
                            isSelected={index === selectedIndex}
                            index={index}
                            setHighlightedIndex={setHighlightedIndex}
                            onClick={() => selectOptionAndCleanUp(option)}
                          >
                            <div className="flex min-w-0 items-start gap-base">
                              <div className="mt-[2px] flex h-6 w-6 shrink-0 items-center justify-center rounded-sm border border-border bg-secondary text-low">
                                <CommandIcon
                                  className="size-icon-xs"
                                  weight="bold"
                                />
                              </div>
                              <div className="min-w-0 flex-1">
                                <div className="flex min-w-0 items-center justify-between gap-base font-medium">
                                  <span className="min-w-0 truncate font-mono">
                                    /{option.command.name}
                                  </span>
                                  <span
                                    className="shrink-0 rounded-sm border border-border bg-panel px-half py-[1px] text-[10px] leading-4 text-low"
                                    title={meta.title}
                                  >
                                    {meta.badge}
                                  </span>
                                </div>
                                {details && (
                                  <div className="mt-[1px] truncate text-xs text-low">
                                    {details}
                                  </div>
                                )}
                              </div>
                            </div>
                          </TypeaheadMenu.Item>
                        );
                      })}
                    </Fragment>
                  ))}
                  {options.length > 0 && optionGroups.length === 0 && (
                    <TypeaheadMenu.Empty>
                      {t('typeahead.noCommands')}
                    </TypeaheadMenu.Empty>
                  )}
                </TypeaheadMenu.ScrollArea>
              )}
            </div>
          </TypeaheadMenu>,
          document.body
        );
      }}
    />
  );
}
