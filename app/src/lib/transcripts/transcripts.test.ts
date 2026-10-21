import { describe, expect, it } from 'vitest';
import { FakeTranscripts, seedSessions } from '../host/fake-transcripts';
import type { RecordType, TranscriptRecord } from '../host/transcripts';
import { parseUnifiedDiff } from '../diff';
import { distinct, filterItems, sortItems } from './list';
import { computeMetrics } from './metrics';
import { buildTimeline, keyForRecord, toMarkdown } from './timeline';

const ALL_TYPES: RecordType[] = [
  'session_start',
  'user_message',
  'assistant_message',
  'tool_call',
  'tool_result',
  'hook_run',
  'mode_change',
  'model_change',
  'compaction',
  'turn_end',
  'session_resumed',
  'mcp_servers'
];

const target = { project: '/demo/project' };
const api = new FakeTranscripts({ delayMs: 0 });
const seed = (id: string) => seedSessions().find((s) => s.id === id)!;
const auth = () => seed('20260301-101500-a1b2c3');

describe('seed data', () => {
  it('has every record type, a large transcript and a corrupt one', () => {
    const types = new Set(seedSessions().flatMap((s) => s.records.map((r) => r.type)));
    for (const t of ALL_TYPES) expect(types.has(t), t).toBe(true);
    expect(seedSessions().length).toBeGreaterThanOrEqual(5);
    expect(seed('20260304-120000-d4e5f6').records.length).toBeGreaterThan(1900);
    expect(seed('20260305-160000-e5f6a7').skippedLines).toBe(1);
    // Indexes are consecutive from zero.
    for (const s of seedSessions()) s.records.forEach((r, i) => expect(r.index).toBe(i));
  });
});

describe('buildTimeline', () => {
  it('maps every record type to an item and folds tool results into their call', () => {
    const recs = auth().records;
    const items = buildTimeline(recs);
    const kinds = new Set(items.map((i) => i.kind));
    for (const k of [
      'session_start',
      'user',
      'assistant',
      'tool',
      'hook',
      'mode',
      'model',
      'compaction',
      'turn_end',
      'mcp',
      'resumed'
    ]) {
      expect(kinds.has(k as never), k).toBe(true);
    }
    // 3 tool calls, 3 results, but 3 cards.
    expect(items.filter((i) => i.kind === 'tool')).toHaveLength(3);
    expect(items.length).toBe(recs.length - 3);
    const edit = items.find((i) => i.kind === 'tool' && i.call?.name === 'Edit');
    expect(edit?.kind === 'tool' && edit.result?.permission).toBe('ask_approved');
    expect(edit?.kind === 'tool' && edit.result?.edits).toHaveLength(1);
    // A search hit on the result record lands on the call's card.
    const resultIndex = recs.find((r) => r.type === 'tool_result')!.index;
    expect(keyForRecord(items, resultIndex)).toBe(items.find((i) => i.kind === 'tool')!.key);
    expect(keyForRecord(items, 99999)).toBeNull();
  });

  it('shows calls without a loaded result and results without a loaded call', () => {
    const recs = auth().records;
    const call = recs.findIndex((r) => r.type === 'tool_call');
    const head = buildTimeline(recs.slice(0, call + 1));
    const last = head.at(-1)!;
    expect(last.kind === 'tool' && last.call && !last.result).toBe(true);
    const tail = buildTimeline(recs.slice(call + 1));
    expect(tail[0].kind).toBe('tool');
    expect(tail[0].kind === 'tool' && tail[0].call === null && tail[0].result !== null).toBe(true);
  });
});

describe('toMarkdown', () => {
  it('renders every kind of record', async () => {
    const r = await api.read(target, auth().id);
    const md = toMarkdown(r.meta, r.records);
    expect(md).toContain('# Session 20260301-101500-a1b2c3');
    expect(md).toContain('## You');
    expect(md).toContain('Fix the login bug');
    expect(md).toContain('## Assistant (mock-large');
    expect(md).toContain('<summary>Thinking</summary>');
    expect(md).toContain('### Tool: Edit src/auth.rs');
    expect(md).toContain('```diff');
    expect(md).toContain('permission: approved by you');
    expect(md).toContain('### Hook post_tool: `cargo clippy -- -D warnings`');
    expect(md).toContain('failed (exit 101)');
    expect(md).toContain('_Mode explore → implement (agent)');
    expect(md).toContain('_Model default → mock-small_');
    expect(md).toContain('### Compaction (6 messages replaced)');
    expect(md).toContain('_Turn ended: end_turn');
    expect(md).toContain('github: failed: spawn github-mcp');
    expect(md).toContain('_Session resumed_');
  });

  it('uses a longer fence than any backticks in the content and flags partial exports', async () => {
    const r = await api.read(target, auth().id, { limit: 5 });
    const rec = r.records.find((x) => x.type === 'user_message')!;
    const fake = {
      ...rec,
      index: 99,
      type: 'tool_result',
      toolUseId: 'z',
      name: 'Bash',
      output: 'a ``` b',
      isError: false,
      permission: 'allow',
      durationMs: 1,
      edits: []
    } as unknown as TranscriptRecord;
    const md = toMarkdown(r.meta, [fake]);
    expect(md).toContain('````\na ``` b\n````');
    expect(md).toContain('Exported 1 of');
  });
});

