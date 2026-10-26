/**
 * Render filesystem paths without Windows' extended-length transport prefix.
 * Keep the original path for filesystem operations, requests and identity.
 * Device and volume paths have no equivalent ordinary path and stay intact.
 */
export function formatPathForDisplay(path: string): string {
  if (/^\\\\\?\\UNC\\/i.test(path)) {
    return '\\\\' + path.slice(8);
  }
  if (/^\\\\\?\\[a-z]:\\/i.test(path)) {
    return path.slice(4);
  }
  return path;
}
