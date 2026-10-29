import {
  memo,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react';
import { useTranslation } from 'react-i18next';
import {
  FileTextIcon,
  InfoIcon,
  ListMagnifyingGlassIcon,
} from '@phosphor-icons/react';
import type { ConversationBlock } from 'shared/types';
import type { ToolStatusLike } from '@vibe/ui/components/ToolStatusDot';
import {
  ChatDisclosure,
  ChatDisclosureBody,
} from '@vibe/ui/components/ChatDisclosure';
import { ChatAssistantMessage } from '@vibe/ui/components/ChatAssistantMessage';
import { ChatUserMessage } from '@vibe/ui/components/ChatUserMessage';
import { ChatThinkingMessage } from '@vibe/ui/components/ChatThinkingMessage';
import { ChatToolSummary } from '@vibe/ui/components/ChatToolSummary';
import { ChatTodoList } from '@vibe/ui/components/ChatTodoList';
import { ChatApprovalCard } from '@vibe/ui/components/ChatApprovalCard';
import { Button } from '@vibe/ui/components/Button';
import { formatPathForDisplay } from '@vibe/ui/lib/path';
import { usePersistedExpanded } from '@/shared/stores/useUiPreferencesStore';
import { agentConversationApi } from '@/shared/lib/agentConversationApi';
import {
  useAgentConversationStore,
  useConversationBlock,
  type AgentRunSessionContextValue,
} from '@/features/agent-runtime/model/AgentRunSessionContext';
import { isCanonicalRunActive } from '@/features/agent-runtime/model/canonicalAgentSessionTimeline';
import { segmentStreamingMarkdown } from '@/features/agent-runtime/model/streamingMarkdown';
import { AppChatMarkdown } from './AppChatMarkdown';

const StableMarkdown = memo(AppChatMarkdown);

function StreamingMarkdown({
  content,
  finished,
  workspaceId,
  sessionId,
}: {
  content: string;
  finished: boolean;
  workspaceId?: string;
  sessionId?: string;
}) {
  const segments = useMemo(
    () => segmentStreamingMarkdown(content, finished),
    [content, finished]
  );
  return (
    <>
      {segments.map((segment) =>
        segment.provisional ? (
          <pre
            key={segment.offset}
            className="max-h-96 overflow-auto whitespace-pre-wrap break-words rounded bg-secondary p-base font-ibm-plex-mono text-sm"
          >
            {segment.content}
          </pre>
        ) : (
          <StableMarkdown
            key={segment.offset}
            content={segment.content}
            workspaceId={workspaceId}
            sessionId={sessionId}
            className={undefined}
            maxWidth={undefined}
          />
        )
      )}
    </>
  );
}

function printable(value: unknown) {
  return typeof value === 'string'
    ? value
    : value == null
      ? ''
      : JSON.stringify(value, null, 2);
}

/** Detail chunks are requested only for a visible/expanded body. Keep at most
 * four chunks in the browser; the server retains the complete original body.
 */
function BlockBody({
  block,
  field,
  markdown = false,
  workspaceId,
  active = false,
}: {
  block: ConversationBlock;
  field: string;
  markdown?: boolean;
  workspaceId?: string;
  active?: boolean;
}) {
  const { t } = useTranslation('common');
  const store = useAgentConversationStore();
  const reference = block.bodies.find((item) => item.field === field);
  const preview = printable(block.fields[field as keyof typeof block.fields]);
  const [page, setPage] = useState<{
    revision: string;
    text: string;
    offset: number;
    next: number | null;
    total: number;
  } | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [followTail, setFollowTail] = useState(true);
  const controllerRef = useRef<AbortController | null>(null);
  useEffect(() => () => controllerRef.current?.abort(), []);
  const total = Number(reference?.byte_length ?? 0);
  // While following a growing tail retain the last bounded section until the
  // next chunk arrives. Ordinary snapshot replacement invalidates old pages.
  const currentPage =
    page?.revision === block.revision || (followTail && page && page.offset > 0)
      ? page
      : null;
  const content = currentPage?.text ?? preview;
  const previewBytes = new TextEncoder().encode(preview).byteLength;
  const next = currentPage
    ? currentPage.next
    : reference && Number(reference.byte_length) > previewBytes
      ? previewBytes
      : null;
  const liveRef = useRef({ block, total });
  liveRef.current = { block, total };
  const latestReadRef = useRef<string | null>(null);
  const longBody = total > previewBytes;
  const hasTailPage = Boolean(page?.offset);
  const settledRevision = active ? null : block.revision;
  useEffect(() => {
    if (!store || !followTail || !longBody || (!active && !hasTailPage)) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const abort = new AbortController();
    const readTail = async () => {
      const latest = liveRef.current;
      try {
        if (latestReadRef.current !== latest.block.revision) {
          const result = await agentConversationApi.detail(
            store.sessionId,
            store.hostId,
            block.block_id,
            field,
            Math.max(0, latest.total - 16384),
            abort.signal
          );
          if (disposed) return;
          store.acceptDetailBlock(result.block);
          latestReadRef.current = result.block.revision;
          setPage({
            revision: result.block.revision,
            text: result.content,
            offset: Number(result.offset),
            next:
              result.next_offset === null ? null : Number(result.next_offset),
            total: Number(result.total_length),
          });
        }
      } catch (failure) {
        if (!disposed)
          setError(
            failure instanceof Error ? failure.message : String(failure)
          );
      }
      if (!disposed && active) timer = setTimeout(() => void readTail(), 250);
    };
    void readTail();
    return () => {
      disposed = true;
      abort.abort();
      if (timer) clearTimeout(timer);
    };
    // Read from liveRef so each delta does not cancel/restart the bounded timer.
  }, [
    store,
    followTail,
    longBody,
    active,
    hasTailPage,
    settledRevision,
    block.block_id,
    field,
  ]);
  const load = async (fromStart = false) => {
    if (!store || (!fromStart && next === null) || loading) return;
    setFollowTail(false);
    const abort = new AbortController();
    controllerRef.current?.abort();
    controllerRef.current = abort;
    setLoading(true);
    setError(null);
    try {
      let result = await agentConversationApi.detail(
        store.sessionId,
        store.hostId,
        block.block_id,
        field,
        fromStart ? 0 : next!,
        abort.signal
      );
      if (abort.signal.aborted) return;
      store.acceptDetailBlock(result.block);
      // A new body may have changed before the old offset. Reload the beginning
      // rather than joining that offset to a different body revision.
      if (
        result.block.revision !== block.revision &&
        Number(result.offset) !== 0
      ) {
        result = await agentConversationApi.detail(
          store.sessionId,
          store.hostId,
          block.block_id,
          field,
          0,
          abort.signal
        );
        if (abort.signal.aborted) return;
        store.acceptDetailBlock(result.block);
      }
      const sameRevision = result.block.revision === block.revision;
      const previous =
        sameRevision && !fromStart && Number(result.offset) > 0
          ? (currentPage?.text ?? preview)
          : '';
      const merged = previous + result.content;
      const overWindow = merged.length > 131072;
      setPage({
        revision: result.block.revision,
        text: overWindow ? result.content : merged,
        offset: overWindow ? Number(result.offset) : (currentPage?.offset ?? 0),
        next: result.next_offset === null ? null : Number(result.next_offset),
        total: Number(result.total_length),
      });
    } catch (failure) {
      if (!abort.signal.aborted)
        setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      if (!abort.signal.aborted) setLoading(false);
    }
  };
  return (
    <div className="min-w-0">
      {currentPage && currentPage.offset > 0 && (
        <div className="mb-2 flex flex-wrap items-center gap-2 text-xs text-low">
          <span>
            {t('conversation.bodyWindow', {
              defaultValue: 'Showing a section of the full output.',
            })}
          </span>
          <Button
            variant="ghost"
            size="xs"
            disabled={loading}
            onClick={() => void load(true)}
          >
            {t('conversation.showBeginning', {
              defaultValue: 'Show beginning',
            })}
          </Button>
        </div>
      )}
      {markdown ? (
        <StreamingMarkdown
          content={content}
          finished={block.finished && next === null}
          workspaceId={workspaceId}
          sessionId={store?.sessionId}
        />
      ) : content ? (
        <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-words rounded-[var(--vk-radius-sm)] bg-secondary p-base font-ibm-plex-mono text-base leading-relaxed">
          {content}
        </pre>
      ) : null}
      {next !== null && (
        <Button
          variant="ghost"
          disabled={loading}
          onClick={() => void load()}
          className="mt-2 text-low"
        >
          {loading
            ? t('conversation.loadingContent', {
                defaultValue: 'Loading content…',
              })
            : t('conversation.loadFullContent', {
                defaultValue: 'Load more content',
              })}
        </Button>
      )}
      {longBody && active && !followTail && (
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            latestReadRef.current = null;
            setFollowTail(true);
          }}
        >
          {t('conversation.followLiveOutput', {
            defaultValue: 'Follow live output',
          })}
        </Button>
      )}
      {error && (
        <p role="alert" className="mt-2 break-words text-sm text-error">
          {error}
        </p>
      )}
    </div>
  );
}

