import { describe, expect, it } from 'vitest';
import {
  baseName,
  hostLabel,
  loadRecent,
  MAX_RECENT,
  pushRecent,
  removeRecent,
  saveRecent,
  type Project
} from './projects';

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

const local = (path: string): Project => ({ path, host: 'local' });

describe('recent projects', () => {
  it('moves a reopened project to the front, dedupes and caps the list', () => {
    let l = pushRecent([], local('/a'));
    l = pushRecent(l, local('/b'));
    l = pushRecent(l, local('/a'));
    expect(l).toEqual([local('/a'), local('/b')]);
    for (let i = 0; i < 20; i++) l = pushRecent(l, local(`/p${i}`));
    expect(l).toHaveLength(MAX_RECENT);
    expect(pushRecent(l, local('  '))).toBe(l);
    expect(removeRecent([local('/a'), local('/b')], local('/a'))).toEqual([local('/b')]);
  });

  it('keeps the same path on different hosts or distros apart', () => {
    const wsl = (distro: string): Project => ({ path: '/home/x', host: 'wsl', distro });
    let l = pushRecent([], local('/home/x'));
    l = pushRecent(l, wsl('Ubuntu'));
    l = pushRecent(l, wsl('Debian'));
    l = pushRecent(l, wsl('Ubuntu'));
    expect(l.map((p) => hostLabel(p))).toEqual(['WSL: Ubuntu', 'WSL: Debian', '']);
    expect(removeRecent(l, wsl('Debian'))).toHaveLength(2);
    expect(hostLabel({ path: '/x', host: 'subprocess', rcPath: '/bin/rc' })).toBe('custom rc');
  });

  it('round-trips host choice through storage and migrates old formats', () => {
    const s = memory();
    const list: Project[] = [
      { path: '\\\\wsl$\\U\\home', host: 'wsl', distro: 'U' },
      { path: '/b', host: 'subprocess', rcPath: '/opt/rc' },
      local('/c')
    ];
    saveRecent(list, s);
    expect(loadRecent(s)).toEqual(list);
    // v1 stored plain strings; the very first version a single folder
    expect(loadRecent(memory({ 'ruckcode.recent': '["/a","/b"]' }))).toEqual([
      local('/a'),
      local('/b')
    ]);
    expect(loadRecent(memory({ 'ruckcode.cwd': '/old' }))).toEqual([local('/old')]);
    // junk entries and unknown hosts degrade gracefully
    expect(
      loadRecent(memory({ 'ruckcode.recent': '[1,{"path":"/x","host":"zzz"},{"host":"wsl"}]' }))
    ).toEqual([local('/x')]);
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
    expect(() => saveRecent([local('/a')], boom)).not.toThrow();
    expect(loadRecent(null)).toEqual([]);
  });

  it('takes the last path segment', () => {
    expect(baseName('/home/me/proj/')).toBe('proj');
    expect(baseName('C:\\work\\app')).toBe('app');
  });
});
