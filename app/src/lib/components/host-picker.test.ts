// @vitest-environment jsdom
import { render, screen, waitFor } from '@testing-library/svelte';
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
    await screen.findByRole('option', { name: 'Ubuntu-24.04' });
    // no default/running/stopped noise in the list
    expect(screen.getByRole('option', { name: 'Debian' })).toBeInTheDocument();
    expect((select as HTMLSelectElement).value).toBe('Ubuntu-24.04'); // the default distro
    expect(screen.getByRole('option', { name: /Alpine.*WSL1/ })).toBeDisabled();
    expect(screen.getByRole('option', { name: /docker-desktop/ })).toBeDisabled();
    expect(screen.getByRole('option', { name: /Debian/ })).not.toBeDisabled();
  });

  it('installs the bundled rc automatically and then shows nothing', async () => {
    const { container } = render(HostPicker, props());
    await waitFor(() => expect(container.querySelector('[data-rc="ok"]')).not.toBeNull());
    expect(screen.queryByRole('status', { name: 'rc status' })).toBeNull();
    expect(screen.queryByLabelText('Install progress')).toBeNull();
    expect(screen.queryByRole('button', { name: /Install rc|Reinstall rc/ })).toBeNull();
  });

  it('updates an older rc automatically', async () => {
    const { container } = render(HostPicker, props({ distro: 'Debian' }));
    await waitFor(() => expect(container.querySelector('[data-rc="ok"]')).not.toBeNull());
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
