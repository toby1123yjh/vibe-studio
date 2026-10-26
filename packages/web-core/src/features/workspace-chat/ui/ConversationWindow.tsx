import { useTranslation } from 'react-i18next';
import {
  FloatingPanel,
  type FloatingPanelProps,
} from '@vibe/ui/components/FloatingPanel';
import { cn } from '@vibe/ui/lib/cn';
import { CONVERSATION_PANEL_SIZING } from '@/shared/lib/floatingPanelSizing';

type ConversationWindowProps = Omit<
  FloatingPanelProps,
  'open' | 'onOpenChange' | 'resize' | 'portal' | 'autoFocus' | 'restoreFocus'
> & { onClose: () => void };

/** Shared float shell; business context is supplied above the shared chat. */
export function ConversationWindow({
  onClose,
  className,
  contentClassName,
  ...props
}: ConversationWindowProps) {
  const { t } = useTranslation('common');
  return (
    <FloatingPanel
      {...props}
      open
      portal={false}
      autoFocus={false}
      restoreFocus={false}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      resize={{
        ...CONVERSATION_PANEL_SIZING,
        label: t('panels.resizeWidth', { defaultValue: 'Resize panel width' }),
      }}
      className={cn(
        'absolute bottom-6 right-6 top-6 !max-w-[calc(100%_-_3rem)] max-md:!max-w-none motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-right-2 motion-safe:duration-150',
        className
      )}
      contentClassName={cn(
        'flex min-h-0 flex-col overflow-hidden',
        contentClassName
      )}
    />
  );
}