function displayToolStatus(
  block: ConversationBlock,
  active: boolean
): ToolStatusLike {
  switch (block.fields.status) {
    case 'succeeded':
      return { status: 'success' };
    case 'failed':
      return { status: 'failed' };
    case 'denied':
      return { status: 'denied' };
    case 'timed_out':
      return { status: 'timed_out' };
    case 'cancelled':
      return { status: 'cancelled' };
    case 'waiting_approval':
      return { status: active ? 'pending_approval' : 'unconfirmed' };
    default:
      return { status: active ? 'created' : 'unconfirmed' };
  }
}

/** Block animation and approvals belong to the current canonical attempt.
 * A terminal Run can leave unfinished blocks, which remain inspectable without
 * continuing their activity animation or inventing a successful result.
 */
function isBlockActive(
  block: ConversationBlock,
  snapshot:
    | Pick<AgentRunSessionContextValue, 'timeline' | 'pendingInteractions'>
    | undefined
): boolean {
  const run = snapshot?.timeline?.runs.find(
    (item) => item.summary.agent_run_id === block.agent_run_id
  )?.summary.state;
  if (
    block.finished ||
    !isCanonicalRunActive(run) ||
    (run?.last_run_attempt_id &&
      run.last_run_attempt_id !== block.run_attempt_id)
  ) {
    return false;
  }
  if (block.kind !== 'tool') return true;
  if (block.fields.status === 'waiting_approval') {
    return Boolean(
      run?.status === 'awaiting_approval' &&
        snapshot?.pendingInteractions.some(
          (request) =>
            request.run_attempt_id === block.run_attempt_id &&
            request.fields.interaction?.tool_call_id != null &&
            request.fields.interaction.tool_call_id ===
              block.fields.tool_call_id &&
            ['pending', 'delivering', 'delivered', 'delivery_unknown'].includes(
              request.fields.interaction.status
            )
        )
    );
  }
  return (
    block.fields.status == null ||
    ['created', 'running'].includes(block.fields.status)
  );
}

