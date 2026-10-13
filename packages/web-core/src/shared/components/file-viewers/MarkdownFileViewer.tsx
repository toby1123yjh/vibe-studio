import { MarkdownPreview } from '@/shared/components/MarkdownPreview';
import { getActualTheme } from '@/shared/lib/theme';
import { useTheme } from '@/shared/hooks/useTheme';

export interface MarkdownFileViewerProps {
  content: string;
}

export function MarkdownFileViewer({ content }: MarkdownFileViewerProps) {
  const { theme } = useTheme();
  const actualTheme = getActualTheme(theme);

  return (
    <div className="h-full min-h-0 overflow-auto bg-primary px-double py-base">
      <MarkdownPreview
        content={content}
        theme={actualTheme}
        className="mx-auto max-w-[820px] [&_pre]:max-w-full"
      />
    </div>
  );
}
