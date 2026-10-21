// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it } from 'vitest';
import { FakeSetup } from '../host/fake-setup';
import KeysTab from './KeysTab.svelte';
import SettingsDialog from './SettingsDialog.svelte';
import SetupWizardDialog from './SetupWizardDialog.svelte';

const target = { project: '/demo/project' };

beforeEach(() => {
  window.history.replaceState({}, '', '/?mock=1');
});

async function keysTab(api: FakeSetup) {
  const state = await api.state(target);
  const seen: unknown[] = [];
  const r = render(KeysTab, {
    props: {
      api,
      target,
      info: state,
      onchange: (s: unknown) => {
        seen.push(s);
        void r.rerender({ info: s as typeof state });
      }
    }
  });
  return { r, seen };
}

describe('KeysTab', () => {
  it('adds a key, clears the secret field on submit and never shows it', async () => {
    const api = new FakeSetup({ delayMs: 0 });
    await keysTab(api);
    await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'work' } });
    const secret = screen.getByLabelText('API key') as HTMLInputElement;
    expect(secret.type).toBe('password');
    await fireEvent.input(secret, { target: { value: 'sk-very-secret-9876' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Save key' }));
    await waitFor(() =>
      expect(screen.getByRole('table', { name: 'API keys' })).toBeInTheDocument()
    );
    expect(secret.value).toBe('');
    expect(screen.getByText('…9876')).toBeInTheDocument();
    expect(document.body.innerHTML).not.toContain('very-secret');
    expect(api.calls).toContain('keysAdd:work');
  });

  it('clears the secret even when saving fails', async () => {
    const api = new FakeSetup({ delayMs: 0 });
    api.keysAdd = async () => {
      throw new Error('nope');
    };
    await keysTab(api);
    await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'work' } });
    const secret = screen.getByLabelText('API key') as HTMLInputElement;
    await fireEvent.input(secret, { target: { value: 'sk-x' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Save key' }));
    await screen.findByText('nope');
    expect(secret.value).toBe('');
  });

  it('removes only after confirmation and uses a key for a project profile', async () => {
    const api = new FakeSetup({ delayMs: 0 });
    await api.keysAdd(target, 'acme', 'sk-acme-00001111', 'client');
    await keysTab(api);
    await fireEvent.click(screen.getByRole('button', { name: 'Use for profile' }));
    await fireEvent.click(screen.getByLabelText('This project only'));
    await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
    await screen.findByText(/this project only\)\./);
    expect(api.calls).toContain('keysUse:acme:mock:project');
    await screen.findByText(/profile `mock` \(this project\)/);

    await fireEvent.click(screen.getByRole('button', { name: 'Remove acme' }));
    expect(api.calls).not.toContain('keysRemove:acme');
    await fireEvent.click(screen.getByRole('button', { name: 'Confirm remove' }));
    await screen.findByText(/Still referenced by/);
    expect(api.calls).toContain('keysRemove:acme');
  });

  it('rejects invalid names', async () => {
    const api = new FakeSetup({ delayMs: 0 });
    await keysTab(api);
    await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'bad name' } });
    await fireEvent.input(screen.getByLabelText('API key'), { target: { value: 'x' } });
    expect(screen.getByRole('button', { name: 'Save key' })).toBeDisabled();
  });
});

describe('SettingsDialog', () => {
  it('lists providers, hands off to the wizard, switches tabs with arrows and closes on Escape', async () => {
    const api = new FakeSetup({ delayMs: 0 });
    let closed = false;
    let added = false;
    render(SettingsDialog, {
      props: { api, target, onclose: () => (closed = true), onaddprovider: () => (added = true) }
    });
    const tab = await screen.findByRole('tab', { name: 'Providers' });
    expect(tab).toHaveAttribute('aria-selected', 'true');
    await screen.findByRole('table', { name: 'Providers' });
    expect(screen.queryByText('How should ruckcode authenticate?')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Add provider…' }));
    expect(added).toBe(true);
    await fireEvent.keyDown(tab, { key: 'ArrowRight' });
    expect(screen.getByRole('tab', { name: 'API keys' })).toHaveAttribute('aria-selected', 'true');
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(closed).toBe(true);
  });
});

describe('SetupWizardDialog', () => {
  it('shows the first-run welcome and walks key -> endpoint with validation and no suggestions', async () => {
    const api = new FakeSetup({ fresh: true, delayMs: 0 });
    render(SetupWizardDialog, {
      props: { api, target, firstRun: true, onclose: () => {}, onreconnect: () => {} }
    });
    await screen.findByRole('dialog', { name: 'Set up ruckcode' });
    await screen.findByText(/No model provider is configured/);
    const next = await screen.findByRole('button', { name: 'Next' });
    expect(next).toBeDisabled();
    expect(screen.queryByText(/environment variable/i)).toBeNull();
    // Pasting a key (masked) enables Next and hints the endpoint from the key shape.
    const secret = screen.getByLabelText('API key') as HTMLInputElement;
    expect(secret.type).toBe('password');
    await fireEvent.input(secret, { target: { value: 'sk-or-v1-abc' } });
    expect(next).toBeEnabled();
    await fireEvent.click(next);
    const url = await screen.findByLabelText('Endpoint URL');
    expect(url).toHaveValue(''); // never pre-filled, no suggestions
    expect(next).toBeDisabled();
    for (const label of ['Anthropic', 'OpenAI', 'OpenRouter', 'Ollama', 'LM Studio', 'vLLM'])
      expect(screen.queryByRole('button', { name: label })).toBeNull();
    await fireEvent.input(url, { target: { value: 'localhost:11434' } });
    expect(screen.getByTestId('endpoint-error')).toHaveTextContent(/http/);
    expect(next).toBeDisabled();
    await fireEvent.input(url, { target: { value: 'http://localhost:11434/v1' } });
    expect(screen.getByTestId('endpoint-error')).toHaveTextContent('');
    expect(next).toBeEnabled();
  });
});
