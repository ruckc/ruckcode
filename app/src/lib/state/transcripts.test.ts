import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { FakeTranscripts } from '../host/fake-transcripts';
import type { SearchHit, TranscriptsApi } from '../host/transcripts';
import { TranscriptsController } from './transcripts.svelte';

const target = { project: '/demo/project' };
const BIG = '20260304-120000-d4e5f6';
const AUTH = '20260301-101500-a1b2c3';

function make(api: TranscriptsApi = new FakeTranscripts({ delayMs: 0 })) {
  return new TranscriptsController(api, target, { debounceMs: 250 });
}

describe('TranscriptsController', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('loads, sorts and filters the list', async () => {
    const c = make();
    await c.refresh();
    expect(c.items).toHaveLength(5);
    expect(c.visible[0].id).toBe('20260305-160000-e5f6a7');
    c.sort = 'tokens';
    expect(c.visible[0].id).toBe(BIG);
    c.asc = true;
    expect(c.visible.at(0)!.id).not.toBe(BIG);
    c.mode = 'plan';
    expect(c.visible.map((s) => s.id)).toEqual(['20260302-143000-b2c3d4']);
    c.mode = '';
    c.model = 'mock-small';
    expect(c.visible.every((s) => s.model === 'mock-small')).toBe(true);
    expect(c.modes).toContain('explore');
  });

  it('reports list errors', async () => {
    const api: TranscriptsApi = {
      list: () => Promise.reject(new Error('rc: boom')),
      read: () => Promise.reject(new Error('x')),
      search: () => Promise.reject(new Error('x'))
    };
    const c = make(api);
    await c.refresh();
    expect(c.error).toBe('rc: boom');
    expect(c.loading).toBe(false);
  });

  it('debounces search and ignores stale answers', async () => {
    const calls: string[] = [];
    const release: Record<string, (h: SearchHit[]) => void> = {};
    const api: TranscriptsApi = {
      list: async () => [],
      read: async () => {
        throw new Error('unused');
      },
      search: (_t, text) => {
        calls.push(text);
        return new Promise((res) => (release[text] = res));
      }
    };
    const c = make(api);
    c.setSearch('a');
    c.setSearch('ab');
    c.setSearch('abc');
    expect(c.searching).toBe(true);
    await vi.advanceTimersByTimeAsync(249);
    expect(calls).toEqual([]);
    await vi.advanceTimersByTimeAsync(2);
    expect(calls).toEqual(['abc']);
    // A newer query before the first answer arrives.
    c.setSearch('abcd');
    await vi.advanceTimersByTimeAsync(300);
    expect(calls).toEqual(['abc', 'abcd']);
    const hit = (id: string): SearchHit => ({
      id,
      recordIndex: 1,
      type: 'user_message',
      snippet: 's'
    });
    release['abcd']([hit('new')]);
    await vi.advanceTimersByTimeAsync(0);
    release['abc']([hit('stale')]);
    await vi.advanceTimersByTimeAsync(0);
    expect(c.hits.map((h) => h.id)).toEqual(['new']);
    expect(c.searching).toBe(false);
    c.setSearch('  ');
    expect(c.hits).toEqual([]);
    expect(c.searching).toBe(false);
  });

  it('opens a transcript in pages and loads more', async () => {
    const c = make();
    await c.open(BIG);
    expect(c.detail!.records).toHaveLength(200);
    expect(c.detail!.total).toBeGreaterThan(1900);
    expect(c.complete).toBe(false);
    expect(c.remaining).toBe(c.detail!.total - 200);
    await c.loadMore();
    expect(c.detail!.records).toHaveLength(400);
    expect(c.detail!.records[399].index).toBe(399);
    await c.loadAll();
    expect(c.complete).toBe(true);
    expect(c.detail!.records).toHaveLength(c.detail!.total);
    expect(c.timeline.length).toBeLessThan(c.detail!.records.length);
  });

  it('jumps to a record that is not loaded yet', async () => {
    const c = make();
    const seq = c.focusSeq;
    await c.open(BIG, 1500);
    expect(c.detail!.records.length).toBeGreaterThan(1500);
    expect(c.focus).toBe(1500);
    expect(c.focusSeq).toBeGreaterThan(seq);
    expect(c.focusKey).toBeTruthy();
    // Jumping within the open transcript loads nothing new when already loaded.
    const n = c.detail!.records.length;
    await c.open(BIG, 10);
    expect(c.detail!.records.length).toBe(n);
    expect(c.focus).toBe(10);
  });

  it('keeps the latest open when two overlap, and surfaces errors', async () => {
    const c = make();
    const a = c.open(BIG);
    const b = c.open(AUTH);
    await Promise.all([a, b]);
    expect(c.selectedId).toBe(AUTH);
    expect(c.detail!.meta.id).toBe(AUTH);
    await c.open('missing');
    expect(c.detailError).toMatch(/not found/);
    expect(c.detail).toBeNull();
    c.close();
    expect(c.selectedId).toBeNull();
  });

  it('exports the whole transcript as markdown', async () => {
    const c = make();
    await c.open(BIG);
    const md = await c.markdown();
    expect(c.complete).toBe(true);
    expect(md).toContain('Refactor step 95');
    expect(md).not.toContain('Exported');
  });
});
