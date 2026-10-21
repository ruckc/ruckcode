import { FakeModes } from './fake-modes';
import type {
  InitInput,
  InitResult,
  KeysList,
  KeyView,
  ModelsAuth,
  ModelView,
  ModesState,
  PlanInput,
  PresetInfo,
  ProfileInfo,
  ReportView,
  SetupApi,
  SetupState,
  SetupTarget
} from './setup';

const PRESETS: PresetInfo[] = [
  {
    id: 'anthropic',
    label: 'Anthropic',
    kind: 'anthropic',
    baseUrl: 'https://api.anthropic.com',
    apiKeyEnv: 'ANTHROPIC_API_KEY'
  },
  {
    id: 'openai',
    label: 'OpenAI',
    kind: 'openai',
    baseUrl: 'https://api.openai.com/v1',
    apiKeyEnv: 'OPENAI_API_KEY'
  },
  {
    id: 'ollama',
    label: 'Ollama (local)',
    kind: 'openai',
    baseUrl: 'http://localhost:11434/v1',
    apiKeyEnv: null
  },
  {
    id: 'openrouter',
    label: 'OpenRouter',
    kind: 'openai',
    baseUrl: 'https://openrouter.ai/api/v1',
    apiKeyEnv: 'OPENROUTER_API_KEY'
  },
  {
    id: 'custom',
    label: 'vLLM / LM Studio / other OpenAI-compatible',
    kind: 'openai',
    baseUrl: null,
    apiKeyEnv: null
  }
];

const ANTHROPIC = ['claude-opus-5-5', 'claude-sonnet-5-5', 'claude-haiku-5-5'];
const OLLAMA = ['llama3.1:8b', 'mistral:7b', 'qwen2.5-coder:7b'];
const OPENROUTER = [
  'anthropic/claude-sonnet-5-5',
  'google/gemini-3-pro',
  'meta-llama/llama-4-70b',
  'openai/gpt-5',
  'qwen/qwen3-coder'
];
const OPENAI = ['gpt-5', 'gpt-5-mini', 'o4-mini'];

export interface FakeSetupOptions {
  /** Start with no usable profile and no modes.toml (first run). */
  fresh?: boolean;
  delayMs?: number;
  /** The mode editor's store, shared so both views see the same modes.toml. */
  modes?: FakeModes;
}

interface Ref {
  key: string;
  profile: string;
  scope: 'user' | 'project';
}

const mask = (s: string) => (s.length <= 8 ? '…' : `…${s.slice(-4)}`);
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * In-memory setup/keys for browser mock mode. Model lists: Anthropic, OpenAI, Ollama and OpenRouter
 * endpoints answer; a base URL containing `broken` (or `fail`) answers HTTP 401, an unset env var
 * or the secret `bad` fails too, and any other URL lists two generic models.
 */
export class FakeSetup implements SetupApi {
  profiles: ProfileInfo[];
  defaultProfile: string;
  keys = new Map<string, { description: string | null; createdAt: string; secret: string }>();
  refs: Ref[] = [];
  env: Record<string, boolean> = {
    ANTHROPIC_API_KEY: false,
    OPENAI_API_KEY: false,
    OPENROUTER_API_KEY: true
  };
  /** modes.toml per project, shared with the mode editor. */
  readonly modes: FakeModes;
  private fresh: boolean;
  private delayMs: number;
  /** Every call, for tests (never contains secrets). */
  readonly calls: string[] = [];
  /** Last plan passed to apply (secret included, in memory only, for e2e assertions via window). */
  lastApplied: { profileName: string; hadNewKey: boolean } | null = null;

  constructor(opts: FakeSetupOptions = {}) {
    this.fresh = !!opts.fresh;
    this.delayMs = opts.delayMs ?? 150;
    this.modes =
      opts.modes ??
      new FakeModes({ fresh: this.fresh, delayMs: 20, profiles: () => this.profiles });
    if (this.fresh) {
      this.profiles = [];
      this.defaultProfile = 'anthropic';
    } else {
      this.profiles = [
        {
          name: 'mock',
          provider: 'openai',
          model: 'mock-large',
          models: ['mock-large', 'mock-small'],
          baseUrl: null,
          apiKeyEnv: null,
          apiKeyName: null
        }
      ];
      this.defaultProfile = 'mock';
    }
  }

  async presets() {
    return PRESETS;
  }

  private modesOf(project: string): ModesState {
    return this.modes.summary(project);
  }

