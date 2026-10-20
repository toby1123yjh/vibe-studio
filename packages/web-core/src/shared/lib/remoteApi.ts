import type {
  AttachmentUrlResponse,
  AttachmentWithBlob,
  CommitAttachmentsRequest,
  CommitAttachmentsResponse,
  ConfirmUploadRequest,
  InitUploadRequest,
  InitUploadResponse,
  ListRelayHostsResponse,
  RelayHost,
  Project,
  UpdateTaskRequest,
  UpdateProjectRequest,
  UpdateProjectStatusRequest,
} from 'shared/remote-types';
import { getAuthRuntime } from '@/shared/lib/auth/runtime';
import { makeLocalApiRequest } from '@/shared/lib/localApiTransport';
import { syncRelayApiBaseWithRemote } from '@/shared/lib/relayBackendApi';
import { getCurrentHostId } from '@/shared/providers/HostIdProvider';
import { z } from 'zod';

const BUILD_TIME_API_BASE = import.meta.env.VITE_VK_SHARED_API_BASE || '';

// Mutable module-level variable — overridden at runtime by ConfigProvider
// when VK_SHARED_API_BASE is set (for self-hosting support)
let _remoteApiBase: string = BUILD_TIME_API_BASE;
let _localRemoteApiEnabled = false;

export function setLocalRemoteApiEnabled(enabled: boolean) {
  _localRemoteApiEnabled = enabled;
}

export function isLocalRemoteApiEnabled(): boolean {
  return _localRemoteApiEnabled && !getRemoteApiUrl();
}

const projectDirectoryConflictSchema = z.object({
  success: z.literal(false),
  error_data: z.object({
    code: z.literal('PROJECT_DIRECTORY_EXISTS'),
    project: z.object({
      id: z.string().uuid(),
      name: z.string().refine((name) => name.trim().length > 0),
    }),
  }),
});

export class ProjectDirectoryExistsError extends Error {
  constructor(readonly project: Pick<Project, 'id' | 'name'>) {
    super(
      'This directory already contains a project. Confirm before opening it.'
    );
    this.name = 'ProjectDirectoryExistsError';
  }
}

export interface OpenLocalProjectDirectoryInput {
  directory_path: string;
  name: string;
  color: string;
  id?: string;
  confirmed_project_id?: string;
}

/** Create a directory project; existing identities require explicit confirmation. */
export async function openLocalProjectDirectory(
  input: OpenLocalProjectDirectoryInput,
  hostId: string | null
): Promise<Project> {
  const response = await makeLocalApiRequest('/api/local/v1/projects/open', {
    method: 'POST',
    hostScope: 'explicit',
    hostId,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(input),
  });
  if (!response.ok) {
    if (response.status === 409) {
      const body: unknown = await response
        .clone()
        .json()
        .catch(() => null);
      const conflict = projectDirectoryConflictSchema.safeParse(body);
      if (conflict.success) {
        throw new ProjectDirectoryExistsError(conflict.data.error_data.project);
      }
    }
    throw await parseErrorResponse(
      response,
      'Failed to open project directory'
    );
  }
  const result = (await response.json()) as { data: Project };
  return result.data;
}

