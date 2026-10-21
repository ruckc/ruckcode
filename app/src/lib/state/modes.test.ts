import { describe, expect, it } from 'vitest';
import { FakeModes } from '../host/fake-modes';
import type { ModesApi } from '../host/modes';
import { ModesEditor } from './modes.svelte';

const target = { project: '/demo/project' };

async function loaded(opts: ConstructorParameters<typeof FakeModes>[0] = {}, api?: ModesApi) {
  const fake = new FakeModes({ delayMs: 0, ...opts });
  const e = new ModesEditor(api ?? fake, target, 0);
  await e.load();
  return { e, fake };
}

describe('ModesEditor', () => {
  it('loads the document, starts clean and tracks edits', async () => {
    const { e } = await loaded();
    expect(e.phase).toBe('ready');
    expect(e.status).toBe('ready');
    expect(e.doc.modes.map((m) => m.id)).toEqual(['plan', 'build', 'verify']);
    expect(e.doc.modes[0].originalId).toBe('plan');
    expect(e.dirty).toBe(false);
    expect(e.canReview).toBe(false);
    e.doc.modes[0].prompt = 'new prompt';
    expect(e.dirty).toBe(true);
    expect(e.canReview).toBe(true);
    e.discard();
    expect(e.dirty).toBe(false);
    // formatting-only edits are not changes
    e.doc.modes[0].name = `${e.doc.modes[0].name}`;
    e.doc.modes[2].profile = '';
    expect(e.dirty).toBe(false);
  });

  it('instant client validation, then the host answer replaces it', async () => {
    const { e, fake } = await loaded();
    e.doc.modes[1].hooks.pre_tool_use = [
      { run: 'x', glob: null, matcher: '(', onFailure: 'ignore', maxFixTurns: 2, timeoutSecs: 120 }
    ];
    expect(e.errors).toBe(1);
    expect(e.issues[0].path).toBe('modes[1].hooks.pre_tool_use[0].matcher');
    expect(e.canReview).toBe(false);
    await e.validateNow();
    expect(fake.calls).toContain('validate');
    expect(e.errors).toBe(1);
    expect(e.serverIssues?.length).toBe(1);
    // an edit makes the host answer stale: client issues apply until the next answer
    e.doc.modes[1].hooks.pre_tool_use![0].matcher = 'Bash|(Edit';
    expect(e.issues).toEqual(e.clientIssues);
    e.doc.modes[1].hooks.pre_tool_use![0].matcher = 'Bash|Edit';
    expect(e.errors).toBe(0);
    expect(e.canReview).toBe(true);
  });

  it('keeps client issues and notes when the host cannot validate', async () => {
    const fake = new FakeModes({ delayMs: 0 });
    const api: ModesApi = {
      get: (t) => fake.get(t),
      validate: async () => {
        throw new Error('wsl.exe failed');
      },
      preview: (t, d) => fake.preview(t, d),
      apply: (t, d) => fake.apply(t, d),
      initModes: (t, o) => fake.initModes(t, o)
    };
    const { e } = await loaded({}, api);
    e.doc.defaultMode = 'ghost';
    await e.validateNow();
    expect(e.serverNote).toContain('wsl.exe failed');
    expect(e.errors).toBe(1);
  });

  it('ignores a stale host answer', async () => {
    const fake = new FakeModes({ delayMs: 0 });
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const api: ModesApi = {
      get: (t) => fake.get(t),
      validate: async (t, d) => {
        await gate;
        return fake.validate(t, d);
      },
      preview: (t, d) => fake.preview(t, d),
      apply: (t, d) => fake.apply(t, d),
      initModes: (t, o) => fake.initModes(t, o)
    };
    const { e } = await loaded({}, api);
    e.doc.defaultMode = 'ghost';
    const p = e.validateNow();
    e.doc.defaultMode = 'plan'; // fixed while the answer is in flight
    release();
    await p;
    expect(e.serverIssues).toBeNull();
    expect(e.errors).toBe(0);
  });

  it('adds blank, template and duplicate modes and selects them', async () => {
    const { e } = await loaded();
    expect(e.addBlank()).toBe('mode');
    expect(e.selected).toBe('mode');
    expect(e.addBlank()).toBe('mode-2');
    const t = e.catalog!.templates.find((x) => x.set === 'full' && x.mode.id === 'review')!;
    expect(e.addTemplate(t)).toBe('review');
    expect(e.duplicate('build')).toBe('build-copy');
    expect(e.doc.modes.map((m) => m.id)).toEqual([
      'plan',
      'build',
      'verify',
      'mode',
      'mode-2',
      'review',
      'build-copy'
    ]);
    expect(e.dirty).toBe(true);
    expect(e.selectedMode?.id).toBe('build-copy');
  });

  it('renames with references, validates ids, and keeps the selection', async () => {
    const { e } = await loaded();
    e.select('build');
    expect(e.idProblem('build', 'verify')).toMatch(/exists/);
    expect(e.idProblem('build', 'bad id')).toMatch(/letters/);
    expect(e.idProblem('build', '')).toMatch(/required/);
    expect(e.rename('build', 'verify')).toBe(false);
    expect(e.references('build').length).toBe(3);
    expect(e.rename('build', 'implement')).toBe(true);
    expect(e.selected).toBe('implement');
    expect(e.doc.modes[0].next).toEqual(['implement']);
    expect(e.doc.modes[1].originalId).toBe('build');
    expect(e.errors).toBe(0);
  });

  it('reorders, sets the default and deletes with references', async () => {
    const { e } = await loaded();
    e.move('verify', 0);
    expect(e.doc.modes.map((m) => m.id)).toEqual(['verify', 'plan', 'build']);
    e.setDefault('verify');
    e.select('build');
    e.remove('build');
    expect(e.doc.modes.map((m) => m.id)).toEqual(['verify', 'plan']);
    expect(e.doc.modes[1].next).toEqual([]);
    expect(e.selected).toBe('plan');
    e.remove('verify');
    expect(e.doc.defaultMode).toBe('plan');
    expect(e.errors).toBe(0);
  });

  it('reviews then saves, reloading what the host stored', async () => {
    const { e, fake } = await loaded();
    e.doc.modes[0].prompt = 'Changed.';
    await e.review();
    expect(e.step).toBe('review');
    expect(e.diff).toContain('+prompt = "Changed."');
    expect(e.diff).toContain('-prompt = """');
    await e.save();
    expect(e.step).toBe('saved');
    expect(e.dirty).toBe(false);
    expect(e.savedChanged).toBe(true);
    expect(fake.lastApplied?.modes[0].prompt).toBe('Changed.');
    expect(e.original.modes[0].prompt).toBe('Changed.');
    // unchanged: nothing to review
    e.doc.modes[0].prompt = 'Changed.';
    expect(e.canReview).toBe(false);
  });

  it('shows a save error on the review step and keeps the draft', async () => {
    const fake = new FakeModes({ delayMs: 0 });
    const api: ModesApi = {
      get: (t) => fake.get(t),
      validate: (t, d) => fake.validate(t, d),
      preview: (t, d) => fake.preview(t, d),
      apply: async () => {
        throw new Error('disk full');
      },
      initModes: (t, o) => fake.initModes(t, o)
    };
    const { e } = await loaded({}, api);
    e.doc.modes[0].description = 'x';
    await e.review();
    await e.save();
    expect(e.step).toBe('review');
    expect(e.problem).toBe('disk full');
    expect(e.dirty).toBe(true);
    e.backToEdit();
    expect(e.problem).toBeNull();
  });

  it('will not review a document with errors', async () => {
    const { e, fake } = await loaded();
    e.doc.defaultMode = 'nope';
    await e.review();
    expect(e.step).toBe('edit');
    expect(fake.calls).not.toContain('preview');
  });

  it('offers rc init when modes.toml is missing, and a blank file', async () => {
    const { e, fake } = await loaded({ fresh: true });
    expect(e.status).toBe('missing');
    expect(e.editable).toBe(false);
    await e.initProject({ modes: ['explore', 'review'] });
    expect(e.initialized).toContain('modes.toml');
    expect(e.status).toBe('ready');
    expect(e.doc.modes.map((m) => m.id)).toEqual(['explore', 'review']);
    expect(e.doc.modes[0].next).toEqual([]); // design was not selected
    expect(fake.calls).toContain('initModes');

    const b = await loaded({ fresh: true });
    b.e.startBlank();
    expect(b.e.editable).toBe(true);
    expect(b.e.dirty).toBe(true);
    expect(b.e.errors).toBe(0);
    await b.e.review();
    expect(b.e.diff).toContain('+[modes.default]');
    await b.e.save();
    expect(b.e.status).toBe('ready');
    expect(b.e.doc.modes[0].originalId).toBe('default');
    b.e.discard();
  });

  it('reports a broken file and can overwrite it', async () => {
    const { e } = await loaded({ broken: true });
    expect(e.status).toBe('broken');
    expect(e.view?.error).toContain('parsing modes.toml');
    expect(e.editable).toBe(false);
    await e.initProject({});
    expect(e.problem).toContain('already exists');
    await e.initProject({ force: true });
    expect(e.status).toBe('ready');
  });

  it('surfaces load failures', async () => {
    const fake = new FakeModes({ delayMs: 0 });
    const e = new ModesEditor(
      {
        get: async () => {
          throw new Error('rc not found');
        },
        validate: (t, d) => fake.validate(t, d),
        preview: (t, d) => fake.preview(t, d),
        apply: (t, d) => fake.apply(t, d),
        initModes: (t, o) => fake.initModes(t, o)
      },
      target,
      0
    );
    await e.load();
    expect(e.phase).toBe('error');
    expect(e.loadError).toBe('rc not found');
  });
});
