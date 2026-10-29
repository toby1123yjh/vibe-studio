import { forwardRef, type HTMLAttributes } from 'react';

interface ActivityTextProps extends HTMLAttributes<HTMLSpanElement> {
  active?: boolean;
}

export const ActivityText = forwardRef<HTMLSpanElement, ActivityTextProps>(
  function ActivityText({ active = true, className, children, ...props }, ref) {
    return (
      <span ref={ref} className={className} {...props}>
        <span className={active ? 'vk-activity-text' : undefined}>
          {children}
        </span>
      </span>
    );
  }
);