function withDefaultJsonHeaders<T extends RequestInit>(
  options: T
): T & { headers: Headers } {
  const headers = new Headers(options.headers ?? {});
  if (!headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  return {
    ...options,
    headers,
  };
}

function toLocalRemoteApiPath(path: string): string {
  if (path.startsWith('/v1/')) {
    return `/api/local${path}`;
  }

  return `/api/local/${path.replace(/^\/+/, '')}`;
}

/**
 * Set the remote API base URL at runtime.
 * Called by ConfigProvider when /api/info returns a shared_api_base value.
 * No-op if base is null/undefined/empty (preserves build-time fallback).
 */
export function setRemoteApiBase(base: string | null | undefined) {
  _remoteApiBase = base || BUILD_TIME_API_BASE;
  if (_remoteApiBase) {
    syncRelayApiBaseWithRemote(_remoteApiBase);
  }
}

/**
 * Get the current remote API base URL.
 * Returns the runtime value if set by ConfigProvider, otherwise the build-time default.
 */
export function getRemoteApiUrl(): string {
  return _remoteApiBase;
}

// Backward-compatible export — consumers should migrate to getRemoteApiUrl()
export const REMOTE_API_URL = BUILD_TIME_API_BASE;

export const makeRequest = async (
  path: string,
  options: RequestInit = {},
  retryOn401 = true,
  hostId: string | null = getCurrentHostId()
): Promise<Response> => {
  if (isLocalRemoteApiEnabled()) {
    return makeLocalApiRequest(
      toLocalRemoteApiPath(path),
      withDefaultJsonHeaders({
        ...options,
        hostScope: 'explicit' as const,
        hostId,
      })
    );
  }

  return makeAuthenticatedRequest(getRemoteApiUrl(), path, options, retryOn401);
};

async function makeAuthenticatedRequest(
  baseUrl: string,
  path: string,
  options: RequestInit = {},
  retryOn401 = true
): Promise<Response> {
  const authRuntime = getAuthRuntime();
  const token = await authRuntime.getToken();
  if (!token) {
    throw new Error('Not authenticated');
  }

  const requestOptions = withDefaultJsonHeaders(options);
  const headers = new Headers(requestOptions.headers);
  headers.set('Authorization', `Bearer ${token}`);
  headers.set('X-Client-Version', __APP_VERSION__);
  headers.set('X-Client-Type', 'frontend');

  const response = await fetch(`${baseUrl}${path}`, {
    ...requestOptions,
    headers,
    credentials: 'include',
  });

  // Handle 401 - token may have expired
  if (response.status === 401 && retryOn401) {
    const newToken = await authRuntime.triggerRefresh();
    if (newToken) {
      // Retry the request with the new token
      headers.set('Authorization', `Bearer ${newToken}`);
      return fetch(`${baseUrl}${path}`, {
        ...requestOptions,
        headers,
        credentials: 'include',
      });
    }
    // Refresh failed, throw an auth error
    throw new Error('Session expired. Please log in again.');
  }

  return response;
}

export interface BulkUpdateTaskItem {
  id: string;
  changes: Partial<UpdateTaskRequest>;
}

export interface BulkUpdateProjectItem {
  id: string;
  changes: Partial<UpdateProjectRequest>;
}

export async function bulkUpdateProjects(
  updates: BulkUpdateProjectItem[]
): Promise<void> {
  const response = await makeRequest('/v1/projects/bulk', {
    method: 'POST',
    body: JSON.stringify({
      updates: updates.map((u) => ({ id: u.id, ...u.changes })),
    }),
  });
  if (!response.ok) {
    const error = await response.json();
    throw new Error(error.message || 'Failed to bulk update projects');
  }
}

export async function bulkUpdateTasks(
  updates: BulkUpdateTaskItem[],
  hostId = getCurrentHostId()
): Promise<void> {
  const response = await makeRequest(
    '/v1/tasks/bulk',
    {
      method: 'POST',
      body: JSON.stringify({
        updates: updates.map((u) => ({ id: u.id, ...u.changes })),
      }),
    },
    true,
    hostId
  );
  if (!response.ok) {
    const error = await response.json();
    throw new Error(error.message || 'Failed to bulk update tasks');
  }
}

export interface BulkUpdateProjectStatusItem {
  id: string;
  changes: Partial<UpdateProjectStatusRequest>;
}

export async function bulkUpdateProjectStatuses(
  updates: BulkUpdateProjectStatusItem[],
  hostId = getCurrentHostId()
): Promise<void> {
  const response = await makeRequest(
    '/v1/project_statuses/bulk',
    {
      method: 'POST',
      body: JSON.stringify({
        updates: updates.map((u) => ({ id: u.id, ...u.changes })),
      }),
    },
    true,
    hostId
  );
  if (!response.ok) {
    const error = await response.json();
    throw new Error(error.message || 'Failed to bulk update project statuses');
  }
}

// ---------------------------------------------------------------------------
// Relay host API functions (served by remote backend)
// ---------------------------------------------------------------------------

export async function listRelayHosts(): Promise<RelayHost[]> {
  const response = await makeRequest('/v1/hosts', { method: 'GET' });
  if (!response.ok) {
    throw await parseErrorResponse(response, 'Failed to list relay hosts');
  }

  const body = (await response.json()) as ListRelayHostsResponse;
  return body.hosts;
}

// ---------------------------------------------------------------------------
// SAS URL cache with TTL — SAS URLs expire after 5 minutes, cache for 4
// ---------------------------------------------------------------------------

const SAS_URL_TTL_MS = 4 * 60 * 1000;

interface CachedSasUrl {
  url: string;
  expiresAt: number;
}

const sasUrlCache = new Map<string, CachedSasUrl>();

// ---------------------------------------------------------------------------
// Utility: SHA-256 file hash
// ---------------------------------------------------------------------------

export async function computeFileHash(file: File): Promise<string> {
  const buffer = await file.arrayBuffer();
  const hash = await crypto.subtle.digest('SHA-256', buffer);
  return Array.from(new Uint8Array(hash))
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

// ---------------------------------------------------------------------------
// Utility: Upload to Azure Blob Storage with progress
// ---------------------------------------------------------------------------

export function uploadToAzure(
  uploadUrl: string,
  file: File,
  onProgress?: (pct: number) => void
): Promise<void> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('PUT', uploadUrl, true);
    xhr.setRequestHeader('x-ms-blob-type', 'BlockBlob');
    xhr.setRequestHeader('Content-Type', file.type);

    if (onProgress) {
      xhr.upload.addEventListener('progress', (e) => {
        if (e.lengthComputable) {
          onProgress(Math.round((e.loaded / e.total) * 100));
        }
      });
    }

    xhr.onload = () => {
      if (xhr.status === 201) {
        resolve();
      } else {
        reject(
          new Error(
            `Azure upload failed with status ${xhr.status}: ${xhr.statusText}`
          )
        );
      }
    };

    xhr.onerror = () => {
      reject(new Error('Azure upload failed: network error'));
    };

    xhr.send(file);
  });
}

