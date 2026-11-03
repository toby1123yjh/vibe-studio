import { memo, useEffect, useMemo, useRef, useState } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';
import { Check, Clipboard } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Button } from '@vibe/ui/components/Button';
import {
  isExternalReadOnlyHref,
  sanitizeReadOnlyHref,
} from '@vibe/ui/lib/read-only-link';
import { writeClipboardViaBridge } from '@/shared/lib/clipboard';
import { cn } from '@/shared/lib/utils';
import './read-only-chat-markdown.css';

const remarkPlugins = [remarkGfm];
const rehypePlugins = [rehypeHighlight];

interface ReadOnlyChatMarkdownProps {
  content: string;
  className?: string;
  findMatchingDiffPath: (text: string) => string | null;
  onCodeClick: (path: string) => void;
}

/** A display-only projection: no editor state, selection listeners, discovery,
 * or writable Markdown synchronization is mounted for ordinary history rows. */
export const ReadOnlyChatMarkdown = memo(function ReadOnlyChatMarkdown({
  content,
  className,
  findMatchingDiffPath,
  onCodeClick,
}: ReadOnlyChatMarkdownProps) {
  const { t } = useTranslation('common');
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => () => clearTimeout(copyTimer.current), []);

  const components = useMemo<Components>(
    () => ({
      a: ({ children, href, title }) => {
        const safeHref = sanitizeReadOnlyHref(href);
        return isExternalReadOnlyHref(safeHref) ? (
          <a
            href={safeHref}
            title={title}
            target="_blank"
            rel="noopener noreferrer"
            onClick={(event) => event.stopPropagation()}
          >
            {children}
          </a>
        ) : (
          <a role="link" aria-disabled="true" title={title ?? href}>
            {children}
          </a>
        );
      },
      code: ({ children, className: codeClassName }) => {
        // CommonMark appends a newline to fenced/indented blocks, including a
        // one-line block. Only inline code participates in file navigation.
        const path =
          !codeClassName &&
          typeof children === 'string' &&
          !children.includes('\n')
            ? findMatchingDiffPath(children.trim())
            : null;
        return path ? (
          <code>
            <button
              type="button"
              className="clickable-code cursor-pointer text-left underline underline-offset-2 focus-visible:outline focus-visible:outline-2"
              title={path}
              onClick={(event) => {
                event.stopPropagation();
                onCodeClick(path);
              }}
            >
              {children}
            </button>
          </code>
        ) : (
          <code className={codeClassName}>{children}</code>
        );
      },
      table: ({ children }) => (
        <div className="max-w-full overflow-x-auto">
          <table>{children}</table>
        </div>
      ),
    }),
    [findMatchingDiffPath, onCodeClick]
  );

  const copyLabel = copied
    ? t('actions.copied', { defaultValue: 'Copied' })
    : t('actions.copyMarkdown', { defaultValue: 'Copy as Markdown' });

  return (
    <div className="relative group">
      <div className="sticky top-0 right-2 z-10 pointer-events-none h-0">
        <div className="flex justify-end opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity duration-150 motion-reduce:transition-none">
          <Button
            type="button"
            aria-label={copyLabel}
            title={copyLabel}
            variant="icon"
            size="icon"
            className="pointer-events-auto p-2 bg-muted h-8 w-8"
            onClick={async (event) => {
              event.stopPropagation();
              await writeClipboardViaBridge(content.replace(/\\_/g, '_'));
              setCopied(true);
              clearTimeout(copyTimer.current);
              copyTimer.current = setTimeout(() => setCopied(false), 400);
            }}
          >
            {copied ? (
              <Check className="w-4 h-4 text-success" />
            ) : (
              <Clipboard className="w-4 h-4 text-muted-foreground" />
            )}
          </Button>
        </div>
      </div>
      <div className={cn('chat-markdown text-base', className)}>
        <ReactMarkdown
          remarkPlugins={remarkPlugins}
          rehypePlugins={rehypePlugins}
          components={components}
        >
          {content}
        </ReactMarkdown>
      </div>
    </div>
  );
});
