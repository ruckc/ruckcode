/** Sorting and filtering of the transcript list (client side; search is server side). */
import type { SessionItem } from '../host/transcripts';

export type SortKey = 'updated' | 'created' | 'tokens' | 'duration';

export const SORT_LABELS: Record<SortKey, string> = {
  updated: 'Last updated',
  created: 'Created',
  tokens: 'Tokens',
  duration: 'Duration'
};

const value = (s: SessionItem, k: SortKey): string | number =>
  k === 'updated'
    ? s.updatedAt
    : k === 'created'
      ? s.createdAt
      : k === 'tokens'
        ? s.inputTokens + s.outputTokens
        : s.durationMs;

/** Sorted copy, descending unless `asc`; ties fall back to the id (newest first). */
export function sortItems(items: SessionItem[], key: SortKey, asc = false): SessionItem[] {
  const dir = asc ? 1 : -1;
  return [...items].sort((a, b) => {
    const x = value(a, key);
    const y = value(b, key);
    const c = x < y ? -1 : x > y ? 1 : 0;
    return c !== 0 ? c * dir : b.id.localeCompare(a.id);
  });
}

export function filterItems(
  items: SessionItem[],
  f: { mode?: string; model?: string }
): SessionItem[] {
  return items.filter((s) => (!f.mode || s.mode === f.mode) && (!f.model || s.model === f.model));
}

export const distinct = (items: SessionItem[], k: 'mode' | 'model'): string[] =>
  [...new Set(items.map((s) => s[k]))].sort();
