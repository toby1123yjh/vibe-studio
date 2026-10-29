import { CircleNotchIcon } from '@phosphor-icons/react';

export type ChatComposerActivityState = 'sending' | 'running' | 'stopping';

interface ChatComposerActivityProps {
  state: ChatComposerActivityState;
  label: string;
  detail?: string;
}

/** A compact projection of the caller's actual request or runtime state. */
export function ChatComposerActivity({
  state,
  label,
  detail,
}: ChatComposerActivityProps) {
  return (
    <span
      className="vk-chat-activity"
      data-composer-activity={state}
      role="status"
      aria-live="polite"
      aria-atomic="true"
    >
      <span className="vk-chat-activity__icon" aria-hidden="true">
        <CircleNotchIcon size={14} weight="bold" />
      </span>
      <span className="vk-chat-activity__label">{label}</span>
      {detail && (
        <span className="vk-chat-activity__detail" title={detail}>
          <span aria-hidden="true">· </span>
          {detail}
        </span>
      )}
    </span>
  );
}
