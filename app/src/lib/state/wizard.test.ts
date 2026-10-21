import { describe, expect, it } from 'vitest';
import { FakeSetup } from '../host/fake-setup';
import {
  isLocalEndpoint,
  suggestedProfileName,
  suggestionForKey,
  suggestionForUrl,
  validateEndpoint,
  type SetupTarget
} from '../host/setup';
import { SetupWizard, parseChoice } from './wizard.svelte';

const target: SetupTarget = { project: '/demo/project' };

async function make(
  opts: ConstructorParameters<typeof FakeSetup>[0] = { fresh: true, delayMs: 0 }
) {
  const api = new FakeSetup({ delayMs: 0, ...opts });
  const w = new SetupWizard(api, target, await api.presets(), await api.state(target));
  return { api, w };
}

/** Wait for the running compatibility check. */
async function settle(w: SetupWizard) {
  for (let i = 0; i < 50 && w.checkPhase === 'checking'; i++)
    await new Promise((r) => setTimeout(r, 0));
}

/** Walk key -> models: paste an OpenRouter key (the URL is hinted), check, land on models. */
async function toModels(w: SetupWizard, key = 'sk-or-v1-abcdef', url?: string) {
  if (key) {
    w.setAuth('new');
    w.newKeySecret = key;
    w.keyEdited();
  } else {
    w.setAuth('none');
  }
  if (url) w.setEndpoint(url);
  await w.next(); // endpoint
  await w.next(); // check (starts probing)
  await settle(w);
  await w.next(); // models
}

describe('endpoint helpers (mirror the Rust core)', () => {
  it('validates, trims and strips trailing slashes', () => {
    expect(validateEndpoint('  https://api.openai.com/v1/ ')).toEqual({
      url: 'https://api.openai.com/v1'
    });
    for (const bad of ['', '  ', 'localhost:11434', 'ftp://x.com', 'http://', 'http://a b'])
      expect(validateEndpoint(bad)).toHaveProperty('error');
  });

  it('knows local endpoints, suggestions and profile names', async () => {
    const { w } = await make();
    for (const u of [
      'http://localhost:1/v1',
      'http://192.168.1.5:8000',
      'http://10.0.0.2',
      'http://box.local'
    ])
      expect(isLocalEndpoint(u)).toBe(true);
    for (const u of ['https://api.openai.com/v1', 'http://172.32.0.1', 'http://8.8.8.8'])
      expect(isLocalEndpoint(u)).toBe(false);
    const s = w.suggestions;
    expect(s.map((x) => x.id)).toEqual([
      'anthropic',
      'openai',
      'openrouter',
      'ollama',
      'lmstudio',
      'vllm'
    ]);
    expect(suggestionForUrl(s, 'http://127.0.0.1:1234/v1')?.id).toBe('lmstudio');
    expect(suggestionForUrl(s, 'http://localhost:9/v1')).toBeUndefined();
    expect(suggestionForKey(s, 'sk-ant-api03-x')?.id).toBe('anthropic');
    expect(suggestionForKey(s, 'sk-or-v1-x')?.id).toBe('openrouter');
    expect(suggestionForKey(s, 'sk-proj-x')?.id).toBe('openai');
    expect(suggestionForKey(s, 'abc')).toBeUndefined();
    const name = (k: 'anthropic' | 'openai', u: string) => suggestedProfileName(s, k, u);
    expect(name('anthropic', 'https://api.anthropic.com')).toBe('anthropic');
    expect(name('openai', 'https://openrouter.ai/api/v1')).toBe('openrouter');
    expect(name('openai', 'http://localhost:11434/v1')).toBe('ollama');
    expect(name('openai', 'http://localhost:9999/v1')).toBe('localhost');
    expect(name('openai', 'https://llm.corp.example.com:8443/v1')).toBe('llm-corp-example-com');
  });
});

