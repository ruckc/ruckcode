/**
 * Pure mapping from transcript records to the timeline the UI renders, plus the Markdown export.
 * Every record type of `ruckcode_core::transcript` has a timeline item; a `tool_call` and its
 * `tool_result` (same `toolUseId`) are folded into one card.
 */
import { formatDuration } from '../format';
import type { Block, TranscriptMeta, TranscriptRecord } from '../host/transcripts';

type Rec<T extends TranscriptRecord['type']> = Extract<TranscriptRecord, { type: T }>;

export type TimelineItem =
  | { kind: 'session_start'; key: string; indexes: number[]; ts: string; rec: Rec<'session_start'> }
  | { kind: 'user'; key: string; indexes: number[]; ts: string; rec: Rec<'user_message'> }
  | {
      kind: 'assistant';
      key: string;
      indexes: number[];
      ts: string;
      rec: Rec<'assistant_message'>;
    }
  | {
      kind: 'tool';
      key: string;
      indexes: number[];
      ts: string;
      /** Missing when the page that holds the call was not loaded. */
      call: Rec<'tool_call'> | null;
      /** Missing while the result is not loaded (or was never recorded). */
      result: Rec<'tool_result'> | null;
    }
  | { kind: 'hook'; key: string; indexes: number[]; ts: string; rec: Rec<'hook_run'> }
  | { kind: 'mode'; key: string; indexes: number[]; ts: string; rec: Rec<'mode_change'> }
  | { kind: 'model'; key: string; indexes: number[]; ts: string; rec: Rec<'model_change'> }
  | { kind: 'compaction'; key: string; indexes: number[]; ts: string; rec: Rec<'compaction'> }
  | { kind: 'turn_end'; key: string; indexes: number[]; ts: string; rec: Rec<'turn_end'> }
  | { kind: 'mcp'; key: string; indexes: number[]; ts: string; rec: Rec<'mcp_servers'> }
  | { kind: 'resumed'; key: string; indexes: number[]; ts: string; rec: Rec<'session_resumed'> };

export type TimelineKind = TimelineItem['kind'];

export const itemKey = (index: number): string => `r${index}`;

/** Fold the loaded records into timeline items (in record order). */
export function buildTimeline(records: TranscriptRecord[]): TimelineItem[] {
  const results = new Map<string, Rec<'tool_result'>>();
  const calls = new Set<string>();
  for (const r of records) {
    if (r.type === 'tool_result') results.set(r.toolUseId, r);
    else if (r.type === 'tool_call') calls.add(r.toolUseId);
  }
  const out: TimelineItem[] = [];
  for (const r of records) {
    const base = { key: itemKey(r.index), indexes: [r.index], ts: r.ts };
    switch (r.type) {
      case 'session_start':
        out.push({ ...base, kind: 'session_start', rec: r });
        break;
      case 'user_message':
        out.push({ ...base, kind: 'user', rec: r });
        break;
      case 'assistant_message':
        out.push({ ...base, kind: 'assistant', rec: r });
        break;
      case 'tool_call': {
        const result = results.get(r.toolUseId) ?? null;
        out.push({
          ...base,
          indexes: result ? [r.index, result.index] : [r.index],
          kind: 'tool',
          call: r,
          result
        });
        break;
      }
      case 'tool_result':
        // Paired results are shown on their call's card.
        if (!calls.has(r.toolUseId)) {
          out.push({ ...base, kind: 'tool', call: null, result: r });
        }
        break;
      case 'hook_run':
        out.push({ ...base, kind: 'hook', rec: r });
        break;
      case 'mode_change':
        out.push({ ...base, kind: 'mode', rec: r });
        break;
      case 'model_change':
        out.push({ ...base, kind: 'model', rec: r });
        break;
      case 'compaction':
        out.push({ ...base, kind: 'compaction', rec: r });
        break;
      case 'turn_end':
        out.push({ ...base, kind: 'turn_end', rec: r });
        break;
      case 'mcp_servers':
        out.push({ ...base, kind: 'mcp', rec: r });
        break;
      case 'session_resumed':
        out.push({ ...base, kind: 'resumed', rec: r });
        break;
    }
  }
  return out;
}

/** Key of the timeline item that shows record `index` (a paired result maps to its call). */
export function keyForRecord(items: TimelineItem[], index: number): string | null {
  return items.find((i) => i.indexes.includes(index))?.key ?? null;
}

export const blocksText = (blocks: Block[], type: 'text' | 'thinking' = 'text'): string =>
  blocks
    .map((b) =>
      b.type === type ? (b.type === 'text' ? b.text : (b as { thinking: string }).thinking) : ''
    )
    .filter(Boolean)
    .join('\n\n');

export const userSourceLabel = (s: string): string =>
  ({ user: 'You', fix_turn: 'Fix turn', hook: 'Hook feedback', tool_results: 'Tool results' })[s] ??
  s;

export const permissionLabel = (p: string): string =>
  ({
    allow: 'allowed',
    allow_always: 'always allowed',
    ask_approved: 'approved by you',
    denied: 'denied by policy',
    rejected: 'rejected by you'
  })[p] ?? p;

