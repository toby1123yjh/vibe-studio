/** Scope current-file previews from any local API to the selected host. */
export function scopeWorkspaceFileRawUrl(
  rawUrl: string | null | undefined,
  hostId: string | null
): string | null {
  if (!rawUrl) return null;
  if (!hostId) return rawUrl;

  const pathAndQuery = toPathAndQuery(rawUrl);
  if (!pathAndQuery.startsWith('/api/')) return rawUrl;
  if (pathAndQuery.startsWith('/api/host/')) return pathAndQuery;

  return `/api/host/${hostId}${pathAndQuery.slice('/api'.length)}`;
}

function toPathAndQuery(rawUrl: string): string {
  if (/^https?:\/\//i.test(rawUrl)) {
    const url = new URL(rawUrl);
    return `${url.pathname}${url.search}`;
  }

  return rawUrl.startsWith('/') ? rawUrl : `/${rawUrl}`;
}
