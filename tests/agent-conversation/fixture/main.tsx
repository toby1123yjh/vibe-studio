import * as React from 'react';
import { createRoot } from 'react-dom/client';
import {
  AgentRunSessionProvider,
  useAgentRunSession,
} from '@/features/agent-runtime/model/AgentRunSessionContext';
import {
  ConversationBlockEntry,
  ConversationBlockGroup,
} from '@/features/workspace-chat/ui/ConversationBlockEntry';
import { ConversationInteractionPanel } from '@/features/workspace-chat/ui/ConversationInteractionPanel';
import { FixtureHostProvider } from '../../agent-stream/fixture/host';
import {
  append,
  block,
  bodies,
  requests,
  sockets,
  state,
  updateBlock,
} from './transport';
import { widenWindow } from './transport';
import { EntriesProvider } from '@/features/workspace-chat/model/contexts/EntriesContext';
import ConversationListContainer, {
  type ConversationListHandle,
} from '@/features/workspace-chat/ui/ConversationListContainer';
import type { WorkspaceWithSession } from '@/shared/types/attempt';
import '@/i18n/config';
import '../../../packages/ui/src/styles/tokens.css';
import './list.css';

const renders = { draft: 0 };
const list = React.createRef<ConversationListHandle>();
Object.assign(window, {
  conversationFixture: {
    append,
    block,
    bodies,
    requests,
    sockets,
    state,
    updateBlock,
    widenWindow,
    renders,
    list,
  },
});
const attempt = {
  id: 'workspace',
  session: { id: 'session' },
} as WorkspaceWithSession;
function ListTimeline() {
  return (
    <div className="list-host">
      <ConversationListContainer ref={list} attempt={attempt} readOnly />
    </div>
  );
}
function Draft() {
  const session = useAgentRunSession();
  const [value, setValue] = React.useState('');
  ++renders.draft;
  return (
    <label>
      Follow-up draft
      <textarea
        aria-label="Follow-up draft"
        value={value}
        onChange={(event) => setValue(event.target.value)}
        data-connected={session.isConnected}
      />
    </label>
  );
}
function Timeline() {
  const { conversation } = useAgentRunSession();
  return (
    <div data-testid="timeline" style={{ height: 420, overflow: 'auto' }}>
      {conversation.entries.map(
        (entry) =>
          !['tool-a', 'tool-b'].includes(entry.conversationBlockId!) && (
            <ConversationBlockEntry
              key={entry.patchKey}
              blockId={entry.conversationBlockId!}
            />
          )
      )}
    </div>
  );
}
function App() {
  const [host, setHost] = React.useState<string | null>('host-a');
  const listFixture = new URLSearchParams(location.search).has('list');
  return (
    <FixtureHostProvider value={host}>
      <button onClick={() => setHost('host-b')}>Switch host</button>
      {listFixture ? (
        <EntriesProvider sessionId="session">
          <ListTimeline />
          <Draft />
        </EntriesProvider>
      ) : (
        <AgentRunSessionProvider sessionId="session">
          <Timeline />
          <ConversationBlockGroup ids={['tool-a', 'tool-b']} />
          <ConversationInteractionPanel />
          <Draft />
        </AgentRunSessionProvider>
      )}
    </FixtureHostProvider>
  );
}
createRoot(document.getElementById('root')!).render(<App />);
