// @vitest-environment jsdom
import { fireEvent, render, screen } from '@testing-library/svelte';
import type { ComponentProps } from 'svelte';
import { beforeEach, describe, expect, it } from 'vitest';
import HostPicker from './HostPicker.svelte';

const props = (
  over: Partial<ComponentProps<typeof HostPicker>> = {}
): ComponentProps<typeof HostPicker> => ({
  host: 'wsl',
  distro: '',
  rcPath: '',
  cwd: '',
  platform: 'windows',
  ...over
});

beforeEach(() => {
  // Browser mock mode: the scripted FakeWsl answers instead of Tauri.
  window.history.replaceState({}, '', '/?mock=1&delay=0');
});

describe('HostPicker', () => {
  it('hides WSL2 outside Windows but keeps Local and Custom rc', () => {
    render(HostPicker, props({ host: 'local', platform: 'linux' }));
    const radios = screen.getAllByRole('radio').map((r) => r.textContent?.trim());
    expect(radios).toEqual(['Local', 'Custom rc']);
  });

  it('offers WSL2 on Windows and lists distros, only WSL2 ones selectable', async () => {
    render(HostPicker, props());
    expect(screen.getByRole('radio', { name: 'WSL2' })).toHaveAttribute('aria-checked', 'true');
    const select = await screen.findByRole('combobox', { name: 'WSL distro' });
    await screen.findByRole('option', { name: /Ubuntu-24.04 \(default, Running, WSL2\)/ });
    expect((select as HTMLSelectElement).value).toBe('Ubuntu-24.04'); // the default distro
    expect(screen.getByRole('option', { name: /Alpine.*WSL1/ })).toBeDisabled();
    expect(screen.getByRole('option', { name: /docker-desktop/ })).toBeDisabled();
    expect(screen.getByRole('option', { name: /Debian/ })).not.toBeDisabled();
  });

  it('shows rc as missing, installs it with progress and then shows the version', async () => {
    render(HostPicker, props());
    const status = await screen.findByRole('status', { name: 'rc status' });
    await screen.findByText(/rc is not installed in Ubuntu-24.04/);
    expect(status).toHaveAttribute('data-state', 'missing');
    await fireEvent.click(screen.getByRole('button', { name: 'Install rc' }));
    await screen.findByText(/rc 0.1.0 installed/);
    expect(screen.getByLabelText('Install progress').textContent).toContain('verifying checksum');
    expect(screen.queryByRole('button', { name: 'Install rc' })).toBeNull();
    expect(status).toHaveAttribute('data-state', 'ok');
  });

  it('warns about a version mismatch and offers an update', async () => {
    render(HostPicker, props({ distro: 'Debian' }));
    await screen.findByText(/rc 0.0.9 is installed, but this app is 0.1.0/);
    expect(screen.getByRole('status', { name: 'rc status' })).toHaveAttribute(
      'data-state',
      'mismatch'
    );
    await fireEvent.click(screen.getByRole('button', { name: 'Update rc' }));
    await screen.findByText(/rc 0.1.0 installed/);
  });

  it('shows the translated Linux path, or why the path cannot be used', async () => {
    const { rerender } = render(HostPicker, props({ cwd: 'C:\\Users\\me\\proj' }));
    const out = await screen.findByLabelText('Linux path');
    await screen.findByText('/mnt/c/Users/me/proj');
    expect(out).toHaveAttribute('data-ok', 'true');
    await rerender(
      props({ distro: 'Ubuntu-24.04', cwd: '\\\\wsl.localhost\\Ubuntu-24.04\\home\\me\\p' })
    );
    await screen.findByText('/home/me/p');
    await rerender(props({ distro: 'Ubuntu-24.04', cwd: '\\\\wsl$\\Debian\\home\\me' }));
    await screen.findByText(/belongs to distro `Debian`/);
    expect(screen.getByLabelText('Linux path')).toHaveAttribute('data-ok', 'false');
  });

  it('asks for a binary path for the custom rc host', async () => {
    render(HostPicker, props({ host: 'subprocess' }));
    expect(screen.getByPlaceholderText('/path/to/rc')).toBeInTheDocument();
    expect(screen.queryByRole('combobox')).toBeNull();
  });
});
