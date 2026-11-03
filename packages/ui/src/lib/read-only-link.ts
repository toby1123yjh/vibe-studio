/** Read-only content never navigates the application through relative links. */
export function sanitizeReadOnlyHref(href?: string): string | undefined {
  if (typeof href !== 'string') return undefined;
  const trimmed = href.trim();
  if (/^(javascript|vbscript|data):/i.test(trimmed)) return undefined;
  if (
    trimmed.startsWith('#') ||
    trimmed.startsWith('./') ||
    trimmed.startsWith('../') ||
    trimmed.startsWith('/')
  )
    return trimmed;
  if (/^https:\/\//i.test(trimmed)) return trimmed;
  return undefined;
}

export function isExternalReadOnlyHref(href?: string): boolean {
  return Boolean(href && /^https:\/\//i.test(href));
}
