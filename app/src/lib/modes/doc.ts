/** Pure operations on the modes document: edits, reference handling and validation. */
import {
  DEFAULT_MAX_FIX_TURNS,
  DEFAULT_TIMEOUT_SECS,
  HOOK_EVENT_NAMES,
  type Catalog,
  type HookDto,
  type HookEventName,
  type Issue,
  type ModeDto,
  type ModesDoc,
  type TemplateInfo
} from '../host/modes';
import { globProblem, matcherProblem } from './glob';

export const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

export const ID_RE = /^[A-Za-z0-9_-]{1,64}$/;

export function emptyMode(id: string): ModeDto {
  return {
    id,
    name: null,
    description: '',
    prompt: '',
    tools: null,
    miseTasks: [],
    mcpTools: [],
    next: [],
    autoAdvanceTo: [],
    profile: null,
    model: null,
    permissions: {},
    hooks: {}
  };
}

export function newHook(): HookDto {
  return {
    run: '',
    glob: null,
    matcher: null,
    onFailure: 'ignore',
    maxFixTurns: DEFAULT_MAX_FIX_TURNS,
    timeoutSecs: DEFAULT_TIMEOUT_SECS
  };
}

/** Display name: the explicit name, else the capitalized id. */
export function displayName(m: Pick<ModeDto, 'id' | 'name'>): string {
  const n = m.name?.trim();
  return n || m.id.charAt(0).toUpperCase() + m.id.slice(1);
}

/** `base`, else `base-2`, `base-3`, ... not in `taken`. */
export function uniqueId(base: string, taken: string[]): string {
  const clean = base.replace(/[^A-Za-z0-9_-]/g, '-').slice(0, 60) || 'mode';
  if (!taken.includes(clean)) return clean;
  for (let i = 2; ; i++) if (!taken.includes(`${clean}-${i}`)) return `${clean}-${i}`;
}

/** Canonical form: blank strings become null, empty hook lists vanish. */
export function normalizeDoc(doc: ModesDoc): ModesDoc {
  const blank = (s: string | null | undefined) => (s && s.trim() ? s : null);
  return {
    defaultMode: doc.defaultMode,
    modes: doc.modes.map((m) => {
      const hooks: ModeDto['hooks'] = {};
      for (const ev of HOOK_EVENT_NAMES) {
        const list = m.hooks[ev];
        if (list?.length)
          hooks[ev] = list.map((h) => ({ ...h, glob: blank(h.glob), matcher: blank(h.matcher) }));
      }
      const out: ModeDto = {
        ...m,
        name: blank(m.name),
        profile: blank(m.profile),
        model: blank(m.model),
        hooks
      };
      if (m.originalId === undefined) delete out.originalId;
      return out;
    })
  };
}

/** Same content, ignoring formatting-only differences (blank vs null, empty hook lists). */
export function docsEqual(a: ModesDoc, b: ModesDoc): boolean {
  return JSON.stringify(normalizeDoc(a)) === JSON.stringify(normalizeDoc(b));
}

export function modeIndex(doc: ModesDoc, id: string): number {
  return doc.modes.findIndex((m) => m.id === id);
}

/** Add a template as a new mode; references to modes this document lacks are dropped. */
export function modeFromTemplate(t: TemplateInfo, doc: ModesDoc): ModeDto {
  const ids = doc.modes.map((m) => m.id);
  const m = clone(t.mode);
  delete m.originalId;
  m.id = uniqueId(m.id, ids);
  m.next = m.next.filter((r) => ids.includes(r));
  m.autoAdvanceTo = m.autoAdvanceTo.filter((r) => ids.includes(r));
  return m;
}

export function duplicateOf(m: ModeDto, doc: ModesDoc): ModeDto {
  const copy = clone(m);
  delete copy.originalId;
  copy.id = uniqueId(
    `${m.id}-copy`,
    doc.modes.map((x) => x.id)
  );
  if (copy.name) copy.name = `${copy.name} (copy)`;
  return copy;
}

export interface Reference {
  /** The mode that holds the reference, or null for `defaultMode`. */
  from: string | null;
  field: 'defaultMode' | 'next' | 'autoAdvanceTo';
}

