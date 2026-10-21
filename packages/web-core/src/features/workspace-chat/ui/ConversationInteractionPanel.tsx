import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { ReplyAgentRunInteractionRequest } from 'shared/types';
import { Button } from '@vibe/ui/components/Button';
import {
  useAgentRunSession,
  useAgentConversationStore,
  useConversationBlock,
} from '@/features/agent-runtime/model/AgentRunSessionContext';
import {
  agentRunsApi,
  createAgentInteractionReply,
} from '@/shared/lib/agentRunApi';

function NativeRequest({ blockId }: { blockId: string }) {
  const { t } = useTranslation('common');
  const store = useAgentConversationStore();
  const block = useConversationBlock(blockId);
  const [option, setOption] = useState<string>();
  const [answers, setAnswers] = useState<Record<string, string[]>>({});
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const decisionRef = useRef<ReplyAgentRunInteractionRequest | null>(null);
  const interaction = block?.fields.interaction;
  if (!block || !interaction || !store) return null;

  const run = store
    .getSnapshot()
    .timeline?.runs.find(
      (item) => item.summary.agent_run_id === block.agent_run_id
    );
  const currentAttempt =
    run?.summary.state.last_run_attempt_id === block.run_attempt_id;
  const locked =
    sending ||
    sent ||
    uncertain ||
    interaction.status !== 'pending' ||
    !interaction.actionable ||
    !currentAttempt;
  const valid =
    interaction.kind === 'permission'
      ? Boolean(option)
      : interaction.questions.length > 0
        ? interaction.questions.every(
            (question) =>
              !question.required ||
              (answers[question.id] ?? []).some((value) => value.trim())
          )
        : Boolean(text.trim());
  const submit = async () => {
    if (locked || !valid || decisionRef.current) return;
    const request = createAgentInteractionReply(block.run_attempt_id, {
      request_id: interaction.request_id,
      ...(option ? { option_id: option } : {}),
      answers: interaction.questions.map((question) => ({
        question_id: question.id,
        values: answers[question.id] ?? [],
      })),
      ...(text.trim() ? { text: text.trim() } : {}),
    });
    decisionRef.current = request;
    setSending(true);
    setError(null);
    try {
      await agentRunsApi.replyInteraction(
        block.agent_run_id,
        request,
        store.hostId
      );
      setSent(true);
    } catch (failure) {
      // A dropped HTTP response does not prove the native write failed. Keep
      // the decision identity and ask for status instead of risking a repeat.
      setUncertain(true);
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setSending(false);
      void store.refresh().catch(() => {});
    }
  };
  return (
    <section
      className="space-y-3"
      aria-label={
        interaction.title ??
        t('conversation.agentRequest', { defaultValue: 'Agent request' })
      }
      data-interaction-request={interaction.request_id}
    >
      <div>
        <p className="text-sm font-medium text-high">
          {interaction.title ??
            t(
              interaction.kind === 'permission'
                ? 'conversation.permissionRequest'
                : 'conversation.questionRequest',
              {
                defaultValue:
                  interaction.kind === 'permission'
                    ? 'Permission required'
                    : 'Agent question',
              }
            )}
        </p>
        {interaction.prompt && (
          <p className="mt-1 whitespace-pre-wrap break-words text-sm text-normal">
            {interaction.prompt}
          </p>
        )}
      </div>
      {interaction.options.length > 0 && (
        <fieldset disabled={locked} className="flex flex-wrap gap-2">
          <legend className="sr-only">
            {t('conversation.nativeChoices', { defaultValue: 'Agent choices' })}
          </legend>
          {interaction.options.map((choice) => (
            <label
              key={choice.id}
              className="flex min-h-9 cursor-pointer items-start gap-2 rounded border border-border px-3 py-2 text-sm has-[:checked]:border-brand has-[:checked]:bg-brand/10"
            >
              <input
                type="radio"
                className="mt-1 accent-brand"
                name={`permission:${blockId}`}
                value={choice.id}
                checked={option === choice.id}
                onChange={() => setOption(choice.id)}
              />
              <span>
                <span className="block">{choice.label}</span>
                {choice.description && (
                  <span className="block text-xs text-low">
                    {choice.description}
                  </span>
                )}
              </span>
            </label>
          ))}
        </fieldset>
      )}
      {interaction.questions.map((question) => (
        <fieldset key={question.id} disabled={locked} className="space-y-2">
          <legend className="mb-1 text-sm font-medium">
            {question.prompt}
          </legend>
          {question.options.map((choice) => (
            <label
              key={choice.id}
              className="flex cursor-pointer items-start gap-2 text-sm"
            >
              <input
                type={question.multiple ? 'checkbox' : 'radio'}
                name={`${blockId}:${question.id}`}
                value={choice.id}
                className="mt-1 accent-brand"
                checked={(answers[question.id] ?? []).includes(choice.id)}
                onChange={(event) =>
                  setAnswers((previous) => ({
                    ...previous,
                    [question.id]: question.multiple
                      ? event.target.checked
                        ? [...(previous[question.id] ?? []), choice.id]
                        : (previous[question.id] ?? []).filter(
                            (id) => id !== choice.id
                          )
                      : [choice.id],
                  }))
                }
              />
              <span>
                {choice.label}
                {choice.description && (
                  <span className="ml-2 text-xs text-low">
                    {choice.description}
                  </span>
                )}
              </span>
            </label>
          ))}
          {(question.allow_free_text || question.options.length === 0) && (
            <textarea
              rows={2}
              aria-label={question.prompt}
              className="w-full resize-y rounded border border-border bg-primary px-3 py-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
              value={(answers[question.id] ?? [])
                .filter(
                  (value) =>
                    !question.options.some((choice) => choice.id === value)
                )
                .join('\n')}
              onChange={(event) =>
                setAnswers((previous) => ({
                  ...previous,
                  [question.id]: [
                    ...(question.multiple
                      ? (previous[question.id] ?? []).filter((value) =>
                          question.options.some((choice) => choice.id === value)
                        )
                      : []),
                    ...(event.target.value ? [event.target.value] : []),
                  ],
                }))
              }
            />
          )}
        </fieldset>
      ))}
      {interaction.kind === 'question' &&
        interaction.questions.length === 0 && (
          <textarea
            rows={2}
            disabled={locked}
            aria-label={
              interaction.prompt ??
              t('conversation.yourAnswer', { defaultValue: 'Your answer' })
            }
            value={text}
            onChange={(event) => setText(event.target.value)}
            className="w-full rounded border border-border bg-primary px-3 py-2 text-sm focus-visible:ring-2 focus-visible:ring-brand"
          />
        )}
      {(uncertain || interaction.status === 'delivery_unknown') && (
        <p role="status" className="text-sm text-warning">
          {t('conversation.deliveryUncertain', {
            defaultValue:
              'Delivery could not be confirmed. Check the agent status before making another decision.',
          })}
        </p>
      )}
      {sent && interaction.status === 'pending' && (
        <p role="status" className="text-sm text-low">
          {t('conversation.replyAccepted', {
            defaultValue: 'Response accepted. Waiting for the agent.',
          })}
        </p>
      )}
      {interaction.status !== 'pending' && (
        <p role="status" className="text-sm text-low">
          {t(`conversation.interactionStatus.${interaction.status}`, {
            defaultValue: interaction.status,
          })}
        </p>
      )}
      {!interaction.actionable && interaction.status === 'pending' && (
        <p className="text-sm text-low">
          {t('conversation.requestReadOnly', {
            defaultValue:
              'This request can be viewed here, but cannot be answered through the current connection.',
          })}
        </p>
      )}
      {error && (
        <p role="alert" className="break-words text-sm text-error">
          {error}
        </p>
      )}
      <div className="flex items-center gap-2">
        {!locked && (
          <Button size="sm" disabled={!valid} onClick={() => void submit()}>
            {t('conversation.sendResponse', { defaultValue: 'Send response' })}
          </Button>
        )}
        {sending && (
          <span role="status" className="text-sm text-low">
            {t('conversation.sendingResponse', {
              defaultValue: 'Sending response…',
            })}
          </span>
        )}
        {(uncertain ||
          sent ||
          interaction.status === 'delivery_unknown' ||
          interaction.status === 'failed') && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() =>
              void store
                .refresh()
                .catch((failure: unknown) =>
                  setError(
                    failure instanceof Error ? failure.message : String(failure)
                  )
                )
            }
          >
            {t('conversation.checkResponseStatus', {
              defaultValue: 'Check status',
            })}
          </Button>
        )}
      </div>
    </section>
  );
}

/** Shared with standalone, Task and workflow chats. Native response drafts are
 * intentionally separate from the existing follow-up editor and its scratch. */
export function ConversationInteractionPanel() {
  const { pendingInteractions } = useAgentRunSession();
  if (!pendingInteractions.length) return null;
  return (
    <div className="mx-auto max-h-[50vh] w-chat max-w-full shrink-0 space-y-4 overflow-y-auto border-y border-warning/30 bg-warning/5 px-double py-3">
      {pendingInteractions.map((block) => (
        <NativeRequest
          key={`${block.block_id}:${block.fields.interaction?.request_id}`}
          blockId={block.block_id}
        />
      ))}
    </div>
  );
}
