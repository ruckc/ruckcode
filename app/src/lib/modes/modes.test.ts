import { describe, expect, it } from 'vitest';
import { demoDoc, FakeModes, renderToml } from '../host/fake-modes';
import type { Catalog, HookDto, ModesDoc } from '../host/modes';
import {
  clone,
  deleteMode,
  docsEqual,
  duplicateOf,
  emptyMode,
  issuesAt,
  modeFromTemplate,
  moveItem,
  newHook,
  normalizeDoc,
  referencesTo,
  renameMode,
  uniqueId,
  validateDoc
} from './doc';
import { layers, layout, transitions } from './flow';
import { globMatches, globProblem, globToRegExp, matcherProblem } from './glob';

const target = { project: '/demo/project' };
async function catalog(): Promise<Catalog> {
  return (await new FakeModes({ delayMs: 0 }).get(target)).catalog;
}

describe('document operations', () => {
  it('renames a mode and every reference, remembering the on-disk id', () => {
    const d = demoDoc();
    d.modes[1].originalId = 'build';
    const r = renameMode(d, 'build', 'implement');
    expect(r.modes[1].id).toBe('implement');
    expect(r.modes[1].originalId).toBe('build');
    expect(r.modes[0].next).toEqual(['implement']);
    expect(r.modes[2].next).toEqual(['implement']);
    expect(r.modes[2].autoAdvanceTo).toEqual(['implement']);
    // the default follows
    expect(renameMode(d, 'plan', 'scout').defaultMode).toBe('scout');
    // the input is not mutated
    expect(d.modes[1].id).toBe('build');
    // renaming twice keeps the first original id
    expect(renameMode(r, 'implement', 'ship').modes[1].originalId).toBe('build');
  });

  it('finds references and removes them with the mode', () => {
    const d = demoDoc();
    expect(referencesTo(d, 'build')).toEqual([
      { from: 'plan', field: 'next' },
      { from: 'verify', field: 'next' },
      { from: 'verify', field: 'autoAdvanceTo' }
    ]);
    expect(referencesTo(d, 'plan')).toEqual([{ from: null, field: 'defaultMode' }]);
    const r = deleteMode(d, 'build');
    expect(r.modes.map((m) => m.id)).toEqual(['plan', 'verify']);
    expect(r.modes[0].next).toEqual([]);
    expect(r.modes[1].autoAdvanceTo).toEqual([]);
    expect(deleteMode(d, 'plan').defaultMode).toBe('build');
  });

  it('reorders, makes unique ids and builds modes from templates', async () => {
    expect(moveItem([1, 2, 3, 4], 3, 0)).toEqual([4, 1, 2, 3]);
    expect(moveItem([1, 2, 3], 0, 9)).toEqual([2, 3, 1]);
    expect(moveItem([1, 2, 3], 5, 0)).toEqual([1, 2, 3]);
    expect(uniqueId('plan', ['plan', 'plan-2'])).toBe('plan-3');
    expect(uniqueId('a b!', [])).toBe('a-b-');
    const cat = await catalog();
    const doc = demoDoc();
    const review = cat.templates.find((t) => t.set === 'full' && t.mode.id === 'review')!;
    const m = modeFromTemplate(review, doc);
    expect(m.id).toBe('review');
    expect(m.originalId).toBeUndefined();
    expect(m.next).toEqual([]); // implement and release do not exist here
    const plan = cat.templates.find((t) => t.set === 'minimal' && t.mode.id === 'plan')!;
    expect(modeFromTemplate(plan, doc).id).toBe('plan-2');
    const dup = duplicateOf(doc.modes[2], doc);
    expect(dup.id).toBe('verify-copy');
    expect(dup.name).toBeNull();
    expect(dup.originalId).toBeUndefined();
    expect(duplicateOf(doc.modes[1], doc).name).toBe('Build (copy)');
  });

  it('normalizes blanks so formatting-only edits are not changes', () => {
    const a = demoDoc();
    const b = clone(a);
    b.modes[0].name = '  ';
    b.modes[1].hooks.stop = [];
    b.modes[1].hooks.post_edit![0].glob = '';
    expect(docsEqual(a, b)).toBe(false); // the plan name changed from "Plan" to blank
    b.modes[0].name = a.modes[0].name;
    b.modes[1].hooks.post_edit![0].glob = a.modes[1].hooks.post_edit![0].glob;
    expect(docsEqual(a, b)).toBe(true);
    expect(normalizeDoc(b).modes[1].hooks.stop).toBeUndefined();
  });
});