  private buildKeys(project: string): KeysList {
    void project;
    const keys: KeyView[] = [...this.keys.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, k]) => ({
        name,
        description: k.description,
        createdAt: k.createdAt,
        masked: mask(k.secret),
        usedBy: this.refs
          .filter((r) => r.key === name)
          .map((r) => `profile \`${r.profile}\` (${r.scope === 'user' ? 'user' : 'this project'})`)
      }));
    const missing = this.refs
      .filter((r) => !this.keys.has(r.key))
      .map((r) => ({
        name: r.key,
        usedBy: `profile \`${r.profile}\` (${r.scope === 'user' ? 'user' : 'this project'})`
      }));
    return { keys, missing };
  }

  async state(t: SetupTarget): Promise<SetupState> {
    this.calls.push('state');
    await wait(5);
    const envNames = new Set([
      ...Object.keys(this.env),
      ...this.profiles.flatMap((p) => p.apiKeyEnv ?? [])
    ]);
    return {
      needsOnboarding:
        this.profiles.length === 0 && !this.env.ANTHROPIC_API_KEY && !this.env.OPENAI_API_KEY,
      project: t.project,
      defaultProfile: this.defaultProfile,
      profiles: structuredClone(this.profiles),
      keys: this.buildKeys(t.project),
      env: Object.fromEntries([...envNames].map((n) => [n, !!this.env[n]])),
      modes: structuredClone(this.modesOf(t.project))
    };
  }

  async listModels(
    _t: SetupTarget,
    req: { kind: string; baseUrl: string; auth?: ModelsAuth }
  ): Promise<ModelView[]> {
    this.calls.push(`models:${req.kind}:${req.baseUrl}`);
    await wait(this.delayMs);
    const { auth } = req;
    if (auth && 'env' in auth && !this.env[auth.env]) {
      throw new Error(`environment variable ${auth.env} is not set`);
    }
    if (auth && 'keyName' in auth && !this.keys.has(auth.keyName)) {
      throw new Error(`named key \`${auth.keyName}\` not found`);
    }
    if (/broken|fail/.test(req.baseUrl) || (auth && 'secret' in auth && auth.secret === 'bad')) {
      throw new Error('HTTP 401 Unauthorized: invalid api key');
    }
    const url = req.baseUrl;
    const ids = url.includes('anthropic.com')
      ? ANTHROPIC
      : url.includes('openrouter')
        ? OPENROUTER
        : url.includes('11434')
          ? OLLAMA
          : url.includes('openai.com')
            ? OPENAI
            : ['local-model-a', 'local-model-b'];
    return ids.map((id) => ({ id, displayName: null }));
  }

  private diff(plan: PlanInput, project: string): string {
    const cfg = '~/.config/ruckcode/config.toml';
    const lines = [
      `# ${cfg}`,
      '--- a/config.toml',
      '+++ b/config.toml',
      '@@ -0,0 +1,10 @@',
      `+[profiles.${plan.profileName}]`,
      `+provider = "${plan.provider}"`,
      `+model = "${plan.model}"`
    ];
    if (plan.baseUrl) lines.push(`+base_url = "${plan.baseUrl}"`);
    if (plan.apiKeyEnv) lines.push(`+api_key_env = "${plan.apiKeyEnv}"`);
    if (plan.apiKeyName && plan.keyScope === 'user')
      lines.push(`+api_key_name = "${plan.apiKeyName}"`);
    lines.push(`+models = [${plan.models.map((m) => `"${m}"`).join(', ')}]`);
    if (plan.setDefault || this.profiles.length === 0)
      lines.push(`+default_profile = "${plan.profileName}"`);
    if (plan.newKey) {
      lines.push(
        '',
        `key \`${plan.newKey.name}\` will be saved to ~/.config/ruckcode/credentials.toml (mode 0600)`
      );
    }
    if (plan.apiKeyName && plan.keyScope === 'project') {
      lines.push(
        '',
        `# ${project}/.ruckcode/config.toml (this project only)`,
        '--- a/config.toml',
        '+++ b/config.toml',
        '@@ -0,0 +1,2 @@',
        `+[profiles.${plan.profileName}]`,
        `+api_key_name = "${plan.apiKeyName}"`
      );
    }
    if (plan.modeModels.length) {
      lines.push('', `# ${project}/.ruckcode/modes.toml`, '--- a/modes.toml', '+++ b/modes.toml');
      for (const m of plan.modeModels) {
        lines.push(`@@ [modes.${m.mode}] @@`);
        if (m.model) lines.push(`+profile = "${m.profile}"`, `+model = "${m.model}"`);
        else lines.push('-profile/model removed (uses the profile default)');
      }
    }
    return lines.join('\n') + '\n';
  }

  async preview(t: SetupTarget, plan: PlanInput) {
    this.calls.push('preview');
    this.validate(plan);
    await wait(10);
    return { diff: this.diff(plan, t.project) };
  }

  private validate(plan: PlanInput) {
    if (!/^[A-Za-z0-9._-]+$/.test(plan.profileName)) throw new Error('invalid profile name');
    if (!plan.model) throw new Error('a default model is required');
    if (plan.apiKeyName && plan.apiKeyEnv) throw new Error('choose either apiKeyName or apiKeyEnv');
  }

  async apply(t: SetupTarget, plan: PlanInput): Promise<ReportView> {
    this.calls.push('apply');
    this.validate(plan);
    await wait(30);
    const modes = this.modesOf(t.project);
    if (plan.modeModels.some((m) => m.model) && modes.status !== 'ready')
      throw new Error('.ruckcode/modes.toml does not exist; run `rc init` first');
    let credentialsPath: string | null = null;
    if (plan.newKey) {
      this.keys.set(plan.newKey.name, {
        description: plan.newKey.description || null,
        createdAt: new Date().toISOString().replace(/\.\d+Z$/, 'Z'),
        secret: plan.newKey.secret.trim()
      });
      credentialsPath = '~/.config/ruckcode/credentials.toml';
    }
    const apiKeyName = plan.newKey?.name ?? plan.apiKeyName;
    this.refs = this.refs.filter((r) => r.profile !== plan.profileName);
    if (apiKeyName)
      this.refs.push({ key: apiKeyName, profile: plan.profileName, scope: plan.keyScope });
    const info: ProfileInfo = {
      name: plan.profileName,
      provider: plan.provider,
      model: plan.model,
      models: plan.models,
      baseUrl: plan.baseUrl,
      apiKeyEnv: plan.apiKeyEnv,
      apiKeyName: apiKeyName ?? null
    };
    this.profiles = [...this.profiles.filter((p) => p.name !== info.name), info];
    if (plan.setDefault || this.profiles.length === 1) this.defaultProfile = info.name;
    let modesUpdated = 0;
    if (modes.status === 'ready') {
      for (const mm of plan.modeModels) {
        this.modes.setModeModel(t.project, mm.mode, mm.model ? mm.profile : null, mm.model);
        modesUpdated++;
      }
    }
    this.lastApplied = { profileName: plan.profileName, hadNewKey: !!plan.newKey };
    return {
      configPath: '~/.config/ruckcode/config.toml',
      projectConfigPath:
        plan.keyScope === 'project' && apiKeyName ? `${t.project}/.ruckcode/config.toml` : null,
      credentialsPath,
      modesPath: modesUpdated ? `${t.project}/.ruckcode/modes.toml` : null,
      modesUpdated
    };
  }

  async keysList(t: SetupTarget): Promise<KeysList> {
    this.calls.push('keysList');
    await wait(5);
    return this.buildKeys(t.project);
  }

  async keysAdd(_t: SetupTarget, name: string, secret: string, description?: string) {
    this.calls.push(`keysAdd:${name}`);
    await wait(5);
    if (!/^[A-Za-z0-9._-]+$/.test(name)) throw new Error(`invalid key name \`${name}\``);
    if (!secret.trim()) throw new Error('the key is empty');
    this.keys.set(name, {
      description: description || null,
      createdAt: this.keys.get(name)?.createdAt ?? new Date().toISOString().replace(/\.\d+Z$/, 'Z'),
      secret: secret.trim()
    });
  }

  async keysRemove(_t: SetupTarget, name: string) {
    this.calls.push(`keysRemove:${name}`);
    await wait(5);
    if (!this.keys.delete(name)) throw new Error(`no key named \`${name}\``);
    return {
      stillUsedBy: this.refs
        .filter((r) => r.key === name)
        .map((r) => `profile \`${r.profile}\` (${r.scope === 'user' ? 'user' : 'this project'})`)
    };
  }

  async keysUse(_t: SetupTarget, name: string, profile: string | null, projectScope: boolean) {
    this.calls.push(`keysUse:${name}:${profile ?? ''}:${projectScope ? 'project' : 'user'}`);
    await wait(5);
    if (!this.keys.has(name)) throw new Error(`no key named \`${name}\``);
    const p = profile ?? this.defaultProfile;
    const scope = projectScope ? 'project' : 'user';
    this.refs = this.refs.filter((r) => !(r.profile === p && r.scope === scope));
    this.refs.push({ key: name, profile: p, scope });
    const info = this.profiles.find((x) => x.name === p);
    if (info && !projectScope) info.apiKeyName = name;
  }

  async initModes(t: SetupTarget, opts: InitInput = {}): Promise<InitResult> {
    this.calls.push('initModes');
    return this.modes.initModes(t, opts);
  }
}
