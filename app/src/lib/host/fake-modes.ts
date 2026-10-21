import { createTwoFilesPatch } from 'diff';
import { clone, docsEqual, normalizeDoc, validateDoc } from '../modes/doc';
import {
  DEFAULT_MAX_FIX_TURNS,
  DEFAULT_TIMEOUT_SECS,
  HOOK_EVENT_NAMES,
  type Catalog,
  type HookEventInfo,
  type Issue,
  type ModeDto,
  type ModesApi,
  type ModesDoc,
  type ModesPreview,
  type ModesView,
  type TemplateInfo
} from './modes';
import type { InitInput, InitResult, ModesState, ProfileInfo, SetupTarget } from './setup';

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

function mode(id: string, over: Partial<ModeDto>): ModeDto {
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
    hooks: {},
    ...over
  };
}

const ALL_TOOLS = ['Read', 'Write', 'Edit', 'Glob', 'Grep', 'Bash', 'AskUserQuestion', 'TodoWrite'];

/** The mock project's modes (`rc init --minimal`, enriched). */
export function demoDoc(): ModesDoc {
  return {
    defaultMode: 'plan',
    modes: [
      mode('plan', {
        name: 'Plan',
        description: 'Explore and plan, read only',
        prompt:
          'You are in the plan phase. Read the relevant code, ask the user about anything\nambiguous with AskUserQuestion, and write a short plan under plans/.\nWhen the plan is agreed, use SwitchMode to propose moving to build.',
        tools: ['Read', 'Glob', 'Grep', 'Write', 'AskUserQuestion', 'TodoWrite', 'SwitchMode'],
        miseTasks: ['docs*'],
        next: ['build']
      }),
      mode('build', {
        name: 'Build',
        description: 'Implement changes',
        prompt:
          'You are in the build phase. Implement the plan with minimal, focused changes.\nWhen the work is complete, use SwitchMode to propose moving to verify.',
        tools: ALL_TOOLS.concat('SwitchMode'),
        miseTasks: ['build', 'lint*'],
        mcpTools: ['mcp__github__*'],
        next: ['verify'],
        permissions: { Write: 'allow', Edit: 'allow', Bash: 'ask' },
        hooks: {
          post_edit: [
            {
              run: 'mise run fmt',
              glob: '*.rs',
              matcher: null,
              onFailure: 'fix_turn',
              maxFixTurns: DEFAULT_MAX_FIX_TURNS,
              timeoutSecs: DEFAULT_TIMEOUT_SECS
            }
          ]
        }
      }),
      mode('verify', {
        description: 'Run tests and checks',
        prompt: 'You are in the verify phase. Run the tests and linters and fix what they find.',
        tools: ALL_TOOLS.concat('SwitchMode'),
        miseTasks: ['test*', 'lint*'],
        model: 'mock-small',
        next: ['build'],
        autoAdvanceTo: ['build'],
        permissions: { Bash: 'ask' }
      })
    ]
  };
}

const EVENTS: HookEventInfo[] = [
  ['session_start', false, false, 'When a session starts or resumes.'],
  ['user_prompt_submit', false, false, 'Before a prompt is sent to the model.'],
  ['pre_tool_use', false, true, 'Before a tool runs; a failing hook can block the call.'],
  ['post_tool_use', false, true, 'After a tool ran.'],
  ['post_edit', true, false, 'After a file edit; {files} expands to the edited files.'],
  ['mode_change', false, false, 'After the mode changed.'],
  ['stop', false, false, 'When the agent finishes a turn.']
].map(([name, glob, matcher, description]) => ({
  name: name as HookEventInfo['name'],
  glob: glob as boolean,
  matcher: matcher as boolean,
  description: description as string
}));

const TOOLS: Catalog['builtinTools'] = [
  { name: 'Read', kind: 'read', readOnly: true },
  { name: 'Write', kind: 'edit', readOnly: false },
  { name: 'Edit', kind: 'edit', readOnly: false },
  { name: 'Glob', kind: 'search', readOnly: true },
  { name: 'Grep', kind: 'search', readOnly: true },
  { name: 'Bash', kind: 'execute', readOnly: false },
  { name: 'TodoWrite', kind: 'think', readOnly: true },
  { name: 'AskUserQuestion', kind: 'other', readOnly: true },
  { name: 'SwitchMode', kind: 'switch_mode', readOnly: true }
];