describe('validation', () => {
  const hook = (over: Partial<HookDto>): HookDto => ({ ...newHook(), run: 'x', ...over });

  it('maps problems to paths with severities', async () => {
    const cat = await catalog();
    const d: ModesDoc = {
      defaultMode: 'ghost',
      modes: [
        {
          ...emptyMode('a'),
          tools: ['Read', 'Nope'],
          miseTasks: ['['],
          mcpTools: ['github__*'],
          next: ['zzz', 'a'],
          autoAdvanceTo: ['b'],
          profile: 'nope',
          permissions: { Bash: 'maybe' as never },
          hooks: {
            post_edit: [hook({ run: ' ', glob: '[', matcher: '(', onFailure: 'boom' as never })],
            stop: [hook({ glob: '*.rs', matcher: 'x' })]
          }
        },
        { ...emptyMode('a') },
        { ...emptyMode('bad id') },
        emptyMode('b')
      ]
    };
    const issues = validateDoc(d, cat);
    const has = (path: string, severity: string) =>
      issues.some((i) => i.path === path && i.severity === severity);
    expect(has('defaultMode', 'error')).toBe(true);
    expect(has('modes[1].id', 'error')).toBe(true);
    expect(has('modes[2].id', 'error')).toBe(true);
    expect(has('modes[0].tools[1]', 'warning')).toBe(true);
    expect(has('modes[0].miseTasks[0]', 'error')).toBe(true);
    expect(has('modes[0].mcpTools[0]', 'warning')).toBe(true);
    expect(has('modes[0].next[0]', 'error')).toBe(true);
    expect(has('modes[0].next[1]', 'warning')).toBe(true);
    expect(has('modes[0].autoAdvanceTo[0]', 'warning')).toBe(true);
    expect(has('modes[0].profile', 'warning')).toBe(true);
    expect(has('modes[0].permissions.Bash', 'error')).toBe(true);
    for (const f of ['run', 'glob', 'matcher', 'onFailure'])
      expect(has(`modes[0].hooks.post_edit[0].${f}`, 'error')).toBe(true);
    expect(has('modes[0].hooks.stop[0].glob', 'warning')).toBe(true);
    expect(has('modes[0].hooks.stop[0].matcher', 'warning')).toBe(true);
    expect(validateDoc({ defaultMode: '', modes: [] }).map((i) => i.path)).toEqual(['modes']);
    // the demo project is clean
    expect(validateDoc(demoDoc(), cat).filter((i) => i.severity !== 'info')).toEqual([]);
  });

  it('scopes issues by path prefix', () => {
    const issues = [
      { path: 'modes[1].tools[0]', message: 'a', severity: 'warning' as const },
      { path: 'modes[1].tools', message: 'b', severity: 'error' as const },
      { path: 'modes[10].tools', message: 'c', severity: 'error' as const }
    ];
    expect(issuesAt(issues, 'modes[1]').map((i) => i.message)).toEqual(['a', 'b']);
    expect(issuesAt(issues, 'modes[1].tools').length).toBe(2);
  });
});

describe('globs and regexes', () => {
  it('validates and matches', () => {
    expect(globProblem('test*')).toBeNull();
    expect(globProblem('[')).toMatch(/unclosed/);
    expect(globProblem('{a,b')).toMatch(/unclosed/);
    expect(globProblem('a**b')).toMatch(/component/);
    expect(globProblem('')).toBe('empty pattern');
    const tasks = ['test', 'test:unit', 'lint', 'build'];
    expect(globMatches('test*', tasks)).toEqual(['test', 'test:unit']);
    expect(globMatches('{lint,build}', tasks)).toEqual(['lint', 'build']);
    expect(globMatches('t?st', tasks)).toEqual(['test']);
    expect(globMatches('[lb]*', tasks)).toEqual(['lint', 'build']);
    expect(globMatches('[', tasks)).toBeNull();
    expect(globToRegExp('mcp__*')!.test('mcp__github__list')).toBe(true);
    expect(globToRegExp('a.b')!.test('axb')).toBe(false);
    expect(matcherProblem('Bash|Edit')).toBeNull();
    expect(matcherProblem('(')).toMatch(/group|Unterminated/i);
    expect(matcherProblem('(?=x)')).toMatch(/lookaround/);
  });
});

describe('flow layout', () => {
  it('puts modes in columns by distance from the default and styles auto edges', () => {
    const d = demoDoc();
    expect(layers(d.modes, 'plan')).toEqual([['plan'], ['build'], ['verify']]);
    const t = transitions(d.modes);
    expect(t).toContainEqual({ from: 'verify', to: 'build', auto: true }); // auto wins over next
    expect(t.filter((e) => e.from === 'verify' && e.to === 'build').length).toBe(1);
    const f = layout(d.modes, 'plan');
    expect(f.nodes.map((n) => [n.id, n.col])).toEqual([
      ['plan', 0],
      ['build', 1],
      ['verify', 2]
    ]);
    expect(f.nodes[0].isDefault).toBe(true);
    expect(f.edges.find((e) => e.from === 'verify')?.auto).toBe(true);
    expect(f.edges.find((e) => e.from === 'plan')?.auto).toBe(false);
    expect(f.width).toBeGreaterThan(400);
  });

  it('handles unreachable modes, empty next and bad targets', () => {
    const modes = [
      { ...emptyMode('a'), next: ['ghost', 'a'] },
      { ...emptyMode('b') },
      { ...emptyMode('c'), next: ['b'] }
    ];
    expect(transitions(modes)).toEqual([{ from: 'c', to: 'b', auto: false }]);
    const f = layout(modes, 'a');
    expect(f.nodes.find((n) => n.id === 'b')!.anyTarget).toBe(true);
    expect(f.nodes.find((n) => n.id === 'c')!.anyTarget).toBe(false);
    expect(new Set(f.nodes.map((n) => n.id)).size).toBe(3);
    expect(layout([], '').nodes).toEqual([]);
  });
});

describe('the mock file text', () => {
  it('renders defaults away and hooks as arrays of tables', () => {
    const t = renderToml(demoDoc());
    expect(t).toContain('[modes.build.permissions]');
    expect(t).toContain('[[modes.build.hooks.post_edit]]\nrun = "mise run fmt"');
    expect(t).toContain('on_failure = "fix_turn"');
    expect(t).not.toContain('max_fix_turns');
    expect(t).toContain('prompt = """\n');
  });
});
