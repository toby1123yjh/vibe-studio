import { useSyncExternalStore } from 'react';
import type {
  CreateFollowUpAttempt,
  ExecutorConfig,
  ExecutorProfileId,
  QueueStatus,
  Session,
} from 'shared/types';

let host: string | null = null;
export const getCurrentHostId = () => host;
export async function handleApiResponse<T>(response: Response): Promise<T> {
  const body = await response.json();
  if (!response.ok || !body.success) throw new Error(body.message ?? 'Fixture API request failed');
  return body.data;
}
const listeners = new Set<() => void>();
export function switchHost() {
  host = host ? null : 'remote-host';
  listeners.forEach((listener) => listener());
}
export const useHostId = () =>
  useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    () => host
  );

export const sessionsApi = {
  getExecutorConfig: async (
    id: string,
    hostId: string | null = host
  ): Promise<ExecutorConfig | null> => {
    const response = await fetch(
      `/__fixture/session-config/${id}?host=${hostId ?? 'local'}`
    );
    if (!response.ok) throw new Error('Saved session config is unavailable');
    return response.json();
  },
  create: async ({
    workspace_id,
  }: {
    workspace_id: string;
  }): Promise<Session> => ({
    id: 'a',
    workspace_id,
    name: null,
    executor: null,
    agent_working_dir: null,
    created_at: '2026-09-07T00:00:00Z',
    updated_at: '2026-09-07T00:00:00Z',
  }),
  followUp: async (id: string, body: CreateFollowUpAttempt) => {
    const response = await fetch(
      `/__fixture/session-config/${id}?host=${host ?? 'local'}`,
      { method: 'POST', body: JSON.stringify(body.executor_config) }
    );
    if (!response.ok) throw new Error('Fixture follow-up failed');
    return response.json();
  },
};

export const agentsApi = {
  getPresetOptions: async (
    profile: ExecutorProfileId
  ): Promise<ExecutorConfig> => ({
    ...profile,
    model_id: 'preset-model',
    agent_id: 'preset-agent',
    reasoning_id: 'preset-reasoning',
    provider_options: null,
  }),
};

export const queueApi = {
  getStatus: async (
    id: string,
    hostId: string | null
  ): Promise<QueueStatus> => {
    const response = await fetch(
      `/__fixture/queue/${id}?host=${hostId ?? 'local'}`
    );
    return response.json();
  },
};
