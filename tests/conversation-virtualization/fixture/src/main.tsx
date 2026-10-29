import * as React from 'react';
import { createRoot } from 'react-dom/client';
import {
  useConversationVirtualizer,
} from '@web-core/features/workspace-chat/model/useConversationVirtualizer';
import type { ConversationRow } from '@web-core/features/workspace-chat/model/conversation-row-model';
import type { DisplayEntry } from '@web-core/shared/hooks/useConversationHistory/types';
import '@ui/styles/tokens.css';
import './style.css';

const MESSAGE_COUNT = 1000;

const rows: ConversationRow[] = Array.from(
  { length: MESSAGE_COUNT },
  (_, index) => ({
    semanticKey: `fixture-message-${index}`,
    rowFamily: 'tool_summary',
    processId: null,
    estimationHint: 'compact',
    isUserMessage: false,
    // The fixture renders a lightweight row shell; production entries are
    // rendered by DisplayConversationEntry after virtualization selects them.
    entry: {} as DisplayEntry,
  })
);

function ConversationVirtualizationFixture() {
  const [revision, setRevision] = React.useState(0);
  const [currentRows, setCurrentRows] = React.useState(rows);
  const scrollContainerRef = React.useRef<HTMLDivElement>(null);
  const { virtualItems, totalSize, measureElement, virtualizer } =
    useConversationVirtualizer({
      rows: currentRows,
      pinnedTailCount: 32,
      scrollContainerRef,
    });
  const measurementCache = React.useRef(virtualizer.measurementsCache);
  const cacheChanges = React.useRef(0);
  if (measurementCache.current !== virtualizer.measurementsCache) {
    measurementCache.current = virtualizer.measurementsCache;
    cacheChanges.current += 1;
  }

  return (
    <main>
      <h1>1000-message conversation</h1>
      <label>
        Interaction input
        <input data-testid="interaction-input" />
      </label>
      <output data-testid="virtual-count">{virtualItems.length}</output>
      <output data-testid="measurement-rebuilds">{cacheChanges.current}</output>
      <output data-testid="revision">{revision}</output>
      <output data-testid="first-key">{virtualizer.options.getItemKey(0)}</output>
      <button onClick={() => setRevision((value) => value + 1)}>Rerender</button>
      <button onClick={() => setCurrentRows((items) => items.map((row) => ({
        ...row, semanticKey: `replacement-${row.semanticKey}`,
      })))}>Replace session</button>
      <div
        ref={scrollContainerRef}
        data-testid="conversation-scroll"
        className="conversation-scroll"
      >
        <div
          className="conversation-spacer"
          style={{ height: `${totalSize}px` }}
        >
          {virtualItems.map((item) => (
            <div
              key={currentRows[item.index].semanticKey}
              ref={measureElement}
              data-index={item.index}
              data-testid="message-row"
              className="message-row"
              style={{ transform: `translateY(${item.start}px)` }}
            >
              Message {item.index + 1}
            </div>
          ))}
        </div>
      </div>
    </main>
  );
}

const root = document.getElementById('root');
if (!root)
  throw new Error('Conversation virtualization fixture root is missing');
createRoot(root).render(<ConversationVirtualizationFixture />);
