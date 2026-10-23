import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { PairRelayHostRequest } from 'shared/types';
import type { RelayHost } from 'shared/remote-types';
import { relayApi } from '@/shared/lib/api';
import { useAppRuntime } from '@/shared/hooks/useAppRuntime';
import {
  listPairedRelayHosts as listBrowserPairedRelayHosts,
  subscribeRelayPairingChanges,
} from '@/shared/lib/relayPairingStorage';
import { getRemoteApiUrl, listRelayHosts } from '@/shared/lib/remoteApi';

export type RemoteCloudHostStatus = 'online' | 'offline' | 'unpaired';

export interface RemoteCloudHost {
  id: string;
  name: string;
  status: RemoteCloudHostStatus;
  pairedAt: string;
  lastUsedAt: string;
}

export interface RemoteCloudHostsState {
  hosts: RemoteCloudHost[];
  relayHosts: RelayHost[];
}

export const REMOTE_CLOUD_HOSTS_STATE_QUERY_KEY = [
  'remote-cloud-hosts',
  'state',
] as const;

const REMOTE_CLOUD_RELAY_HOSTS_QUERY_KEY = [
  ...REMOTE_CLOUD_HOSTS_STATE_QUERY_KEY,
  'relay-hosts',
] as const;

interface UseRemoteCloudHostsStateOptions {
  enabled?: boolean;
}

function normalizeRemoteCloudHostStatus(
  status: RelayHost['status'] | undefined
): RemoteCloudHostStatus {
  if (status === 'online' || status === 'offline' || status === 'unpaired') {
    return status;
  }

  return 'offline';
}

function projectRemoteCloudHostsState(
  pairedHosts: Awaited<ReturnType<typeof relayApi.listPairedRelayHosts>>,
  remoteHosts: RelayHost[]
): RemoteCloudHostsState {
  const remoteHostsById = new Map(remoteHosts.map((host) => [host.id, host]));

  const hosts = pairedHosts
    .map((host) => {
      const remoteHost = remoteHostsById.get(host.host_id);
      const status = normalizeRemoteCloudHostStatus(remoteHost?.status);
      const pairedAt = host.paired_at ?? '';

      return {
        id: host.host_id,
        name: remoteHost?.name ?? host.host_name ?? host.host_id,
        status,
        pairedAt,
        lastUsedAt: pairedAt,
      };
    })
    .sort(
      (a, b) => b.pairedAt.localeCompare(a.pairedAt) || a.id.localeCompare(b.id)
    );

  return { hosts, relayHosts: remoteHosts };
}

export function useRemoteCloudHostsState(
  options: UseRemoteCloudHostsStateOptions = {}
) {
  const { enabled: requestedEnabled = true } = options;
  const runtime = useAppRuntime();
  // Local authentication also represents offline/local-only usage. It does
  // not imply a configured relay, and the local project API has no Hosts API.
  const enabled =
    requestedEnabled && (runtime === 'remote' || Boolean(getRemoteApiUrl()));
  const queryClient = useQueryClient();
  const pairingOwner = runtime === 'remote' ? 'browser' : 'machine';
  const pairedHostsQueryKey = useMemo(
    () =>
      [
        ...REMOTE_CLOUD_HOSTS_STATE_QUERY_KEY,
        'paired-hosts',
        pairingOwner,
      ] as const,
    [pairingOwner]
  );
  const {
    data: pairedHosts,
    error: pairedHostsError,
    isFetching: pairedHostsFetching,
    refetch: refetchPairedHosts,
  } = useQuery({
    queryKey: pairedHostsQueryKey,
    queryFn:
      pairingOwner === 'browser'
        ? listBrowserPairedRelayHosts
        : relayApi.listPairedRelayHosts,
    enabled,
    staleTime: pairingOwner === 'browser' ? 5_000 : 0,
  });

  useEffect(() => {
    if (!enabled || pairingOwner !== 'browser') return;

    return subscribeRelayPairingChanges(() => {
      void queryClient.invalidateQueries({ queryKey: pairedHostsQueryKey });
    });
  }, [enabled, pairedHostsQueryKey, pairingOwner, queryClient]);

  const {
    data: relayHostsSnapshot,
    error: relayHostsError,
    isFetching: relayHostsFetching,
    refetch: refetchRelayHosts,
  } = useQuery({
    queryKey: REMOTE_CLOUD_RELAY_HOSTS_QUERY_KEY,
    queryFn: listRelayHosts,
    enabled,
    staleTime: 0,
  });
  const relayHosts = useMemo(
    () =>
      relayHostsSnapshot === undefined
        ? undefined
        : [...relayHostsSnapshot].sort(
            (a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id)
          ),
    [relayHostsSnapshot]
  );
  const hasCanonicalData =
    enabled && pairedHosts !== undefined && relayHosts !== undefined;
  const error = enabled ? (pairedHostsError ?? relayHostsError ?? null) : null;
  const data = useMemo(
    () =>
      hasCanonicalData
        ? projectRemoteCloudHostsState(pairedHosts, relayHosts)
        : undefined,
    [hasCanonicalData, pairedHosts, relayHosts]
  );
  const isFetching = enabled && (pairedHostsFetching || relayHostsFetching);
  const retryIdentity = `${runtime}:${pairingOwner}:${enabled ? 'enabled' : 'disabled'}`;
  const retryIdentityRef = useRef(retryIdentity);
  const retryEpochRef = useRef(0);
  const retryPendingEpochRef = useRef<number | null>(null);
  const [retryPendingEpoch, setRetryPendingEpoch] = useState<number | null>(
    null
  );

  useEffect(() => {
    retryIdentityRef.current = retryIdentity;
    retryPendingEpochRef.current = null;
    setRetryPendingEpoch(null);

    return () => {
      retryEpochRef.current += 1;
      retryPendingEpochRef.current = null;
    };
  }, [retryIdentity]);

  const retryPending =
    retryPendingEpoch === retryEpochRef.current &&
    retryPendingEpochRef.current === retryPendingEpoch;

  const retry = useCallback(async () => {
    const epoch = retryEpochRef.current;
    if (
      !enabled ||
      retryIdentityRef.current !== retryIdentity ||
      retryPendingEpochRef.current === epoch ||
      isFetching
    ) {
      return;
    }

    retryPendingEpochRef.current = epoch;
    setRetryPendingEpoch(epoch);
    try {
      await Promise.all([refetchPairedHosts(), refetchRelayHosts()]);
    } finally {
      if (
        retryIdentityRef.current === retryIdentity &&
        retryEpochRef.current === epoch &&
        retryPendingEpochRef.current === epoch
      ) {
        retryPendingEpochRef.current = null;
        setRetryPendingEpoch(null);
      }
    }
  }, [
    enabled,
    isFetching,
    refetchPairedHosts,
    refetchRelayHosts,
    retryIdentity,
  ]);

  return {
    data,
    hasCanonicalData,
    isLoading: enabled && !hasCanonicalData && error == null,
    isFetching,
    isRetrying: enabled && (retryPending || (error != null && isFetching)),
    error,
    canRetry: enabled && !isFetching && !retryPending,
    retry,
  };
}

export function usePairRemoteCloudHostMutation() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (payload: PairRelayHostRequest) =>
      relayApi.pairRelayHost(payload),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: REMOTE_CLOUD_HOSTS_STATE_QUERY_KEY,
      });
    },
  });
}

export function useRemoveRemoteCloudHostMutation() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (hostId: string) => relayApi.removePairedRelayHost(hostId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: REMOTE_CLOUD_HOSTS_STATE_QUERY_KEY,
      });
    },
  });
}
