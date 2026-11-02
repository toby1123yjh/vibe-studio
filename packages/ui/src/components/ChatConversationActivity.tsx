import { CircleNotchIcon } from '@phosphor-icons/react';

export type ChatConversationActivityState = 'sending' | 'running' | 'stopping';

interface ChatConversationActivityProps {
  state: ChatConversationActivityState;
  label: string;
}

/** Transient feedback at the end of the conversation, outside the composer. */
export function ChatConversationActivity({
  state,
  label,
}: ChatConversationActivityProps) {
  return (
    <span
      className="vk-conversation-activity"
      data-conversation-activity={state}
      role="status"
      aria-live="polite"
      aria-atomic="true"
    >
      <span className="vk-conversation-activity__icon" aria-hidden="true">
        <CircleNotchIcon size={14} weight="bold" />
      </span>
      <span className="vk-conversation-activity__label">{label}</span>
    </span>
  );
}
