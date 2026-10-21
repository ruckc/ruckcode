import { describe, expect, it } from 'vitest';
import { FakeSetup } from '../host/fake-setup';
import type { SetupTarget } from '../host/setup';
import { SetupWizard, isHttpUrl, parseChoice } from './wizard.svelte';

const target: SetupTarget = { project: '/demo/project' };

async function make(
  opts: ConstructorParameters<typeof FakeSetup>[0] = { fresh: true, delayMs: 0 }
) {
  const api = new FakeSetup({ delayMs: 0, ...opts });
  const w = new SetupWizard(api, target, await api.presets(), await api.state(target));
  return { api, w };
}

/** Walk provider -> models for the OpenRouter preset with the (set) env var. */
async function toModels(w: SetupWizard, preset = 'openrouter') {
  w.choosePreset(preset);
  await w.next(); // endpoint
  await w.next(); // auth
  await w.next(); // profile
  await w.next(); // models (starts fetching)
  await w.fetchModels();
}

describe('SetupWizard state machine', () => {
  it('validates each step before advancing', async () => {
    const { w } = await make();
    expect(w.step).toBe('provider');
    expect(w.canNext).toBe(false);
    await w.next();
    expect(w.step).toBe('provider');
    w.choosePreset('custom');
    await w.next();
    expect(w.step).toBe('endpoint');
    expect(w.problem).toMatch(/URL/);
    w.endpoint = 'localhost:8000';
    expect(w.canNext).toBe(false);
    w.endpoint = 'http://localhost:8000/v1';
    await w.next();
    expect(w.step).toBe('auth');
    expect(w.authKind).toBe('none');
    await w.next();
    expect(w.step).toBe('profile');
    w.profileName = 'bad name';
    expect(w.canNext).toBe(false);
    w.profileName = 'vllm';
    expect(w.canNext).toBe(true);
    w.back();
    expect(w.step).toBe('auth');
  });

  it('presets prefill endpoint, env var and profile name', async () => {
    const { w } = await make();
    w.choosePreset('anthropic');
    expect([w.endpoint, w.envName, w.authKind, w.profileName]).toEqual([
      'https://api.anthropic.com',
      'ANTHROPIC_API_KEY',
      'env',
      'anthropic'
    ]);
    expect(w.envSet).toBe(false);
    w.choosePreset('ollama');
    expect(w.authKind).toBe('none');
    w.choosePreset('openrouter');
    expect(w.envSet).toBe(true);
  });

  it('auth: each kind validates its own fields', async () => {
    const { api, w } = await make({ delayMs: 0 });
    await api.keysAdd(target, 'work', 'sk-12345678901');
    w.state = await api.state(target);
    w.choosePreset('openai');
    w.step = 'auth';
    w.setAuth('existing');
    expect(w.existingKey).toBe('work');
    expect(w.problem).toBeNull();
    w.setAuth('new');
    expect(w.problem).toMatch(/Key name/);
    w.newKeyName = 'team';
    expect(w.problem).toMatch(/Enter the API key/);
    w.newKeySecret = 'sk-abc';
    expect(w.problem).toBeNull();
    w.setAuth('env');
    expect(w.newKeySecret).toBe(''); // leaving "new" wipes the secret
    w.envName = '1BAD';
    expect(w.problem).toMatch(/environment variable/);
  });

  it('existing profile needs an overwrite confirmation', async () => {
    const { w } = await make({ delayMs: 0 });
    w.choosePreset('openai');
    w.step = 'profile';
    w.profileName = 'mock';
    expect(w.profileExists).toBe(true);
    expect(w.canNext).toBe(false);
    w.overwrite = true;
    expect(w.canNext).toBe(true);
  });

  it('fetches models, selects, filters, picks a default', async () => {
    const { w } = await make();
    await toModels(w);
    expect(w.step).toBe('models');
    expect(w.modelsPhase).toBe('ready');
    expect(w.available).toContain('openai/gpt-5');
    expect(w.canNext).toBe(false);
    w.filter = 'claude';
    expect(w.filtered).toEqual(['anthropic/claude-sonnet-5-5']);
    w.toggleAllFiltered();
    expect(w.selected).toEqual(['anthropic/claude-sonnet-5-5']);
    expect(w.defaultModel).toBe('anthropic/claude-sonnet-5-5');
    w.filter = '';
    w.toggleAllFiltered(); // select all
    expect(w.selected.length).toBe(w.available.length);
    w.toggleAllFiltered(); // and none
    expect(w.selected).toEqual([]);
    expect(w.defaultModel).toBe('');
    w.toggleModel('openai/gpt-5');
    w.toggleModel('qwen/qwen3-coder');
    w.defaultModel = 'qwen/qwen3-coder';
    w.toggleModel('qwen/qwen3-coder');
    expect(w.defaultModel).toBe('openai/gpt-5');
    expect(w.canNext).toBe(true);
  });

  it('a failed fetch allows retry and manual entry', async () => {
    const { w } = await make();
    w.choosePreset('custom');
    w.endpoint = 'http://broken.example/v1';
    w.step = 'profile';
    w.profileName = 'x';
    await w.next();
    await w.fetchModels(true);
    expect(w.modelsPhase).toBe('error');
    expect(w.modelsError).toMatch(/401/);
    expect(w.canNext).toBe(false);
    w.manualText = 'my-model, other-model';
    w.addManual();
    expect(w.selected).toEqual(['my-model', 'other-model']);
    expect(w.defaultModel).toBe('my-model');
    expect(w.modelsPhase).toBe('ready');
    expect(w.canNext).toBe(true);
    // retry after fixing the endpoint keeps the manual ids available
    w.endpoint = 'http://localhost:8000/v1';
    await w.fetchModels();
    expect(w.modelsPhase).toBe('ready');
    expect(w.available).toEqual(expect.arrayContaining(['local-model-a', 'my-model']));
  });

  it('ignores a stale fetch result', async () => {
    const { api, w } = await make();
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const orig = api.listModels.bind(api);
    let n = 0;
    api.listModels = async (t, req) => {
      if (n++ === 0) await gate;
      return orig(t, req);
    };
    w.choosePreset('ollama');
    const first = w.fetchModels(true);
    w.endpoint = 'http://localhost:8000/v1';
    const second = w.fetchModels(true);
    await second;
    release();
    await first;
    expect(w.available).toEqual(['local-model-a', 'local-model-b']);
  });

  it('only changed modes go into the plan; mode choices span all profiles', async () => {
    const { w } = await make({ delayMs: 0 });
    await toModels(w, 'ollama');
    w.toggleModel('llama3.1:8b');
    w.toggleModel('mistral:7b');
    await w.next(); // modes
    expect(w.step).toBe('modes');
    const groups = w.choiceGroups.map((g) => g.profile);
    expect(groups).toEqual(['mock', 'ollama']);
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
    await toModels(w, 'ollama');
    w.toggleModel('llama3.1:8b');
    await w.next();
    expect(w.state.modes.status).toBe('missing');
    expect(w.modeRows).toEqual([]);
    await w.runInit();
    expect(w.state.modes.status).toBe('ready');
    expect(w.modeRows.length).toBeGreaterThan(1);
  });

  it('plan carries a new key and clears the secret after saving', async () => {
    const { api, w } = await make();
    w.choosePreset('anthropic');
    w.setAuth('new');
    w.newKeyName = 'work';
    w.newKeySecret = 'sk-ant-supersecret-1234';
    w.newKeyDescription = 'my key';
    w.keyScope = 'project';
    w.step = 'profile';
    await w.next();
    await w.fetchModels();
    w.toggleModel('claude-sonnet-5-5');
    await w.next(); // modes
    await w.next(); // review
    expect(w.step).toBe('review');
    expect(w.diff).toContain('[profiles.anthropic]');
    expect(w.diff).not.toContain('supersecret');
    const plan = w.plan();
    expect(plan.newKey).toEqual({
      name: 'work',
      secret: 'sk-ant-supersecret-1234',
      description: 'my key'
    });
    expect(plan).toMatchObject({ apiKeyName: 'work', apiKeyEnv: null, keyScope: 'project' });
    await w.next(); // save
    expect(w.step).toBe('done');
    expect(w.newKeySecret).toBe('');
    expect(JSON.stringify(w.plan())).not.toContain('supersecret');
    expect(JSON.stringify(w.state)).not.toContain('supersecret');
    expect(api.lastApplied).toEqual({ profileName: 'anthropic', hadNewKey: true });
    expect(w.state.needsOnboarding).toBe(false);
    expect(w.state.keys.keys[0]).toMatchObject({ name: 'work', masked: '…1234' });
    expect(w.state.keys.keys[0].usedBy[0]).toContain('this project');
  });

  it('keeps the secret when saving fails so it can be retried', async () => {
    const { api, w } = await make();
    w.choosePreset('anthropic');
    w.setAuth('new');
    w.newKeyName = 'k';
    w.newKeySecret = 'sk-keepme';
    w.step = 'profile';
    await w.next();
    await w.fetchModels();
    w.toggleModel('claude-haiku-5-5');
    await w.next();
    await w.next();
    api.apply = async () => {
      throw new Error('disk full');
    };
    await w.next();
    expect(w.step).toBe('review');
    expect(w.error).toBe('disk full');
    expect(w.newKeySecret).toBe('sk-keepme');
  });

  it('isHttpUrl', () => {
    expect(isHttpUrl('http://localhost:11434/v1')).toBe(true);
    expect(isHttpUrl('ftp://x')).toBe(false);
    expect(isHttpUrl('')).toBe(false);
  });
});
