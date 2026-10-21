import { structuredPatch } from 'diff';

export type RowType = 'ctx' | 'add' | 'del';

export interface DiffRow {
  type: RowType;
  text: string;
  oldNo: number | null;
  newNo: number | null;
}

export interface Hunk {
  header: string;
  rows: DiffRow[];
}

export interface DiffModel {
  hunks: Hunk[];
  added: number;
  removed: number;
}

/** Compute a unified diff model (3 lines of context) from old/new text. */
export function buildDiff(oldText: string | null, newText: string, path = 'file'): DiffModel {
  const patch = structuredPatch(path, path, oldText ?? '', newText, '', '', { context: 3 });
  let added = 0;
  let removed = 0;
  const hunks = patch.hunks.map((h) => {
    let o = h.oldStart;
    let n = h.newStart;
    const rows: DiffRow[] = [];
    for (const line of h.lines) {
      const tag = line[0];
      if (tag === '\\') continue; // "\ No newline at end of file"
      const text = line.slice(1);
      if (tag === '+') {
        rows.push({ type: 'add', text, oldNo: null, newNo: n++ });
        added++;
      } else if (tag === '-') {
        rows.push({ type: 'del', text, oldNo: o++, newNo: null });
        removed++;
      } else rows.push({ type: 'ctx', text, oldNo: o++, newNo: n++ });
    }
    return {
      header: `@@ -${h.oldStart},${h.oldLines} +${h.newStart},${h.newLines} @@`,
      rows
    };
  });
  return { hunks, added, removed };
}

/**
 * Parse unified-diff text (as stored in transcripts) into the same model `buildDiff` produces.
 * File headers (`---`, `+++`, `diff ...`) before the first hunk are ignored, and text without any
 * hunk header yields no hunks.
 */
export function parseUnifiedDiff(text: string): DiffModel {
  const hunks: Hunk[] = [];
  let added = 0;
  let removed = 0;
  let cur: Hunk | null = null;
  let o = 0;
  let n = 0;
  for (const line of text.replace(/\r?\n$/, '').split(/\r?\n/)) {
    const m = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (m) {
      o = Number(m[1]);
      n = Number(m[2]);
      cur = { header: line.replace(/^(@@ [^@]+@@).*$/, '$1'), rows: [] };
      hunks.push(cur);
      continue;
    }
    if (!cur || line[0] === '\\') continue;
    const tag = line[0];
    const body = line.slice(1);
    if (tag === '+') {
      cur.rows.push({ type: 'add', text: body, oldNo: null, newNo: n++ });
      added++;
    } else if (tag === '-') {
      cur.rows.push({ type: 'del', text: body, oldNo: o++, newNo: null });
      removed++;
    } else if (tag === ' ' || line === '') {
      cur.rows.push({ type: 'ctx', text: body, oldNo: o++, newNo: n++ });
    }
  }
  return { hunks, added, removed };
}

export interface SideRow {
  left: DiffRow | null;
  right: DiffRow | null;
}

/** Pair up removals and additions of one hunk for a side-by-side view. */
export function sideBySide(rows: DiffRow[]): SideRow[] {
  const out: SideRow[] = [];
  let dels: DiffRow[] = [];
  let adds: DiffRow[] = [];
  const flush = () => {
    const n = Math.max(dels.length, adds.length);
    for (let i = 0; i < n; i++) out.push({ left: dels[i] ?? null, right: adds[i] ?? null });
    dels = [];
    adds = [];
  };
  for (const r of rows) {
    if (r.type === 'del') dels.push(r);
    else if (r.type === 'add') adds.push(r);
    else {
      flush();
      out.push({ left: r, right: r });
    }
  }
  flush();
  return out;
}
