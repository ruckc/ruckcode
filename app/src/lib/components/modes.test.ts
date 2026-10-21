// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { demoDoc, FakeModes } from '../host/fake-modes';
import type { Catalog, ModeDto } from '../host/modes';
import ChipHarness from './test-harness/ChipHarness.svelte';
import HooksHarness from './test-harness/HooksHarness.svelte';
import PermsHarness from './test-harness/PermsHarness.svelte';
import HooksEditor from './HooksEditor.svelte';
import FlowDiagram from './FlowDiagram.svelte';
import ModesView from './ModesView.svelte';

const target = { project: '/demo/project' };
/** The list button that selects mode `id` (not its move buttons). */
const pick = (list: HTMLElement, id: string) =>
  within(list)
    .getAllByRole('button')
    .find((b) => b.className.includes('pick') && b.textContent?.includes(id))!;
const catalog = async (): Promise<Catalog> =>
  (await new FakeModes({ delayMs: 0 }).get(target)).catalog;

async function mounted(opts: ConstructorParameters<typeof FakeModes>[0] = {}) {
  const api = new FakeModes({ delayMs: 0, ...opts });
  const ondirty = vi.fn();
  const r = render(ModesView, {
    props: { api, target, ondirty, canReconnect: true, onreconnect: vi.fn() }
  });
  return { api, ondirty, r };
}

