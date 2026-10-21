import { describe, expect, it } from 'vitest';
import { baseName, loadRecent, MAX_RECENT, pushRecent, removeRecent, saveRecent } from './projects';

function memory(initial: Record<string, string> = {}) {
  const data = { ...initial };
  return {
    data,
    getItem: (k: string) => data[k] ?? null,
    setItem: (k: string, v: string) => {
      data[k] = v;
    }
  };
}

describe('recent projects', () => {
  it('moves a reopened project to the front, dedupes and caps the list', () => {
    let l = pushRecent([], '/a');
    l = pushRecent(l, '/b');
    l = pushRecent(l, '/a');
    expect(l).toEqual(['/a', '/b']);
    for (let i = 0; i < 20; i++) l = pushRecent(l, `/p${i}`);
    expect(l).toHaveLength(MAX_RECENT);
    expect(pushRecent(l, '  ')).toBe(l);
    expect(removeRecent(['/a', '/b'], '/a')).toEqual(['/b']);
  });

  it('round-trips through storage and migrates the old single-folder key', () => {
    const s = memory();
    saveRecent(['/a', '/b'], s);
    expect(loadRecent(s)).toEqual(['/a', '/b']);
    expect(loadRecent(memory({ 'ruckcode.cwd': '/old' }))).toEqual(['/old']);
  });

  it('survives broken or throwing storage', () => {
    expect(loadRecent(memory({ 'ruckcode.recent': '{oops' }))).toEqual([]);
    expect(loadRecent(memory({ 'ruckcode.recent': '{"a":1}' }))).toEqual([]);
    const boom = {
      getItem: () => {
        throw new Error('denied');
      },
      setItem: () => {
        throw new Error('denied');
      }
    };
    expect(loadRecent(boom)).toEqual([]);
    expect(() => saveRecent(['/a'], boom)).not.toThrow();
    expect(loadRecent(null)).toEqual([]);
  });

  it('takes the last path segment', () => {
    expect(baseName('/home/me/proj/')).toBe('proj');
    expect(baseName('C:\\work\\app')).toBe('app');
  });
});