export const ConversationBlockEntry = memo(function ConversationBlockEntry({
  blockId,
  workspaceId,
  nested = false,
}: {
  blockId: string;
  workspaceId?: string;
  nested?: boolean;
}) {
  const { t } = useTranslation('common');
  const block = useConversationBlock(blockId);
  const store = useAgentConversationStore();
  const scope = `${store?.hostId ?? 'local'}:${store?.sessionId ?? ''}:${blockId}`;
  const [expanded, toggle] = usePersistedExpanded(`entry:${scope}`, false);
  const contentId = useId();
  const subscribe = useCallback(
    (listener: () => void) => store?.subscribe(listener) ?? (() => {}),
    [store]
  );
  const getActive = useCallback(
    () => Boolean(block && isBlockActive(block, store?.getSnapshot())),
    [block, store]
  );
  const active = useSyncExternalStore(subscribe, getActive, getActive);
  if (!block) return null;
  const body = (
    <BlockBody
      key={`${blockId}:text`}
      block={block}
      field="text"
      markdown
      workspaceId={workspaceId}
      active={active}
    />
  );
  const fields = block.fields;
  let content;
  switch (block.kind) {
    case 'text':
      content =
        block.role === 'user' ? (
          <ChatUserMessage
            content={fields.text ?? ''}
            workspaceId={workspaceId}
            renderMarkdown={() => body}
          />
        ) : block.role === 'system' ? (
          <div className="text-sm text-low">{body}</div>
        ) : (
          <ChatAssistantMessage
            content={fields.text ?? ''}
            workspaceId={workspaceId}
            renderMarkdown={() => body}
          />
        );
      break;
    case 'reasoning':
      content = (
        <ChatThinkingMessage
          content={fields.text ?? ''}
          label={
            fields.reasoning_source === 'summary'
              ? t('conversation.reasoningSummary', {
                  defaultValue: 'Thinking summary',
                })
              : t('conversation.thinking')
          }
          active={active}
          expanded={expanded}
          onToggle={() => toggle()}
          workspaceId={workspaceId}
          renderMarkdown={() => body}
        />
      );
      break;
    case 'tool': {
      const name =
        fields.tool_name ||
        fields.tool_kind ||
        t('conversation.tool', { defaultValue: 'Tool' });
      const status = fields.status;
      const unconfirmed =
        !active &&
        !block.finished &&
        (status === 'created' ||
          status === 'running' ||
          status === 'waiting_approval' ||
          status === undefined);
      content = (
        <div>
          <ChatToolSummary
            summary={name}
            detail={
              fields.locations?.[0]?.path
                ? formatPathForDisplay(fields.locations[0].path)
                : undefined
            }
            toolName={name}
            actionType={fields.tool_kind ?? undefined}
            status={displayToolStatus(block, active)}
            active={active}
            expanded={expanded}
            controlsId={contentId}
            statusLabel={
              unconfirmed
                ? t('conversation.toolResultUnconfirmed', {
                    defaultValue: 'Result not confirmed',
                  })
                : status === 'approved'
                  ? t('conversation.toolAllowed', {
                      defaultValue: 'Allowed to run',
                    })
                  : status === 'cancelled'
                    ? t('agentWorkbench.timeline.cancelled', {
                        defaultValue: 'Cancelled',
                      })
                    : status === 'waiting_approval' && active
                      ? t('conversation.permissionRequest')
                      : ['failed', 'denied', 'timed_out'].includes(status ?? '')
                        ? t('conversation.toolGroupFailed', {
                            defaultValue: 'Tool error',
                          })
                        : undefined
            }
            isTruncated
            onToggle={() => toggle()}
          />
          {expanded && (
            <ChatDisclosureBody id={contentId}>
              {fields.tool_call_id && (
                <div>
                  <p className="mb-half text-sm font-medium text-normal">
                    {t('conversation.toolDetails.callId', {
                      defaultValue: 'Tool call',
                    })}
                  </p>
                  <p className="break-all font-ibm-plex-mono text-sm text-low">
                    {fields.tool_call_id}
                  </p>
                </div>
              )}
              {!!fields.locations?.length && (
                <div>
                  <p className="mb-half text-sm font-medium text-normal">
                    {t('conversation.toolDetails.locations', {
                      defaultValue: 'Files and locations',
                    })}
                  </p>
                  {fields.locations.map((location) => (
                    <p
                      key={`${location.path}:${location.line ?? ''}`}
                      className="break-all font-ibm-plex-mono text-base text-low"
                    >
                      {formatPathForDisplay(location.path)}
                      {location.line ? `:${location.line}` : ''}
                    </p>
                  ))}
                </div>
              )}
              {(fields.arguments != null ||
                block.bodies.some((item) => item.field === 'arguments')) && (
                <div>
                  <p className="mb-half text-sm font-medium text-normal">
                    {t('conversation.toolDetails.parameters', {
                      defaultValue: 'Parameters',
                    })}
                  </p>
                  <BlockBody block={block} field="arguments" />
                </div>
              )}
              {(fields.raw_arguments ||
                block.bodies.some(
                  (item) => item.field === 'raw_arguments'
                )) && (
                <div>
                  <p className="mb-half text-sm font-medium text-normal">
                    {t('conversation.toolDetails.rawParameters', {
                      defaultValue: 'Original parameters',
                    })}
                  </p>
                  {active && fields.arguments == null && (
                    <p className="mb-1 text-xs text-low">
                      {t('conversation.receivingArguments', {
                        defaultValue: 'Receiving parameters…',
                      })}
                    </p>
                  )}
                  <BlockBody block={block} field="raw_arguments" />
                </div>
              )}
              {(fields.result != null ||
                block.bodies.some((item) => item.field === 'result')) && (
                <div>
                  <p className="mb-half text-sm font-medium text-normal">
                    {t('conversation.toolDetails.result', {
                      defaultValue: 'Result',
                    })}
                  </p>
                  <BlockBody block={block} field="result" />
                </div>
              )}
              {(fields.output_text ||
                block.bodies.some((item) => item.field === 'output_text')) && (
                <div>
                  <p className="mb-half text-sm font-medium text-normal">
                    {t('conversation.output', { defaultValue: 'Output' })}
                  </p>
                  <BlockBody
                    block={block}
                    field="output_text"
                    active={active}
                  />
                </div>
              )}
              {fields.text && <BlockBody block={block} field="text" />}
              {block.parent_block_id && (
                <p className="break-all text-xs text-low">
                  {t('conversation.parentTool', {
                    defaultValue: 'Parent tool: {{id}}',
                    id: block.parent_block_id,
                  })}
                </p>
              )}
            </ChatDisclosureBody>
          )}
        </div>
      );
      break;
    }
    case 'plan':
      content = (
        <ChatTodoList
          todos={fields.plan_entries ?? []}
          expanded={expanded}
          onToggle={() => toggle()}
        />
      );
      break;
    case 'interaction': {
      const interaction = fields.interaction;
      content = (
        <ChatApprovalCard
          title={
            interaction?.title ??
            t('conversation.agentRequest', { defaultValue: 'Agent request' })
          }
          content={interaction?.prompt ?? fields.text ?? ''}
          expanded={expanded}
          onToggle={() => toggle()}
          status={{
            status:
              interaction?.status === 'resolved'
                ? 'success'
                : interaction?.status === 'pending'
                  ? 'pending_approval'
                  : interaction?.status === 'failed'
                    ? 'failed'
                    : 'created',
          }}
          renderMarkdown={() => (
            <div className="space-y-2">
              {interaction?.prompt && (
                <p className="whitespace-pre-wrap break-words text-sm">
                  {interaction.prompt}
                </p>
              )}
              {body}
              <p className="text-xs text-low">
                {t(
                  `conversation.interactionStatus.${interaction?.status ?? 'pending'}`,
                  { defaultValue: interaction?.status ?? 'pending' }
                )}
              </p>
              {interaction?.questions.map((question) => (
                <p key={question.id} className="text-sm">
                  {question.prompt}
                </p>
              ))}
              {interaction?.options.map((option) => (
                <p key={option.id} className="text-sm">
                  {option.label}
                  {option.description ? ` — ${option.description}` : ''}
                </p>
              ))}
            </div>
          )}
        />
      );
      break;
    }
    case 'notice': {
      const summary = fields.text?.split(/\r?\n/, 1)[0];
      const collapsible =
        block.finish_reason !== 'failed' &&
        summary &&
        (summary !== fields.text ||
          block.bodies.some((reference) => reference.field === 'text'));
      content = collapsible ? (
        <ChatDisclosure
          icon={<InfoIcon className="size-icon-base" />}
          label={summary}
          ariaLabel={summary}
          expanded={expanded}
          onToggle={() => toggle()}
        >
          {expanded && body}
        </ChatDisclosure>
      ) : (
        <div
          className={
            block.finish_reason === 'failed' ? 'text-error' : 'text-low'
          }
        >
          {body}
        </div>
      );
      break;
    }
    case 'resource':
    case 'extension':
      content = (
        <ChatDisclosure
          icon={<FileTextIcon className="size-icon-base" />}
          label={t(
            block.kind === 'resource'
              ? 'conversation.resourceDetails'
              : 'conversation.providerDetails',
            {
              defaultValue:
                block.kind === 'resource'
                  ? 'Resource details'
                  : 'Agent event details',
            }
          )}
          summary={fields.text?.split(/\r?\n/, 1)[0]}
          expanded={expanded}
          onToggle={() => toggle()}
        >
          {expanded && (
            <>
              {fields.text && body}
              <BlockBody
                block={block}
                field={block.kind === 'resource' ? 'resource' : 'extension'}
              />
            </>
          )}
        </ChatDisclosure>
      );
      break;
  }
  return (
    <div
      className={nested ? 'min-w-0 py-half' : 'min-w-0 px-double py-2'}
      data-conversation-block={blockId}
      data-block-revision={block.revision}
    >
      {content}
    </div>
  );
});

