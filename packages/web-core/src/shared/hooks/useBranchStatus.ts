import { useQuery } from '@tanstack/react-query';
import { workspacesApi } from '@/shared/lib/api';
import { useHostId } from '@/shared/providers/HostIdProvider';

export function useBranchStatus(
  workspaceId?: string,
  options: { enabled?: boolean } = {}
) {
  const hostId = useHostId();
  return useQuery({
    queryKey: ['branchStatus', workspaceId, hostId],
    queryFn: () => workspacesApi.getBranchStatus(workspaceId!, hostId),
    enabled: !!workspaceId && (options.enabled ?? true),
    refetchInterval: 5000,
  });
}