export function referencesTo(doc: ModesDoc, id: string): Reference[] {
  const refs: Reference[] = [];
  if (doc.defaultMode === id) refs.push({ from: null, field: 'defaultMode' });
  for (const m of doc.modes) {
    if (m.id === id) continue;
    if (m.next.includes(id)) refs.push({ from: m.id, field: 'next' });
    if (m.autoAdvanceTo.includes(id)) refs.push({ from: m.id, field: 'autoAdvanceTo' });
  }
  return refs;
}

/** Rename a mode and every reference to it (`defaultMode`, `next`, `autoAdvanceTo`). */
export function renameMode(doc: ModesDoc, oldId: string, newId: string): ModesDoc {
  const d = clone(doc);
  if (d.defaultMode === oldId) d.defaultMode = newId;
  for (const m of d.modes) {
    if (m.id === oldId) {
      m.originalId ??= oldId;
      m.id = newId;
    }
    m.next = m.next.map((r) => (r === oldId ? newId : r));
    m.autoAdvanceTo = m.autoAdvanceTo.map((r) => (r === oldId ? newId : r));
  }
  return d;
}

/** Remove a mode and every reference to it; the default falls back to the first mode. */
export function deleteMode(doc: ModesDoc, id: string): ModesDoc {
  const d = clone(doc);
  d.modes = d.modes.filter((m) => m.id !== id);
  for (const m of d.modes) {
    m.next = m.next.filter((r) => r !== id);
    m.autoAdvanceTo = m.autoAdvanceTo.filter((r) => r !== id);
  }
  if (d.defaultMode === id) d.defaultMode = d.modes[0]?.id ?? '';
  return d;
}

export function moveItem<T>(list: T[], from: number, to: number): T[] {
  if (from === to || from < 0 || from >= list.length) return list;
  const t = Math.max(0, Math.min(list.length - 1, to));
  const out = [...list];
  const [x] = out.splice(from, 1);
  out.splice(t, 0, x);
  return out;
}

export function toggle(list: string[], v: string, on: boolean): string[] {
  const has = list.includes(v);
  if (on && !has) return [...list, v];
  if (!on && has) return list.filter((x) => x !== v);
  return list;
}

// ----- validation (mirrors `ruckcode_core::modes_api::validate_doc`; the host is authoritative) -----

export function validateDoc(doc: ModesDoc, catalog?: Catalog | null): Issue[] {
  const out: Issue[] = [];
  const add = (severity: Issue['severity'], path: string, message: string) =>
    out.push({ path, message, severity });
  const d = normalizeDoc(doc);
  const tools = new Set(catalog?.builtinTools.map((t) => t.name));
  const profiles = catalog ? catalog.profiles.map((p) => p.name) : null;
  if (!d.modes.length) add('error', 'modes', 'Define at least one mode.');
  const seen = new Set<string>();
  d.modes.forEach((m, i) => {
    const p = `modes[${i}].id`;
    if (!m.id) add('error', p, 'The id is required.');
    else if (!ID_RE.test(m.id))
      add(
        'error',
        p,
        `\`${m.id}\` is not a valid id: use letters, digits, \`-\` and \`_\` (max 64).`
      );
    else if (seen.has(m.id)) add('error', p, `Duplicate mode id \`${m.id}\`.`);
    seen.add(m.id);
  });
  const ids = new Set(d.modes.map((m) => m.id));
  if (d.modes.length && !ids.has(d.defaultMode))
    add(
      'error',
      'defaultMode',
      d.defaultMode
        ? `The default mode \`${d.defaultMode}\` does not exist.`
        : 'Choose the default mode.'
    );
  d.modes.forEach((m, i) => {
    const p = `modes[${i}]`;
    const dup = new Set<string>();
    (m.tools ?? []).forEach((t, j) => {
      const tp = `${p}.tools[${j}]`;
      if (dup.has(t)) add('warning', tp, `\`${t}\` is listed twice.`);
      else if (!t.trim()) add('error', tp, 'Empty tool name.');
      else if (catalog && !tools.has(t) && !t.startsWith('mise__') && !t.startsWith('mcp__'))
        add('warning', tp, `Unknown tool \`${t}\`; the agent ignores it.`);
      dup.add(t);
    });
    m.miseTasks.forEach((g, j) => {
      const why = globProblem(g);
      if (why) add('error', `${p}.miseTasks[${j}]`, `Invalid glob \`${g}\`: ${why}.`);
    });
    m.mcpTools.forEach((g, j) => {
      const gp = `${p}.mcpTools[${j}]`;
      const why = globProblem(g);
      if (why) add('error', gp, `Invalid glob \`${g}\`: ${why}.`);
      else if (!g.startsWith('mcp__') && !g.startsWith('*'))
        add(
          'warning',
          gp,
          `\`${g}\` matches full MCP tool names, which look like \`mcp__<server>__<tool>\`.`
        );
    });
    for (const [key, list] of [
      ['next', m.next],
      ['autoAdvanceTo', m.autoAdvanceTo]
    ] as const) {
      const seenRef = new Set<string>();
      list.forEach((r, j) => {
        const rp = `${p}.${key}[${j}]`;
        if (seenRef.has(r)) add('warning', rp, `\`${r}\` is listed twice.`);
        else if (!ids.has(r)) add('error', rp, `\`${r}\` is not a defined mode.`);
        else if (r === m.id)
          add('warning', rp, 'A mode cannot switch to itself; this entry is ignored.');
        else if (key === 'autoAdvanceTo' && m.next.length && !m.next.includes(r))
          add('warning', rp, `\`${r}\` is not in \`next\`, so SwitchMode cannot reach it.`);
        seenRef.add(r);
      });
    }
    if (m.profile && profiles && !profiles.includes(m.profile))
      add(
        'warning',
        `${p}.profile`,
        `Profile \`${m.profile}\` is not configured; the session profile is used instead.`
      );
    for (const [pat, val] of Object.entries(m.permissions)) {
      const pp = `${p}.permissions.${pat}`;
      const why = pat.trim() ? globProblem(pat) : 'empty pattern';
      if (why) add('error', pp, `Invalid tool name or glob \`${pat}\`: ${why}.`);
      if (!['allow', 'ask', 'deny'].includes(val))
        add('error', pp, `\`${val}\` is not allow, ask or deny.`);
    }
    for (const ev of HOOK_EVENT_NAMES) {
      (m.hooks[ev] ?? []).forEach((h, j) => {
        for (const [path, severity, message] of hookIssues(ev, h)) {
          add(severity, `${p}.hooks.${ev}[${j}].${path}`, message);
        }
      });
    }
  });
  return out;
}