describe('SetupWizard state machine', () => {
  it('key first: paste a key, the URL is hinted, validated, and checked before models', async () => {
    const { w } = await make();
    expect(w.step).toBe('key');
    expect(w.authKind).toBe('new');
    expect(w.canNext).toBe(false);
    expect(w.problem).toMatch(/Paste your API key/);
    w.newKeySecret = 'sk-ant-api03-zzz';
    w.keyEdited();
    expect(w.endpoint).toBe('https://api.anthropic.com'); // hinted from the key shape
    await w.next();
    expect(w.step).toBe('endpoint');
    // The URL is required and validated inline.
    w.setEndpoint('');
    expect(w.canNext).toBe(false);
    expect(w.problem).toMatch(/required/);
    w.setEndpoint('localhost:8000');
    expect(w.canNext).toBe(false);
    expect(w.endpointError).toMatch(/http/);
    w.setEndpoint('  https://api.anthropic.com/  ');
    expect(w.canNext).toBe(true);
    await w.next();
    expect(w.endpoint).toBe('https://api.anthropic.com');
    expect(w.step).toBe('check');
    expect(w.checkPhase).toBe('checking');
    expect(w.canNext).toBe(false);
    await settle(w);
    expect(w.checkPhase).toBe('ok');
    expect(w.probe?.message).toBe('Anthropic API ✓ (3 models)');
    expect(w.kind).toBe('anthropic');
    await w.next();
    expect(w.step).toBe('models');
    expect(w.profileName).toBe('anthropic');
    expect(w.available).toContain('claude-opus-5-5');
  });

  it('a typed URL is never overwritten by key hints; keyless is for local servers', async () => {
    const { w } = await make();
    w.setEndpoint('http://gpu-box:8000/v1');
    w.newKeySecret = 'sk-ant-x';
    w.keyEdited();
    expect(w.endpoint).toBe('http://gpu-box:8000/v1');
    w.setAuth('none');
    expect(w.keylessHint).toBe(true); // remote host without a key
    w.setEndpoint('http://localhost:11434/v1');
    expect(w.keylessHint).toBe(false);
  });

  it('Ollama: no key, local URL, detected OpenAI-compatible, profile named ollama', async () => {
    const { w } = await make();
    await toModels(w, '', 'http://localhost:11434/v1');
    expect(w.kind).toBe('openai');
    expect(w.probe?.message).toBe('OpenAI-compatible API ✓ (3 models)');
    expect(w.profileName).toBe('ollama');
    w.toggleModel('llama3.1:8b');
    expect(w.plan()).toMatchObject({
      provider: 'openai',
      baseUrl: 'http://localhost:11434/v1',
      apiKeyName: null,
      newKey: null,
      keyScope: 'user'
    });
  });

  it('failed checks explain themselves: 401, wrong path, refused; retry/override/manual', async () => {
    const { w } = await make();
    w.setAuth('new');
    w.newKeySecret = 'bad';
    w.setEndpoint('https://openrouter.ai/api/v1');
    await w.next();
    await w.next();
    await settle(w);
    expect(w.checkPhase).toBe('failed');
    expect(w.checkError).toMatch(/rejected the API key \(HTTP 401\)/);
    expect(w.canNext).toBe(false);
    // Wrong path: 404 hint about /v1.
    w.back();
    w.newKeySecret = 'sk-or-ok';
    w.keyEdited();
    w.setEndpoint('https://openrouter.ai/api');
    await w.next();
    await settle(w);
    expect(w.checkError).toMatch(/404.*\/v1/);
    // Override the flavor: the Anthropic API does not answer at an OpenAI URL either.
    w.overrideKind();
    await settle(w);
    expect(w.forced).toBe('anthropic');
    expect(w.checkPhase).toBe('failed');
    // Connection refused.
    w.back();
    w.setEndpoint('http://broken.example/v1');
    await w.next();
    await settle(w);
    expect(w.checkError).toMatch(/Could not connect/);
    // Continue without a model list: type ids by hand.
    w.continueManually();
    expect(w.step).toBe('models');
    expect(w.canNext).toBe(false);
    w.manualText = 'my-model, other-model';
    w.addManual();
    expect(w.selected).toEqual(['my-model', 'other-model']);
    expect(w.defaultModel).toBe('my-model');
    expect(w.canNext).toBe(true);
  });

  it('manual override re-checks only the chosen flavor', async () => {
    const { w } = await make();
    w.setAuth('none');
    w.setEndpoint('http://localhost:8000/v1');
    await w.next();
    await w.next();
    await settle(w);
    expect(w.kind).toBe('openai');
    w.overrideKind();
    await settle(w);
    expect(w.forced).toBe('anthropic');
    expect(w.checkPhase).toBe('failed');
    w.overrideKind();
    await settle(w);
    expect(w.forced).toBe('openai');
    expect(w.checkPhase).toBe('ok');
  });

  it('editing the key or URL invalidates the check; a stale probe is ignored', async () => {
    const { api, w } = await make();
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const orig = api.probe.bind(api);
    let n = 0;
    api.probe = async (t, req) => {
      if (n++ === 0) await gate;
      return orig(t, req);
    };
    w.setAuth('none');
    w.setEndpoint('http://localhost:11434/v1');
    await w.next();
    const first = w.runCheck();
    w.setEndpoint('http://localhost:8000/v1'); // invalidates
    expect(w.checkPhase).toBe('idle');
    const second = w.runCheck();
    await second;
    release();
    await first;
    expect(w.available).toEqual(['local-model-a', 'local-model-b']);
  });

  it('auth: each kind validates; leaving "paste" wipes the secret', async () => {
    const { api, w } = await make({ delayMs: 0 });
    await api.keysAdd(target, 'work', 'sk-12345678901');
    w.state = await api.state(target);
    w.setAuth('existing');
    expect(w.existingKey).toBe('work');
    expect(w.problem).toBeNull();
    w.setAuth('new');
    expect(w.problem).toMatch(/Paste your API key/);
    w.newKeySecret = 'sk-abc';
    expect(w.problem).toBeNull();
    w.setAuth('none');
    expect(w.newKeySecret).toBe('');
    expect(w.problem).toBeNull();
  });

  it('existing profile needs an overwrite confirmation; key names stay unique', async () => {
    const { api, w } = await make({ delayMs: 0 });
    await api.keysAdd(target, 'mock', 'sk-12345678901');
    w.state = await api.state(target);
    w.step = 'profile';
    w.profileName = 'mock';
    expect(w.profileExists).toBe(true);
    expect(w.canNext).toBe(false);
    w.overwrite = true;
    expect(w.canNext).toBe(true);
    w.setAuth('new');
    w.newKeySecret = 'sk-x';
    expect(w.newKeyName).toBe('mock-2');
  });

  it('selects, filters and picks a default model', async () => {
    const { w } = await make();
    await toModels(w);
    expect(w.step).toBe('models');
    expect(w.available).toContain('openai/gpt-5');
    expect(w.canNext).toBe(false);
    w.filter = 'claude';
    expect(w.filtered).toEqual(['anthropic/claude-sonnet-5-5']);
    w.toggleAllFiltered();
    expect(w.selected).toEqual(['anthropic/claude-sonnet-5-5']);
    expect(w.defaultModel).toBe('anthropic/claude-sonnet-5-5');
    w.filter = '';
    w.toggleAllFiltered();
    expect(w.selected.length).toBe(w.available.length);
    w.toggleAllFiltered();
    expect(w.selected).toEqual([]);
    expect(w.defaultModel).toBe('');
    w.toggleModel('openai/gpt-5');
    w.toggleModel('qwen/qwen3-coder');
    w.defaultModel = 'qwen/qwen3-coder';
    w.toggleModel('qwen/qwen3-coder');
    expect(w.defaultModel).toBe('openai/gpt-5');
    expect(w.canNext).toBe(true);
  });

  it('only changed modes go into the plan; mode choices span all profiles', async () => {
    const { w } = await make({ delayMs: 0 });
    await toModels(w, '', 'http://localhost:11434/v1');
    w.toggleModel('llama3.1:8b');
    w.toggleModel('mistral:7b');
    await w.next(); // profile
    expect(w.step).toBe('profile');
    await w.next(); // modes
    expect(w.step).toBe('modes');
    expect(w.choiceGroups.map((g) => g.profile)).toEqual(['mock', 'ollama']);
    expect(w.modeRows.map((r) => r.id)).toEqual(['plan', 'build', 'verify']);
    expect(w.choices.verify).toBe('::mock-small');
    expect(w.plan().modeModels).toEqual([]);
    w.choices.plan = 'ollama::mistral:7b';
    w.choices.verify = '';
    expect(w.plan().modeModels).toEqual([
      { mode: 'plan', profile: 'ollama', model: 'mistral:7b' },
      { mode: 'verify', profile: null, model: null }
    ]);
    expect(parseChoice('a::b::c')).toEqual({ profile: 'a', model: 'b::c' });
    expect(parseChoice('')).toBeNull();
  });

  it('offers init when modes.toml is missing and then lists the modes', async () => {
    const { w } = await make({ fresh: true, delayMs: 0 });
    await toModels(w, '', 'http://localhost:11434/v1');
    w.toggleModel('llama3.1:8b');
    await w.next();
    await w.next();
    expect(w.state.modes.status).toBe('missing');
    expect(w.modeRows).toEqual([]);
    await w.runInit();
    expect(w.state.modes.status).toBe('ready');
    expect(w.modeRows.length).toBeGreaterThan(1);
  });

  it('plan carries a new key (named after the profile) and clears the secret after saving', async () => {
    const { api, w } = await make();
    w.newKeyDescription = 'my key';
    await toModels(w, 'sk-ant-supersecret-1234', 'https://api.anthropic.com');
    w.toggleModel('claude-sonnet-5-5');
    await w.next(); // profile
    await w.next(); // modes
    await w.next(); // review
    expect(w.step).toBe('review');
    expect(w.diff).toContain('[profiles.anthropic]');
    expect(w.diff).not.toContain('supersecret');
    const plan = w.plan();
    expect(plan.newKey).toEqual({
      name: 'anthropic',
      secret: 'sk-ant-supersecret-1234',
      description: 'my key'
    });
    expect(plan).toMatchObject({
      provider: 'anthropic',
      apiKeyName: 'anthropic',
      keyScope: 'user',
      baseUrl: 'https://api.anthropic.com'
    });
    expect(JSON.stringify(plan)).not.toContain('apiKeyEnv');
    await w.next(); // save
    expect(w.step).toBe('done');
    expect(w.newKeySecret).toBe('');
    expect(JSON.stringify(w.plan())).not.toContain('supersecret');
    expect(JSON.stringify(w.state)).not.toContain('supersecret');
    expect(api.lastApplied).toEqual({ profileName: 'anthropic', hadNewKey: true });
    expect(w.state.needsOnboarding).toBe(false);
    expect(w.state.keys.keys[0]).toMatchObject({ name: 'anthropic', masked: '…1234' });
    expect(JSON.stringify(w.state)).not.toContain('"env"');
  });

  it('keeps the secret when saving fails so it can be retried', async () => {
    const { api, w } = await make();
    await toModels(w, 'sk-ant-keepme', 'https://api.anthropic.com');
    w.toggleModel('claude-haiku-5-5');
    await w.next();
    await w.next();
    await w.next();
    api.apply = async () => {
      throw new Error('disk full');
    };
    await w.next();
    expect(w.step).toBe('review');
    expect(w.error).toBe('disk full');
    expect(w.newKeySecret).toBe('sk-ant-keepme');
  });
});