export function ConversationBlockGroup({
  ids,
  workspaceId,
}: {
  ids: string[];
  workspaceId?: string;
}) {
  const { t } = useTranslation('common');
  const store = useAgentConversationStore();
  const subscribe = useCallback(
    (listener: () => void) => {
      const unsubscribers = [
        store?.subscribe(listener),
        ...ids.map((id) => store?.subscribeBlock(id, listener)),
      ];
      return () => unsubscribers.forEach((unsubscribe) => unsubscribe?.());
    },
    [store, ids]
  );
  const getSummary = useCallback(() => {
    const snapshot = store?.getSnapshot();
    const tools = ids.flatMap((id) => {
      const block = store?.getBlock(id);
      if (!block) return [];
      const active = isBlockActive(block, snapshot);
      return [
        {
          block,
          active,
          status: displayToolStatus(block, active).status,
        },
      ];
    });
    const latest = [...tools].reverse();
    const approval = latest.find((tool) => tool.status === 'pending_approval');
    const failed = latest.find((tool) =>
      ['failed', 'denied', 'timed_out'].includes(tool.status)
    );
    const running = latest.find((tool) => tool.active);
    const selected = approval ?? running ?? failed ?? latest[0];
    const status =
      approval?.status ??
      running?.status ??
      failed?.status ??
      selected?.status ??
      'success';
    // Only display metadata participates in this scalar snapshot. Streamed
    // arguments/results must not rerender the collapsed group or load bodies.
    return JSON.stringify([
      selected?.block.fields.tool_name ??
        selected?.block.fields.tool_kind ??
        '',
      selected?.block.fields.locations?.[0]?.path ?? '',
      status,
      Boolean(running),
      Boolean(failed),
    ]);
  }, [store, ids]);
  const summarySnapshot = useSyncExternalStore(
    subscribe,
    getSummary,
    getSummary
  );
  const [name, location, status, active, hasFailure] = JSON.parse(
    summarySnapshot
  ) as [string, string, string, boolean, boolean];
  const summary = [name, location && formatPathForDisplay(location)]
    .filter(Boolean)
    .join(' · ');
  const statusLabel =
    status === 'pending_approval'
      ? t('conversation.permissionRequest')
      : hasFailure
        ? t('conversation.toolGroupFailed', { defaultValue: 'Tool error' })
        : status === 'cancelled'
          ? t('agentWorkbench.timeline.cancelled', {
              defaultValue: 'Cancelled',
            })
          : status === 'unconfirmed'
            ? t('conversation.toolResultUnconfirmed')
            : null;
  const [expanded, toggle] = usePersistedExpanded(
    `entry:tool-group:${store?.hostId ?? 'local'}:${store?.sessionId ?? ''}:${ids[0]}`,
    false
  );
  return (
    <div className="min-w-0 px-double py-1">
      <ChatDisclosure
        icon={<ListMagnifyingGlassIcon className="size-icon-base" />}
        label={t('conversation.toolGroupLabel', {
          defaultValue: 'Tool activity',
        })}
        summary={summary}
        active={active}
        status={{ status }}
        statusLabel={
          statusLabel ? (
            <span className={hasFailure ? 'text-error' : undefined}>
              {statusLabel}
            </span>
          ) : undefined
        }
        expanded={expanded}
        onToggle={() => toggle()}
        bodyClassName="space-y-half"
        trailing={
          active && (
            <span className="sr-only">
              {t('conversation.toolGroupRunning', { defaultValue: 'Running' })}
            </span>
          )
        }
      >
        {expanded &&
          ids.map((id) => (
            <ConversationBlockEntry
              key={id}
              blockId={id}
              workspaceId={workspaceId}
              nested
            />
          ))}
      </ChatDisclosure>
    </div>
  );
}
