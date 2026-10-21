import * as React from 'react';
/** Only the rich editor environment is stubbed: the production block store,
 * renderers, segmentation, lazy body loader and request UI stay in use. */
export default function Markdown({ value }: { value: string }) {
  return (
    <div
      data-testid="markdown"
      style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}
    >
      {value}
    </div>
  );
}