/** The six lifecycle templates and the three minimal ones (shortened prompts). */
function templates(): TemplateInfo[] {
  const t = (set: TemplateInfo['set'], id: string, over: Partial<ModeDto>): TemplateInfo => ({
    set,
    mode: mode(id, { name: id.charAt(0).toUpperCase() + id.slice(1), ...over })
  });
  const rw = { Write: 'allow', Edit: 'allow', Bash: 'ask' } as const;
  return [
    t('full', 'explore', {
      description: 'Understand the codebase and the problem; read-only',
      prompt: 'You are in the explore phase. Read code and answer with file references.',
      tools: ['Read', 'Glob', 'Grep', 'Bash', 'AskUserQuestion', 'TodoWrite', 'SwitchMode'],
      next: ['design'],
      permissions: { Bash: 'ask' }
    }),
    t('full', 'design', {
      description: 'Turn findings into a written plan; writes only under plans/',
      prompt: 'You are in the design phase. Produce a concrete plan under plans/.',
      tools: ['Read', 'Glob', 'Grep', 'Write', 'AskUserQuestion', 'TodoWrite', 'SwitchMode'],
      next: ['implement']
    }),
    t('full', 'implement', {
      description: 'Write the code for the approved plan',
      prompt: 'You are in the implement phase. Follow the approved plan.',
      tools: ALL_TOOLS.concat('SwitchMode'),
      next: ['test'],
      permissions: { ...rw }
    }),
    t('full', 'test', {
      description: 'Write and run tests; fix failures',
      prompt: 'You are in the test phase. Add tests and fix failures at their cause.',
      tools: ALL_TOOLS.concat('SwitchMode'),
      next: ['implement', 'review'],
      permissions: { ...rw }
    }),
    t('full', 'review', {
      description: 'Critically review the changes; read-only, reports findings',
      prompt: 'You are in the review phase. Review the pending changes.',
      tools: ['Read', 'Glob', 'Grep', 'Bash', 'AskUserQuestion', 'TodoWrite', 'SwitchMode'],
      next: ['implement', 'release'],
      permissions: { Bash: 'ask' }
    }),
    t('full', 'release', {
      description: 'Prepare changelog, version bump and pull request',
      prompt: 'You are in the release phase. Prepare the change for shipping.',
      tools: ALL_TOOLS.concat('SwitchMode'),
      next: ['explore'],
      permissions: { Write: 'ask', Edit: 'ask', Bash: 'ask' }
    }),
    t('minimal', 'plan', {
      description: 'Understand the task and write a plan; does not change source',
      tools: ['Read', 'Glob', 'Grep', 'Write', 'AskUserQuestion', 'TodoWrite', 'SwitchMode'],
      next: ['build']
    }),
    t('minimal', 'build', {
      description: 'Implement the plan',
      tools: ALL_TOOLS.concat('SwitchMode'),
      next: ['verify'],
      permissions: { ...rw }
    }),
    t('minimal', 'verify', {
      description: 'Run tests and linters and fix what they find',
      tools: ALL_TOOLS.concat('SwitchMode'),
      next: ['build'],
      permissions: { ...rw }
    })
  ];
}

const q = (s: string) => JSON.stringify(s);
const arr = (a: string[]) => `[${a.map(q).join(', ')}]`;

/** A canonical modes.toml for a document (what the mock "writes"; comments are simulated). */
export function renderToml(doc: ModesDoc): string {
  const d = normalizeDoc(doc);
  const out: string[] = [
    '# Generated by `rc init`. Edit freely; run `rc init --diff` to compare with the current templates.',
    '',
    `default_mode = ${q(d.defaultMode)}`
  ];
  for (const m of d.modes) {
    out.push('', `[modes.${m.id}]`);
    if (m.name) out.push(`name = ${q(m.name)}`);
    if (m.description) out.push(`description = ${q(m.description)}`);
    if (m.prompt)
      out.push(
        m.prompt.includes('\n') ? `prompt = """\n${m.prompt}"""` : `prompt = ${q(m.prompt)}`
      );
    if (m.tools) out.push(`tools = ${arr(m.tools)}`);
    if (m.miseTasks.length) out.push(`mise_tasks = ${arr(m.miseTasks)}`);
    if (m.mcpTools.length) out.push(`mcp_tools = ${arr(m.mcpTools)}`);
    if (m.next.length) out.push(`next = ${arr(m.next)}`);
    if (m.autoAdvanceTo.length) out.push(`auto_advance_to = ${arr(m.autoAdvanceTo)}`);
    if (m.profile) out.push(`profile = ${q(m.profile)}`);
    if (m.model) out.push(`model = ${q(m.model)}`);
    const perms = Object.entries(m.permissions);
    if (perms.length) {
      out.push('', `[modes.${m.id}.permissions]`);
      for (const [k, v] of perms) out.push(`${/^[\w-]+$/.test(k) ? k : q(k)} = ${q(v)}`);
    }
    for (const ev of HOOK_EVENT_NAMES) {
      for (const h of m.hooks[ev] ?? []) {
        out.push('', `[[modes.${m.id}.hooks.${ev}]]`, `run = ${q(h.run)}`);
        if (h.glob) out.push(`glob = ${q(h.glob)}`);
        if (h.matcher) out.push(`matcher = ${q(h.matcher)}`);
        if (h.onFailure !== 'ignore') out.push(`on_failure = ${q(h.onFailure)}`);
        if (h.maxFixTurns !== DEFAULT_MAX_FIX_TURNS) out.push(`max_fix_turns = ${h.maxFixTurns}`);
        if (h.timeoutSecs !== DEFAULT_TIMEOUT_SECS) out.push(`timeout_secs = ${h.timeoutSecs}`);
      }
    }
  }
  return out.join('\n') + '\n';
}

