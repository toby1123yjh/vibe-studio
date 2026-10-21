export interface MarkdownSegment {
  offset: number;
  content: string;
  provisional: boolean;
}

/** Only close blocks at blank lines outside fenced code. The unfinished tail
 * stays bounded by the server's body preview; completed snapshots are parsed
 * together again, resolving reference definitions and incomplete Markdown.
 */
export function segmentStreamingMarkdown(
  content: string,
  finished: boolean
): MarkdownSegment[] {
  if (finished) return [{ offset: 0, content, provisional: false }];
  const segments: MarkdownSegment[] = [];
  let fence: { marker: string; length: number } | null = null;
  let offset = 0;
  let start = 0;
  for (const line of content.split(/(?<=\n)/)) {
    const marker = /^ {0,3}(`{3,}|~{3,})/.exec(line);
    if (marker) {
      if (!fence) fence = { marker: marker[1][0], length: marker[1].length };
      else if (
        marker[1][0] === fence.marker &&
        marker[1].length >= fence.length &&
        /^\s*$/.test(line.slice(marker[0].length))
      )
        fence = null;
    }
    offset += line.length;
    if (!fence && /^\s*\n$/.test(line) && offset - start >= 1024) {
      segments.push({
        offset: start,
        content: content.slice(start, offset),
        provisional: false,
      });
      start = offset;
    }
  }
  if (start < content.length || !segments.length)
    segments.push({
      offset: start,
      content: content.slice(start),
      provisional: Boolean(fence) || content.length - start > 8192,
    });
  return segments;
}
