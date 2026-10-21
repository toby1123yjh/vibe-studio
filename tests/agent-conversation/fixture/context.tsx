import * as React from 'react';

// Keep production list/history/virtualizer/store/blocks. Only unrelated
// application services and the obsolete script renderer are isolated here.
const emptyProcesses: never[] = [];
export function useExecutionProcessesContext() {
  return {
    executionProcessesVisible: emptyProcesses,
    isLoading: false,
    isConnected: true,
  };
}
const idleQuery = {
  data: null,
  error: null,
  isFetching: false,
  refetch: async () => {},
};
export function useWorkflowNotifications() {
  return { context: idleQuery, notifications: idleQuery };
}
export const ScriptFixerDialog = { show: async () => {} };
export default function LegacyEntry() {
  return <div data-testid="legacy-row" />;
}