export interface FakeModesOptions {
  /** Start without a modes.toml (first run). */
  fresh?: boolean;
  /** Start with an unreadable modes.toml. */
  broken?: boolean;
  delayMs?: number;
  /** The configured profiles (shared with the fake setup). */
  profiles?: () => ProfileInfo[];
}

/**
 * In-memory mode editor for browser mock mode. Documents are kept per project and "written" as a
 * canonical TOML text only to produce plausible diffs; the real backend keeps comments and
 * formatting of everything that did not change.
 */
export class FakeModes implements ModesApi {
  private docs = new Map<string, ModesDoc | null>();
  private broken: boolean;
  private fresh: boolean;
  private delayMs: number;
  private profiles: () => ProfileInfo[];
  /** Every call, for tests. */
  readonly calls: string[] = [];
  /** Last documents received by `preview` and `apply`, for tests. */
  lastPreview: ModesDoc | null = null;
  lastApplied: ModesDoc | null = null;

  constructor(opts: FakeModesOptions = {}) {
    this.fresh = !!opts.fresh;
    this.broken = !!opts.broken;
    this.delayMs = opts.delayMs ?? 40;
    this.profiles =
      opts.profiles ??
      (() => [
        {
          name: 'mock',
          provider: 'openai',
          model: 'mock-large',
          models: ['mock-large', 'mock-small'],
          baseUrl: null,
          apiKeyEnv: null,
          apiKeyName: null
        }
      ]);
  }

  private docOf(project: string): ModesDoc | null {
    if (!this.docs.has(project)) this.docs.set(project, this.fresh ? null : demoDoc());
    return this.docs.get(project) ?? null;
  }

  /** The summary the setup wizard and Project tab show. */
  summary(project: string): ModesState {
    if (this.broken) return { status: 'broken', error: 'TOML parse error at line 3' };
    const d = this.docOf(project);
    if (!d) return { status: 'missing' };
    return {
      status: 'ready',
      defaultMode: d.defaultMode,
      modes: d.modes.map((m) => ({
        id: m.id,
        description: m.description,
        profile: m.profile,
        model: m.model
      }))
    };
  }

  /** `rc setup`'s per-mode model choice. */
  setModeModel(project: string, id: string, profile: string | null, model: string | null) {
    const m = this.docOf(project)?.modes.find((x) => x.id === id);
    if (!m) throw new Error(`mode \`${id}\` is not defined in modes.toml`);
    m.profile = model ? profile : null;
    m.model = model;
  }

  private catalog(): Catalog {
    const profiles = this.profiles();
    return {
      builtinTools: clone(TOOLS),
      miseTasks: [
        ['build', 'Build everything'],
        ['test', 'Run all tests'],
        ['test:unit', 'Unit tests only'],
        ['lint', 'Run the linters'],
        ['fmt', 'Format the code'],
        ['docs', 'Build the documentation'],
        ['release', 'Cut a release']
      ].map(([name, description]) => ({ name, description })),
      mcpServers: ['github', 'filesystem'],
      profiles: profiles.map((p) => ({ name: p.name, models: [...p.models] })),
      defaultProfile: profiles[0]?.name ?? null,
      hookEvents: clone(EVENTS),
      permissionValues: ['allow', 'ask', 'deny'],
      onFailureValues: ['ignore', 'block', 'fix_turn'],
      templates: templates()
    };
  }

