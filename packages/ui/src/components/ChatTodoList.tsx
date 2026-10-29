import { useTranslation } from 'react-i18next';
import {
  ListChecksIcon,
  CircleIcon,
  CheckCircleIcon,
  CircleDashedIcon,
  MinusCircleIcon,
} from '@phosphor-icons/react';
import { ChatDisclosure, ChatDisclosureRow } from './ChatDisclosure';

export interface TodoItemLike {
  content: string;
  status?: string | null;
}

interface ChatTodoListProps {
  todos: TodoItemLike[];
  expanded?: boolean;
  onToggle?: () => void;
}

function getStatusIcon(status?: string | null) {
  const s = (status || '').toLowerCase();
  if (s === 'completed')
    return (
      <CheckCircleIcon aria-hidden className="size-icon-base text-success" />
    );
  if (s === 'in_progress' || s === 'in-progress')
    return (
      <CircleDashedIcon aria-hidden className="size-icon-base text-brand" />
    );
  if (s === 'cancelled')
    return <MinusCircleIcon aria-hidden className="size-icon-base text-low" />;
  return <CircleIcon aria-hidden className="size-icon-base text-low" />;
}

export function ChatTodoList({ todos, expanded, onToggle }: ChatTodoListProps) {
  const { t } = useTranslation('tasks');

  const row = {
    icon: <ListChecksIcon className="size-icon-base" />,
    label: t('conversation.updatedTodos'),
  };
  const body = todos.length > 0 && (
    <ul className="space-y-half">
      {todos.map((todo, index) => (
        <li key={`${todo.content}-${index}`} className="flex items-start gap-2">
          <span className="inline-flex size-icon-base shrink-0 items-center justify-center">
            {getStatusIcon(todo.status)}
          </span>
          <span className="min-w-0 break-words text-base leading-relaxed">
            {todo.status?.toLowerCase() === 'cancelled' ? (
              <s className="text-low">{todo.content}</s>
            ) : (
              todo.content
            )}
          </span>
        </li>
      ))}
    </ul>
  );
  return onToggle ? (
    <ChatDisclosure {...row} expanded={Boolean(expanded)} onToggle={onToggle}>
      {expanded && body}
    </ChatDisclosure>
  ) : (
    <div className="min-w-0">
      <ChatDisclosureRow {...row} />
      {body}
    </div>
  );
}