describe('metrics', () => {
  it('computes per request and per turn values', () => {
    const m = computeMetrics(auth().records);
    expect(m.requests).toHaveLength(5);
    expect(m.requests[0]).toMatchObject({ n: 1, durationMs: 1400, ttftMs: 310, inputTokens: 1200 });
    expect(m.requests.at(-1)!.cumInput).toBe(1200 + 1500 + 1700 + 900 + 2100);
    expect(m.turns).toHaveLength(2);
    expect(m.totals.requests).toBe(5);
    expect(m.totals.maxDurationMs).toBe(2100);
    expect(m.totals.avgTtftMs).toBeGreaterThan(0);
    expect(computeMetrics([]).totals.avgDurationMs).toBeNull();
  });
});

describe('list helpers', () => {
  it('sorts, filters and lists distinct values', async () => {
    const items = await api.list(target);
    expect(items).toHaveLength(5);
    expect(sortItems(items, 'updated')[0].id).toBe('20260305-160000-e5f6a7');
    expect(sortItems(items, 'updated', true)[0].id).toBe('20260301-101500-a1b2c3');
    expect(sortItems(items, 'created')[0].id).toBe('20260305-160000-e5f6a7');
    expect(sortItems(items, 'tokens')[0].id).toBe('20260304-120000-d4e5f6');
    expect(sortItems(items, 'duration')[0].id).toBe('20260304-120000-d4e5f6');
    expect(filterItems(items, { mode: 'plan' }).map((s) => s.id)).toEqual([
      '20260302-143000-b2c3d4'
    ]);
    expect(filterItems(items, { model: 'mock-small' }).length).toBeGreaterThanOrEqual(2);
    expect(filterItems(items, { mode: 'plan', model: 'mock-small' })).toEqual([]);
    expect(distinct(items, 'mode')).toContain('implement');
  });
});

describe('fake api', () => {
  it('pages, searches and rejects bad ids', async () => {
    const page = await api.read(target, '20260304-120000-d4e5f6', { offset: 1000, limit: 50 });
    expect(page.records).toHaveLength(50);
    expect(page.records[0].index).toBe(1000);
    expect(page.total).toBeGreaterThan(1900);
    const hits = await api.search(target, 'ZEBRA');
    expect(hits[0]).toMatchObject({ id: '20260304-120000-d4e5f6', type: 'assistant_message' });
    expect(hits[0].recordIndex).toBeGreaterThan(1500);
    expect(await api.search(target, '   ')).toEqual([]);
    expect((await api.search(target, 'e', 3)).length).toBe(3);
    await expect(api.read(target, '../x')).rejects.toThrow(/invalid session id/);
    await expect(api.read(target, 'nope')).rejects.toThrow(/not found/);
    expect((await api.list(target, 'login')).map((s) => s.id)).toEqual(['20260301-101500-a1b2c3']);
  });
});

describe('parseUnifiedDiff', () => {
  it('parses hunks, counts and line numbers', () => {
    const edit = auth().records.find((r) => r.type === 'tool_result' && r.edits.length)!;
    const text = edit.type === 'tool_result' ? edit.edits[0].diff : '';
    const m = parseUnifiedDiff(text);
    expect(m.hunks).toHaveLength(1);
    expect(m.hunks[0].header).toBe('@@ -38,7 +38,10 @@');
    expect(m.added).toBe(4);
    expect(m.removed).toBe(1);
    expect(m.hunks[0].rows[0]).toMatchObject({ type: 'ctx', oldNo: 38, newNo: 38 });
    expect(m.hunks[0].rows[1]).toMatchObject({ type: 'del', oldNo: 39, newNo: null });
    expect(m.hunks[0].rows[2]).toMatchObject({ type: 'add', oldNo: null, newNo: 39 });
    expect(parseUnifiedDiff('not a diff').hunks).toEqual([]);
    expect(parseUnifiedDiff('@@ -1 +1 @@\n-a\n+b\n\\ No newline at end of file\n').added).toBe(1);
  });
});
