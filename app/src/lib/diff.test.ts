import { describe, expect, it } from 'vitest';
import { buildDiff, sideBySide } from './diff';

describe('buildDiff', () => {
  it('counts additions and removals with line numbers', () => {
    const d = buildDiff('a\nb\nc\n', 'a\nB\nc\nd\n');
    expect(d.added).toBe(2);
    expect(d.removed).toBe(1);
    const rows = d.hunks[0].rows;
    expect(rows.map((r) => r.type)).toEqual(['ctx', 'del', 'add', 'ctx', 'add']);
    expect(rows[1]).toMatchObject({ text: 'b', oldNo: 2, newNo: null });
    expect(rows[2]).toMatchObject({ text: 'B', oldNo: null, newNo: 2 });
    expect(d.hunks[0].header).toMatch(/^@@ -1,3 \+1,4 @@$/);
  });

  it('treats null oldText as a new file and identical text as no hunks', () => {
    const d = buildDiff(null, 'x\ny\n');
    expect(d.added).toBe(2);
    expect(d.removed).toBe(0);
    expect(buildDiff('same\n', 'same\n').hunks).toEqual([]);
  });

  it('pairs removals with additions side by side', () => {
    const d = buildDiff('a\nb\nc\n', 'a\nB1\nB2\nc\n');
    const pairs = sideBySide(d.hunks[0].rows);
    expect(pairs.map((p) => [p.left?.text ?? null, p.right?.text ?? null])).toEqual([
      ['a', 'a'],
      ['b', 'B1'],
      [null, 'B2'],
      ['c', 'c']
    ]);
  });
});