/** Short label of a record type for search results and filters. */
export const typeLabel = (t: string): string => t.replace(/_/g, ' ');

// ---- Markdown export -----------------------------------------------------------------------

const fence = (body: string, lang = ''): string => {
  // A fence longer than any backtick run inside the text.
  const longest = Math.max(2, ...[...body.matchAll(/`+/g)].map((m) => m[0].length));
  const f = '`'.repeat(longest + 1);
  return `${f}${lang}\n${body.replace(/\n$/, '')}\n${f}`;
};

function pretty(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2) ?? '';
  } catch {
    return String(v);
  }
}

/** The transcript (as far as `records` covers it) as Markdown. */
export function toMarkdown(meta: TranscriptMeta, records: TranscriptRecord[]): string {
  const out: string[] = [];
  const first = meta.firstPrompt.replace(/\s+/g, ' ').trim();
  out.push(`# Session ${meta.id}`);
  out.push(
    [
      first ? `> ${first}` : '',
      `- Project: \`${meta.cwd}\``,
      `- Created: ${meta.createdAt}`,
      `- Mode: ${meta.mode} · Model: ${meta.model} · Profile: ${meta.profile}`,
      `- ${meta.turns} turns, ${meta.toolCalls} tool calls, ${meta.inputTokens} input and ${meta.outputTokens} output tokens, ${formatDuration(meta.durationMs)}`,
      records.length < meta.recordCount
        ? `- Exported ${records.length} of ${meta.recordCount} records`
        : ''
    ]
      .filter(Boolean)
      .join('\n')
  );
  for (const it of buildTimeline(records)) {
    switch (it.kind) {
      case 'session_start':
        break;
      case 'user':
        out.push(
          `## ${userSourceLabel(it.rec.source)}\n\n${
            it.rec.source === 'tool_results'
              ? `_${it.rec.content.filter((b) => b.type === 'tool_result').length} tool results_`
              : blocksText(it.rec.content)
          }`
        );
        break;
      case 'assistant': {
        const m = it.rec.metrics;
        const head = `## Assistant (${it.rec.model}, request ${it.rec.requestId}, ${formatDuration(m.durationMs)}, ${it.rec.usage.inputTokens} in / ${it.rec.usage.outputTokens} out)`;
        const thinking = blocksText(it.rec.content, 'thinking');
        out.push(
          [
            head,
            thinking ? `<details><summary>Thinking</summary>\n\n${thinking}\n\n</details>` : '',
            blocksText(it.rec.content)
          ]
            .filter(Boolean)
            .join('\n\n')
        );
        break;
      }
      case 'tool': {
        const title = it.call?.title ?? it.result?.name ?? 'tool';
        const parts = [`### Tool: ${title}`];
        const meta2 = [
          it.result ? `permission: ${permissionLabel(it.result.permission)}` : '',
          it.result ? formatDuration(it.result.durationMs) : '',
          it.result?.isError ? 'error' : ''
        ].filter(Boolean);
        if (meta2.length) parts.push(`_${meta2.join(' · ')}_`);
        if (it.call) parts.push(fence(pretty(it.call.input), 'json'));
        for (const e of it.result?.edits ?? []) parts.push(fence(e.diff, 'diff'));
        if (it.result?.output) parts.push(fence(it.result.output));
        out.push(parts.join('\n\n'));
        break;
      }
      case 'hook':
        out.push(
          `### Hook ${it.rec.event}: \`${it.rec.command}\`\n\n_${it.rec.success ? 'passed' : `failed (exit ${it.rec.exitCode ?? '?'})`} · ${formatDuration(it.rec.durationMs)}_${it.rec.output ? `\n\n${fence(it.rec.output)}` : ''}`
        );
        break;
      case 'mode':
        out.push(`_Mode ${it.rec.from} → ${it.rec.to} (${it.rec.initiatedBy}): ${it.rec.reason}_`);
        break;
      case 'model':
        out.push(`_Model ${it.rec.from ?? 'default'} → ${it.rec.to ?? 'default'}_`);
        break;
      case 'compaction':
        out.push(
          `### Compaction (${it.rec.replacedMessages} messages replaced)\n\n${it.rec.summary}`
        );
        break;
      case 'turn_end':
        out.push(
          `_Turn ended: ${it.rec.stopReason}, ${formatDuration(it.rec.durationMs)}, ${it.rec.modelRequests} requests, ${it.rec.toolCalls} tool calls, ${it.rec.inputTokens} in / ${it.rec.outputTokens} out_`
        );
        break;
      case 'mcp':
        out.push(
          `### MCP servers\n\n${it.rec.servers
            .map(
              (s) => `- ${s.name}: ${s.ok ? `${s.tools} tools` : `failed: ${s.error ?? 'unknown'}`}`
            )
            .join('\n')}`
        );
        break;
      case 'resumed':
        out.push('_Session resumed_');
        break;
    }
  }
  return out.join('\n\n') + '\n';
}
