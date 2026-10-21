import { describe, expect, it } from 'vitest';
import { parseRecommended, relativeTime, summarizeInput, truncateLines } from './format';

describe('format helpers', () => {
  it('summarizes raw input', () => {
    expect(summarizeInput(null)).toEqual([]);
    expect(summarizeInput('ls')).toEqual(['ls']);
    expect(summarizeInput({ command: 'a\nb', n: 3 })).toEqual(['command: a b', 'n: 3']);
    expect(summarizeInput({ s: 'x'.repeat(50) }, 10)[0].length).toBeLessThanOrEqual(14);
  });

  it('truncates by lines', () => {
    expect(truncateLines('a\nb\nc', 2)).toEqual({ shown: 'a\nb', hidden: 1 });
    expect(truncateLines('a', 2)).toEqual({ shown: 'a', hidden: 0 });
  });

  it('formats relative times', () => {
    const now = Date.parse('2026-01-02T00:00:00Z');
    expect(relativeTime(null, now)).toBe('');
    expect(relativeTime('2026-01-01T23:59:50Z', now)).toBe('just now');
    expect(relativeTime('2026-01-01T23:30:00Z', now)).toBe('30m ago');
    expect(relativeTime('2026-01-01T20:00:00Z', now)).toBe('4h ago');
    expect(relativeTime('2025-12-30T00:00:00Z', now)).toBe('3d ago');
  });

  it('detects the (Recommended) marker', () => {
    expect(parseRecommended('sqlite (Recommended)')).toEqual({
      label: 'sqlite',
      recommended: true
    });
    expect(parseRecommended('postgres')).toEqual({ label: 'postgres', recommended: false });
  });
});