/** Problems of one hook as [field, severity, message]. */
export function hookIssues(ev: HookEventName, h: HookDto): [string, Issue['severity'], string][] {
  const out: [string, Issue['severity'], string][] = [];
  if (!h.run.trim()) out.push(['run', 'error', 'The command is required.']);
  if (h.glob?.trim()) {
    const why = globProblem(h.glob);
    if (why) out.push(['glob', 'error', `Invalid glob \`${h.glob}\`: ${why}.`]);
    else if (ev !== 'post_edit')
      out.push(['glob', 'warning', 'The glob only applies to post_edit hooks.']);
  }
  if (h.matcher?.trim()) {
    const why = matcherProblem(h.matcher);
    if (why) out.push(['matcher', 'error', `Invalid regex: ${why}`]);
    else if (ev !== 'pre_tool_use' && ev !== 'post_tool_use')
      out.push([
        'matcher',
        'warning',
        'The matcher only applies to pre_tool_use and post_tool_use hooks.'
      ]);
  }
  if (!['ignore', 'block', 'fix_turn'].includes(h.onFailure))
    out.push(['onFailure', 'error', `\`${h.onFailure}\` is not ignore, block or fix_turn.`]);
  if (!(h.timeoutSecs >= 1))
    out.push(['timeoutSecs', 'error', 'The timeout must be at least 1 second.']);
  if (h.onFailure === 'fix_turn' && !(h.maxFixTurns >= 1))
    out.push(['maxFixTurns', 'warning', 'With 0 fix turns a failing hook never triggers a fix.']);
  return out;
}

export function issuesAt(issues: Issue[], path: string): Issue[] {
  return issues.filter(
    (i) => i.path === path || i.path.startsWith(`${path}.`) || i.path.startsWith(`${path}[`)
  );
}

export function mergeIssues(a: Issue[], b: Issue[]): Issue[] {
  const seen = new Set<string>();
  const out: Issue[] = [];
  for (const i of [...a, ...b]) {
    const k = `${i.severity}|${i.path}|${i.message}`;
    if (!seen.has(k)) {
      seen.add(k);
      out.push(i);
    }
  }
  return out;
}

export const countBy = (issues: Issue[], sev: Issue['severity']) =>
  issues.filter((i) => i.severity === sev).length;
