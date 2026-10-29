import React, { useState } from 'react';
import { useCreateModeState } from '@/features/create-mode/model/useCreateModeState';

function Draft({ onClose }: { onClose(): void }) {
  const draft = useCreateModeState({
    draftId: 'persistence-test',
    hostId: 'host-a',
    lastWorkspaceId: null,
    localWorkspacesLoading: false,
  });
  if (!draft.hasInitialValue) return <p>Loading draft</p>;
  return (
    <>
      <input
        aria-label="Draft"
        value={draft.message}
        onChange={(event) => draft.setMessage(event.target.value)}
      />
      <button
        onClick={() => {
          void draft.clearDraft();
          draft.setMessage('New draft after clear');
          onClose();
        }}
      >
        Clear and close with newer input
      </button>
    </>
  );
}

export function DraftPersistencePreview() {
  const [open, setOpen] = useState(true);
  return (
    <>
      <button onClick={() => setOpen((value) => !value)}>
        {open ? 'Close draft' : 'Open draft'}
      </button>
      {open && <Draft onClose={() => setOpen(false)} />}
    </>
  );
}