// ---------------------------------------------------------------------------
// Utility: safe error response parsing (handles non-JSON error bodies)
// ---------------------------------------------------------------------------

async function parseErrorResponse(
  response: Response,
  fallbackMessage: string
): Promise<Error> {
  try {
    const body = await response.json();
    const message = body.error || body.message || fallbackMessage;
    return new Error(`${message} (${response.status} ${response.statusText})`);
  } catch {
    return new Error(
      `${fallbackMessage} (${response.status} ${response.statusText})`
    );
  }
}

// ---------------------------------------------------------------------------
// Attachment API functions
// ---------------------------------------------------------------------------

export async function initAttachmentUpload(
  params: InitUploadRequest
): Promise<InitUploadResponse> {
  const response = await makeRequest('/v1/attachments/init', {
    method: 'POST',
    body: JSON.stringify(params),
  });
  if (!response.ok) {
    throw await parseErrorResponse(
      response,
      'Failed to init attachment upload'
    );
  }
  return response.json();
}

export async function confirmAttachmentUpload(
  params: ConfirmUploadRequest
): Promise<AttachmentWithBlob> {
  const response = await makeRequest('/v1/attachments/confirm', {
    method: 'POST',
    body: JSON.stringify(params),
  });
  if (!response.ok) {
    throw await parseErrorResponse(
      response,
      'Failed to confirm attachment upload'
    );
  }
  return response.json();
}

export async function commitTaskAttachments(
  taskId: string,
  request: CommitAttachmentsRequest
): Promise<CommitAttachmentsResponse> {
  const response = await makeRequest(`/v1/tasks/${taskId}/attachments/commit`, {
    method: 'POST',
    body: JSON.stringify(request),
  });
  if (!response.ok) {
    throw await parseErrorResponse(
      response,
      'Failed to commit task attachments'
    );
  }
  return response.json();
}

export async function commitCommentAttachments(
  commentId: string,
  request: CommitAttachmentsRequest
): Promise<CommitAttachmentsResponse> {
  const response = await makeRequest(
    `/v1/comments/${commentId}/attachments/commit`,
    {
      method: 'POST',
      body: JSON.stringify(request),
    }
  );
  if (!response.ok) {
    throw await parseErrorResponse(
      response,
      'Failed to commit comment attachments'
    );
  }
  return response.json();
}

export async function deleteAttachment(attachmentId: string): Promise<void> {
  const response = await makeRequest(`/v1/attachments/${attachmentId}`, {
    method: 'DELETE',
  });
  if (!response.ok) {
    throw await parseErrorResponse(response, 'Failed to delete attachment');
  }
}

export async function fetchAttachmentSasUrl(
  attachmentId: string,
  type: 'file' | 'thumbnail'
): Promise<string> {
  const cacheKey = `${attachmentId}:${type}`;
  const cached = sasUrlCache.get(cacheKey);
  if (cached && Date.now() < cached.expiresAt) {
    return cached.url;
  }

  const response = await makeRequest(`/v1/attachments/${attachmentId}/${type}`);
  if (!response.ok) {
    throw new Error(
      `Failed to fetch attachment ${type}: ${response.statusText}`
    );
  }

  const data: AttachmentUrlResponse = await response.json();
  sasUrlCache.set(cacheKey, {
    url: data.url,
    expiresAt: Date.now() + SAS_URL_TTL_MS,
  });
  return data.url;
}