  /** Lets e2e tests see which project and distro each call was for. */
  private record(op: string, t: SetupTarget) {
    this.calls.push(op);
    const w = globalThis as unknown as { __ruckcodeModesCalls?: unknown[] };
    (w.__ruckcodeModesCalls ??= []).push({ op, project: t.project, distro: t.distro ?? null });
  }

  async get(t: SetupTarget): Promise<ModesView> {
    this.record('get', t);
    await wait(this.delayMs);
    const base = { path: `${t.project}/.ruckcode/modes.toml`, catalog: this.catalog() };
    if (this.broken)
      return {
        ...base,
        status: 'broken',
        error: 'parsing modes.toml: TOML parse error at line 3, column 1',
        modes: []
      };
    const d = this.docOf(t.project);
    if (!d) return { ...base, status: 'missing', modes: [] };
    return {
      ...base,
      status: 'ready',
      defaultMode: d.defaultMode,
      modes: d.modes.map((m) => ({ ...clone(m), originalId: m.id })),
      globalHooks: {
        session_start: [
          {
            run: 'echo session started',
            glob: null,
            matcher: null,
            onFailure: 'ignore',
            maxFixTurns: DEFAULT_MAX_FIX_TURNS,
            timeoutSecs: DEFAULT_TIMEOUT_SECS
          }
        ]
      }
    };
  }

  async validate(t: SetupTarget, doc: ModesDoc): Promise<Issue[]> {
    this.record('validate', t);
    await wait(5);
    return validateDoc(doc, this.catalog());
  }

  private check(doc: ModesDoc) {
    const errors = validateDoc(doc, this.catalog()).filter((i) => i.severity === 'error');
    if (errors.length)
      throw new Error(
        `modes.toml would be invalid: ${errors.map((e) => `${e.path}: ${e.message}`).join('; ')}`
      );
  }

  async preview(t: SetupTarget, doc: ModesDoc): Promise<ModesPreview> {
    this.record('preview', t);
    this.check(doc);
    await wait(this.delayMs / 2);
    this.lastPreview = clone(doc);
    const old = this.docOf(t.project);
    const before = old ? renderToml(old) : '';
    const after = renderToml(doc);
    const changed = before !== after;
    return {
      diff: changed
        ? createTwoFilesPatch('current modes.toml', 'new modes.toml', before, after, '', '', {
            context: 3
          }).replace(/^=+\n/, '')
        : '',
      changed,
      rewritten: []
    };
  }

  async apply(t: SetupTarget, doc: ModesDoc) {
    this.record('apply', t);
    this.check(doc);
    await wait(this.delayMs);
    const old = this.docOf(t.project);
    const changed = !old || !docsEqual(old, doc);
    const saved = clone(normalizeDoc(doc));
    for (const m of saved.modes) delete m.originalId;
    this.docs.set(t.project, saved);
    this.broken = false;
    this.lastApplied = clone(doc);
    return { path: `${t.project}/.ruckcode/modes.toml`, changed };
  }

  async initModes(t: SetupTarget, opts: InitInput = {}): Promise<InitResult> {
    this.record('initModes', t);
    await wait(60);
    const cur = this.docOf(t.project);
    if ((cur || this.broken) && !opts.force)
      throw new Error(
        `${t.project}/.ruckcode/modes.toml already exists; use force to overwrite it`
      );
    // A subset picks from the six lifecycle templates, `minimal` is plan/build/verify, and the
    // default is the mock project's own modes.
    const all = templates();
    let mods: ModeDto[];
    if (opts.modes?.length)
      mods = all
        .filter((t) => t.set === 'full' && opts.modes!.includes(t.mode.id))
        .map((t) => t.mode);
    else if (opts.minimal) mods = all.filter((t) => t.set === 'minimal').map((t) => t.mode);
    else mods = demoDoc().modes.map((m) => ({ ...m, hooks: {} }));
    if (!mods.length) throw new Error('no modes selected');
    let d: ModesDoc = { defaultMode: mods[0].id, modes: mods };
    d = {
      defaultMode: d.modes[0].id,
      modes: d.modes.map((m) => ({
        ...m,
        next: m.next.filter((n) => d.modes.some((x) => x.id === n)),
        autoAdvanceTo: m.autoAdvanceTo.filter((n) => d.modes.some((x) => x.id === n))
      }))
    };
    this.docs.set(t.project, d);
    this.broken = false;
    return {
      path: `${t.project}/.ruckcode/modes.toml`,
      tasksDiscovered: 3,
      modes: this.summary(t.project)
    };
  }
}
