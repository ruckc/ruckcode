/** Layout of the mode transition graph (pure, so it can be tested). */
import type { ModeDto } from '../host/modes';
import { displayName } from './doc';

export const NODE_W = 150;
export const NODE_H = 46;
export const COL_GAP = 60;
export const ROW_GAP = 28;
const PAD = 16;

export interface FlowNode {
  id: string;
  label: string;
  x: number;
  y: number;
  col: number;
  row: number;
  isDefault: boolean;
  /** `next` is empty: any other mode may be proposed. */
  anyTarget: boolean;
}

export interface FlowEdge {
  from: string;
  to: string;
  /** Skips user confirmation. */
  auto: boolean;
  path: string;
  /** Where to put a label. */
  mid: { x: number; y: number };
}

export interface Flow {
  nodes: FlowNode[];
  edges: FlowEdge[];
  width: number;
  height: number;
}

type Slim = Pick<ModeDto, 'id' | 'name' | 'next' | 'autoAdvanceTo'>;

/** Edges as (from, to, auto): `autoAdvanceTo` wins over `next`; unknown targets and self are skipped. */
export function transitions(modes: Slim[]): { from: string; to: string; auto: boolean }[] {
  const ids = new Set(modes.map((m) => m.id));
  const out: { from: string; to: string; auto: boolean }[] = [];
  for (const m of modes) {
    const seen = new Set<string>();
    for (const [list, auto] of [
      [m.autoAdvanceTo, true],
      [m.next, false]
    ] as const) {
      for (const to of list) {
        if (to === m.id || !ids.has(to) || seen.has(to)) continue;
        seen.add(to);
        out.push({ from: m.id, to, auto });
      }
    }
  }
  return out;
}

/** Columns by breadth-first distance from the default mode; unreachable modes follow. */
export function layers(modes: Slim[], defaultMode: string): string[][] {
  const edges = transitions(modes);
  const col = new Map<string, number>();
  const order = modes.map((m) => m.id);
  const roots = [defaultMode, ...order].filter(
    (id, i, a) => order.includes(id) && a.indexOf(id) === i
  );
  let base = 0;
  for (const root of roots) {
    if (col.has(root)) continue;
    col.set(root, base);
    const queue = [root];
    while (queue.length) {
      const cur = queue.shift()!;
      for (const e of edges) {
        if (e.from === cur && !col.has(e.to)) {
          col.set(e.to, col.get(cur)! + 1);
          queue.push(e.to);
        }
      }
    }
    base = Math.max(...col.values()) + 1;
  }
  const cols: string[][] = [];
  for (const id of order) {
    const c = col.get(id)!;
    (cols[c] ??= []).push(id);
  }
  return cols.filter(Boolean);
}

export function layout(modes: Slim[], defaultMode: string): Flow {
  const cols = layers(modes, defaultMode);
  const byId = new Map(modes.map((m) => [m.id, m]));
  const nodes: FlowNode[] = [];
  cols.forEach((ids, c) =>
    ids.forEach((id, r) =>
      nodes.push({
        id,
        label: displayName(byId.get(id)!),
        x: PAD + c * (NODE_W + COL_GAP),
        y: PAD + r * (NODE_H + ROW_GAP),
        col: c,
        row: r,
        isDefault: id === defaultMode,
        anyTarget: !byId.get(id)!.next.length && !byId.get(id)!.autoAdvanceTo.length
      })
    )
  );
  const at = new Map(nodes.map((n) => [n.id, n]));
  const raw = transitions(modes);
  const has = (a: string, b: string) => raw.some((e) => e.from === a && e.to === b);
  const edges: FlowEdge[] = raw.map((e) => {
    const a = at.get(e.from)!;
    const b = at.get(e.to)!;
    const both = has(e.to, e.from);
    // Separate the two directions of a pair.
    const off = both ? (e.from < e.to ? -7 : 7) : 0;
    let sx: number,
      sy: number,
      tx: number,
      ty: number,
      c1x: number,
      c1y: number,
      c2x: number,
      c2y: number;
    if (b.col > a.col) {
      sx = a.x + NODE_W;
      sy = a.y + NODE_H / 2 + off;
      tx = b.x;
      ty = b.y + NODE_H / 2 + off;
      const d = (tx - sx) / 2;
      [c1x, c1y, c2x, c2y] = [sx + d, sy, tx - d, ty];
    } else if (b.col < a.col) {
      sx = a.x;
      sy = a.y + NODE_H / 2 + off;
      tx = b.x + NODE_W;
      ty = b.y + NODE_H / 2 + off;
      const d = (sx - tx) / 2;
      const bulge = (a.row === b.row ? NODE_H : 0) * 0.9 + 12;
      [c1x, c1y, c2x, c2y] = [sx - d, sy + bulge, tx + d, ty + bulge];
    } else {
      const down = b.row > a.row;
      sx = a.x + NODE_W / 2 + off;
      sy = down ? a.y + NODE_H : a.y;
      tx = b.x + NODE_W / 2 + off;
      ty = down ? b.y : b.y + NODE_H;
      const d = Math.abs(ty - sy) / 3;
      [c1x, c1y, c2x, c2y] = [sx + 30, sy + (down ? d : -d), tx + 30, ty + (down ? -d : d)];
    }
    return {
      from: e.from,
      to: e.to,
      auto: e.auto,
      path: `M${sx},${sy} C${c1x},${c1y} ${c2x},${c2y} ${tx},${ty}`,
      mid: { x: (sx + tx) / 2, y: (sy + ty) / 2 + (b.col < a.col ? 10 : 0) }
    };
  });
  const maxRows = Math.max(1, ...cols.map((c) => c.length));
  return {
    nodes,
    edges,
    width: PAD * 2 + cols.length * NODE_W + Math.max(0, cols.length - 1) * COL_GAP,
    height: PAD * 2 + maxRows * NODE_H + (maxRows - 1) * ROW_GAP + 24
  };
}
