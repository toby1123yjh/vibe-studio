import { getCurrentHostId, useHostId } from '@/shared/providers/HostIdProvider';
import {
  useQuery,
  useMutation,
  useQueryClient,
  type UseQueryResult,
} from '@tanstack/react-query';
import { createWorkflowApi } from '@/shared/lib/workflowApi';
import type {
  WorkflowTemplateResponse,
  WorkflowTemplateListResponse,
  CreateWorkflowRequest,
  UpdateWorkflowRequest,
} from 'shared/types';

export const workflowTemplateQueryKeys = {
  all: ['workflow-templates'] as const,
  host: (hostId = getCurrentHostId()) =>
    ['workflow-templates', hostId] as const,
  globalList: (hostId = getCurrentHostId()) =>
    ['workflow-templates', hostId, 'global'] as const,
  list: (projectId: string, hostId = getCurrentHostId()) =>
    ['workflow-templates', hostId, 'project', projectId] as const,
  detail: (workflowId: string, hostId = getCurrentHostId()) =>
    ['workflow-templates', hostId, 'detail', workflowId] as const,
};

export interface UseWorkflowTemplatesOptions {
  enabled?: boolean;
  allProjects?: boolean;
}

export function useWorkflowTemplates(
  projectId: string | null | undefined,
  options: UseWorkflowTemplatesOptions = {}
): UseQueryResult<WorkflowTemplateListResponse> {
  const hostId = useHostId();
  const workflowApi = createWorkflowApi(hostId);
  const { enabled = true, allProjects = false } = options;

  return useQuery({
    queryKey: projectId
      ? workflowTemplateQueryKeys.list(projectId, hostId)
      : allProjects
        ? workflowTemplateQueryKeys.globalList(hostId)
        : ['workflow-templates', hostId, 'noop'],
    queryFn: ({ signal }) =>
      projectId
        ? workflowApi.list(projectId, signal)
        : workflowApi.listAll(signal),
    enabled: (!!projectId || allProjects) && enabled,
  });
}

export function useWorkflowTemplate(
  workflowId: string | null | undefined,
  options: UseWorkflowTemplatesOptions = {}
): UseQueryResult<WorkflowTemplateResponse> {
  const hostId = useHostId();
  const workflowApi = createWorkflowApi(hostId);
  const { enabled = true } = options;

  return useQuery({
    queryKey: workflowId
      ? workflowTemplateQueryKeys.detail(workflowId, hostId)
      : ['workflow-templates', 'detail', 'noop'],
    queryFn: () => workflowApi.get(workflowId as string),
    enabled: !!workflowId && enabled,
  });
}

export function useWorkflowTemplateMutations() {
  const hostId = useHostId();
  const workflowApi = createWorkflowApi(hostId);
  const queryClient = useQueryClient();

  const createMutation = useMutation({
    mutationFn: ({
      projectId,
      payload,
    }: {
      projectId: string;
      payload: CreateWorkflowRequest;
    }) => workflowApi.create(projectId, payload),
    onSuccess: (data) => {
      void queryClient.invalidateQueries({
        queryKey: workflowTemplateQueryKeys.host(hostId),
      });
      queryClient.setQueryData(
        workflowTemplateQueryKeys.detail(data.id, hostId),
        data
      );
    },
  });

  const updateMutation = useMutation({
    mutationFn: ({
      workflowId,
      payload,
    }: {
      workflowId: string;
      payload: UpdateWorkflowRequest;
    }) => workflowApi.update(workflowId, payload),
    onSuccess: (data) => {
      void queryClient.invalidateQueries({
        queryKey: workflowTemplateQueryKeys.host(hostId),
      });
      queryClient.setQueryData(
        workflowTemplateQueryKeys.detail(data.id, hostId),
        data
      );
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (workflowId: string) => workflowApi.delete(workflowId),
    onSuccess: (_, workflowId) => {
      void queryClient.invalidateQueries({
        queryKey: workflowTemplateQueryKeys.host(hostId),
      });
      queryClient.removeQueries({
        queryKey: workflowTemplateQueryKeys.detail(workflowId, hostId),
      });
    },
  });

  return {
    createTemplate: createMutation.mutateAsync,
    isCreating: createMutation.isPending,
    updateTemplate: updateMutation.mutateAsync,
    isUpdating: updateMutation.isPending,
    deleteTemplate: deleteMutation.mutateAsync,
    isDeleting: deleteMutation.isPending,
  };
}