describe('ChipInput', () => {
  it('adds on Enter, removes, previews and flags problems', async () => {
    let values: string[] = [];
    render(ChipHarness, {
      props: {
        initial: ['test*'],
        onvalues: (v: string[]) => (values = v),
        label: 'Mise tasks',
        suggestions: ['build', 'lint'],
        preview: (v: string) => `preview of ${v}`,
        problem: (v: string) => (v === '[' ? 'bad glob' : null)
      }
    });
    expect(screen.getByText('preview of test*')).toBeInTheDocument();
    const input = screen.getByRole('textbox', { name: 'Add to Mise tasks' });
    await fireEvent.input(input, { target: { value: '[' } });
    expect(screen.getByRole('status')).toHaveTextContent('bad glob');
    await fireEvent.input(input, { target: { value: 'docs*' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(values).toEqual(['test*', 'docs*']));
    await fireEvent.click(screen.getByRole('button', { name: '+ build' }));
    await waitFor(() => expect(values).toEqual(['test*', 'docs*', 'build']));
    expect(screen.queryByRole('button', { name: '+ build' })).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Remove docs*' }));
    await waitFor(() => expect(values).toEqual(['test*', 'build']));
  });
});

describe('PermissionsTable', () => {
  it('adds, changes and removes rules and rejects bad globs', async () => {
    let permissions: Record<string, string> = {};
    render(PermsHarness, {
      props: {
        initial: { Bash: 'ask' },
        catalog: await catalog(),
        onchange: (p: Record<string, string>) => (permissions = p)
      }
    });
    expect(screen.getByRole('table', { name: 'Permissions' })).toHaveTextContent('default ask');
    await fireEvent.change(screen.getByLabelText('Permission for Bash'), {
      target: { value: 'deny' }
    });
    await waitFor(() => expect(permissions).toEqual({ Bash: 'deny' }));
    const input = screen.getByLabelText('Tool or glob');
    await fireEvent.input(input, { target: { value: '[' } });
    expect(screen.getByRole('button', { name: 'Add rule' })).toBeDisabled();
    await fireEvent.input(input, { target: { value: 'mise__*' } });
    await fireEvent.change(screen.getByLabelText('New rule permission'), {
      target: { value: 'allow' }
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Add rule' }));
    await waitFor(() => expect(permissions).toEqual({ Bash: 'deny', 'mise__*': 'allow' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Remove rule Bash' }));
    await waitFor(() => expect(permissions).toEqual({ 'mise__*': 'allow' }));
  });
});

describe('HooksEditor', () => {
  it('adds, edits, reorders and removes hooks and shows field issues', async () => {
    let hooks: ModeDto['hooks'] = {};
    const cat = await catalog();
    const props = {
      initial: {} as ModeDto['hooks'],
      catalog: cat,
      issues: [] as { path: string; message: string; severity: 'error' }[],
      onchange: (h: ModeDto['hooks']) => (hooks = h)
    };
    const r = render(HooksHarness, { props });
    await fireEvent.click(screen.getByRole('button', { name: 'Add post_edit hook' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Add post_edit hook' }));
    await waitFor(() => expect(hooks.post_edit).toHaveLength(2));
    await fireEvent.input(screen.getByLabelText('Command for post_edit hook 1'), {
      target: { value: 'fmt {files}' }
    });
    await fireEvent.input(screen.getByLabelText('Command for post_edit hook 2'), {
      target: { value: 'lint' }
    });
    // glob only for post_edit; matcher only for tool hooks
    await fireEvent.input(screen.getByLabelText('Glob for post_edit hook 1'), {
      target: { value: '*.rs' }
    });
    await waitFor(() =>
      expect(hooks.post_edit![0]).toMatchObject({ run: 'fmt {files}', glob: '*.rs' })
    );
    expect(screen.queryByLabelText('Matcher for post_edit hook 1')).toBeNull();
    // fix turns appear only for fix_turn
    expect(screen.queryByLabelText('Max fix turns for post_edit hook 1')).toBeNull();
    await fireEvent.change(screen.getByLabelText('On failure for post_edit hook 1'), {
      target: { value: 'fix_turn' }
    });
    await waitFor(() =>
      expect(screen.getByLabelText('Max fix turns for post_edit hook 1')).toHaveValue(2)
    );
    // reorder
    await fireEvent.click(screen.getByRole('button', { name: 'Move post_edit hook 2 up' }));
    await waitFor(() =>
      expect(hooks.post_edit!.map((h) => h.run)).toEqual(['lint', 'fmt {files}'])
    );
    // tool hooks have a matcher
    await fireEvent.click(screen.getByText('pre_tool_use', { selector: 'code' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Add pre_tool_use hook' }));
    expect(screen.getByLabelText('Matcher for pre_tool_use hook 1')).toBeInTheDocument();
    expect(screen.queryByLabelText('Glob for pre_tool_use hook 1')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Remove pre_tool_use hook 1' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Remove post_edit hook 1' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Remove post_edit hook 1' }));
    await waitFor(() => expect(hooks.post_edit).toBeUndefined());
    r.unmount();
    // issues are shown at their field
    render(HooksHarness, {
      props: {
        ...props,
        initial: {
          stop: [
            {
              run: '',
              glob: null,
              matcher: null,
              onFailure: 'ignore',
              maxFixTurns: 2,
              timeoutSecs: 120
            }
          ]
        },
        issues: [
          {
            path: 'modes[0].hooks.stop[0].run',
            message: 'The command is required.',
            severity: 'error'
          }
        ]
      }
    });
    expect(screen.getByText('The command is required.')).toBeInTheDocument();
  });

  it('read-only view lists only events with hooks', async () => {
    const cat = await catalog();
    const hooks = demoDoc().modes[1].hooks;
    render(HooksEditor, { props: { hooks, catalog: cat, issues: [], base: 'g', readonly: true } });
    expect(screen.getByText('post_edit')).toBeInTheDocument();
    expect(screen.queryByText('session_start')).toBeNull();
    expect(screen.queryByRole('button', { name: /Add/ })).toBeNull();
  });
});

describe('FlowDiagram', () => {
  it('draws nodes and edges, selects on click and describes transitions for screen readers', async () => {
    const onselect = vi.fn();
    render(FlowDiagram, { props: { modes: demoDoc().modes, defaultMode: 'plan', onselect } });
    expect(screen.getByRole('button', { name: 'Mode Plan (default)' })).toBeInTheDocument();
    const svg = screen.getByTestId('flow');
    expect(svg.querySelector('[data-edge="verify>build"]')?.getAttribute('data-kind')).toBe('auto');
    expect(svg.querySelector('[data-edge="plan>build"]')?.getAttribute('data-kind')).toBe('next');
    expect(screen.getByRole('list', { name: 'Transitions' })).toHaveTextContent(
      'Verify to Build (auto-advance)'
    );
    await fireEvent.click(screen.getByRole('button', { name: 'Mode Build' }));
    expect(onselect).toHaveBeenCalledWith('build');
    await fireEvent.keyDown(screen.getByRole('button', { name: 'Mode Verify' }), { key: 'Enter' });
    expect(onselect).toHaveBeenCalledWith('verify');
  });
});

describe('ModesView', () => {
  it('lists modes, edits the prompt, blocks save on an invalid matcher and reports dirty state', async () => {
    const { ondirty } = await mounted();
    await screen.findByRole('list', { name: 'Modes list' });
    expect(screen.getByTestId('flow')).toBeInTheDocument();
    expect(ondirty).toHaveBeenLastCalledWith(false);
    const list = screen.getByRole('list', { name: 'Modes list' });
    await fireEvent.click(pick(list, 'build'));
    const prompt = screen.getByRole('textbox', { name: 'Prompt' });
    await fireEvent.input(prompt, { target: { value: 'Short.' } });
    expect(screen.getByTestId('prompt-count')).toHaveTextContent('6 characters');
    await waitFor(() => expect(ondirty).toHaveBeenLastCalledWith(true));
    expect(screen.getByRole('button', { name: /Review & save/ })).toBeEnabled();

    await fireEvent.click(screen.getByText('pre_tool_use', { selector: 'code' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Add pre_tool_use hook' }));
    await fireEvent.input(screen.getByLabelText('Command for pre_tool_use hook 1'), {
      target: { value: 'x' }
    });
    await fireEvent.input(screen.getByLabelText('Matcher for pre_tool_use hook 1'), {
      target: { value: '(' }
    });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Review & save/ })).toBeDisabled()
    );
    expect(screen.getByText(/Invalid regex/)).toBeInTheDocument();
    expect(screen.getByTestId('error-count')).toHaveTextContent('1 error');
    await fireEvent.input(screen.getByLabelText('Matcher for pre_tool_use hook 1'), {
      target: { value: 'Bash' }
    });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Review & save/ })).toBeEnabled()
    );
  });

  it('reviews the diff, saves and offers to reconnect', async () => {
    const { api } = await mounted();
    const list = await screen.findByRole('list', { name: 'Modes list' });
    await fireEvent.click(pick(list, 'plan'));
    await fireEvent.input(screen.getByLabelText('Description'), {
      target: { value: 'Think first' }
    });
    await fireEvent.click(screen.getByRole('button', { name: /Review & save/ }));
    const diff = await screen.findByLabelText('Changes to modes.toml');
    expect(diff).toHaveTextContent('+description = "Think first"');
    await fireEvent.click(screen.getByRole('button', { name: 'Save modes.toml' }));
    await screen.findByText(/Mode changes apply to new sessions/);
    expect(api.lastApplied?.modes[0].description).toBe('Think first');
    expect(screen.getByRole('button', { name: 'Reconnect the agent now' })).toBeInTheDocument();
  });

  it('offers rc init with a template choice when modes.toml is missing', async () => {
    const { api } = await mounted({ fresh: true });
    await screen.findByText('No modes.toml');
    await fireEvent.click(screen.getByLabelText(/Minimal/));
    await fireEvent.click(screen.getByRole('button', { name: 'Create modes.toml' }));
    await screen.findByRole('list', { name: 'Modes list' });
    expect(api.calls).toContain('initModes');
    expect(screen.getAllByRole('listitem').length).toBeGreaterThanOrEqual(3);
  });

  it('shows an unreadable file and only overwrites after confirmation', async () => {
    const { api } = await mounted({ broken: true });
    await screen.findByText('modes.toml cannot be read');
    expect(screen.getByRole('alert')).toHaveTextContent('parsing modes.toml');
    await fireEvent.click(screen.getByRole('button', { name: 'Recreate from template...' }));
    expect(api.calls).not.toContain('initModes');
    await fireEvent.click(screen.getByRole('button', { name: 'Overwrite modes.toml' }));
    await screen.findByRole('list', { name: 'Modes list' });
  });

  it('adds modes from a template and shows the new mode with its id', async () => {
    await mounted();
    await screen.findByRole('list', { name: 'Modes list' });
    await fireEvent.change(screen.getByLabelText('Add mode from template'), {
      target: { value: 'full:release' }
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Add mode' }));
    expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Release');
    expect(screen.getByRole('textbox', { name: 'Prompt' })).toHaveDisplayValue(/release phase/);
  });

  it('renames through the id field and moves references along', async () => {
    await mounted();
    const list = await screen.findByRole('list', { name: 'Modes list' });
    await fireEvent.click(pick(list, 'build'));
    const id = screen.getByRole('textbox', { name: 'Id' });
    await fireEvent.input(id, { target: { value: 'verify' } });
    expect(screen.getByText(/A mode with id `verify` exists/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Rename' })).toBeDisabled();
    await fireEvent.input(id, { target: { value: 'implement' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
    expect(within(list).getAllByText('implement').length).toBeGreaterThan(0);
    await fireEvent.click(pick(list, 'plan'));
    expect(screen.getByLabelText('next: implement')).toBeChecked();
  });

  it('deleting a mode asks first and mentions the references', async () => {
    await mounted();
    const list = await screen.findByRole('list', { name: 'Modes list' });
    await fireEvent.click(pick(list, 'build'));
    await fireEvent.click(screen.getByRole('button', { name: 'Delete mode...' }));
    expect(screen.getByRole('alert')).toHaveTextContent('referenced in 3 places');
    await fireEvent.click(screen.getByRole('button', { name: 'Delete mode' }));
    expect(within(list).queryByText('build')).toBeNull();
  });
});
