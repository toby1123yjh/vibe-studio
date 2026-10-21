import { expect, test } from '@playwright/test';
import { segmentStreamingMarkdown } from './streamingMarkdown';

test('streaming keeps completed paragraphs stable and an open code fence provisional', () => {
  const paragraph = `${'intro '.repeat(200)}\n\n`;
  const first = segmentStreamingMarkdown(
    `${paragraph}\`\`\`ts\nconst a = 1;`,
    false
  );
  const second = segmentStreamingMarkdown(
    `${paragraph}\`\`\`ts\nconst a = 1;\nconst b = 2;`,
    false
  );
  expect(first[0]).toEqual(second[0]);
  expect(first[0].provisional).toBe(false);
  expect(second[1].provisional).toBe(true);
  expect(second.map((item) => item.content).join('')).toBe(
    `${paragraph}\`\`\`ts\nconst a = 1;\nconst b = 2;`
  );
});

test('does not split blank lines inside a fence or close it with a different marker', () => {
  const content = `~~~js\n${'x'.repeat(1500)}\n\n\`\`\`\n\n~~~\n\nfinal`;
  const segments = segmentStreamingMarkdown(content, false);
  expect(segments).toHaveLength(2);
  expect(segments[0].content.endsWith('~~~\n\n')).toBe(true);
  expect(segments[0].provisional).toBe(false);
});

test('final replacement reparses as one document and replaces a long provisional tail', () => {
  expect(segmentStreamingMarkdown('x'.repeat(9000), false)[0].provisional).toBe(
    true
  );
  const final = '[link][ref]\n\n[ref]: https://example.com\n';
  expect(segmentStreamingMarkdown(final, true)).toEqual([
    { offset: 0, content: final, provisional: false },
  ]);
});
