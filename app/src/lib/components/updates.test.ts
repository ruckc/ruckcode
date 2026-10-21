// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import { FakeUpdates, type FakeUpdatesOptions } from '../host/fake-updates';
import { UpdateController } from '../state/updates.svelte';
import SettingsDialog from './SettingsDialog.svelte';
import UpdateBanner from './UpdateBanner.svelte';
import { FakeSetup } from '../host/fake-setup';

function controller(opts: FakeUpdatesOptions = {}, busy = () => false) {
  const api = new FakeUpdates({ delayMs: 0, ...opts });
  const u = new UpdateController(api, { isBusy: busy, storage: null });
  return { api, u };
}

describe('UpdateBanner', () => {
  it('shows nothing before a check finds something', () => {
    const { u } = controller();
    render(UpdateBanner, { props: { updates: u } });
    expect(screen.queryByTestId('update-banner')).toBeNull();
  });

  it('announces a ready update with release notes, and restarts on request', async () => {
    const { u, api } = controller();
    render(UpdateBanner, { props: { updates: u, openSessions: 2 } });
    await u.check();
    expect(await screen.findByText('Update v9.9.9 ready.')).toBeInTheDocument();
    expect(screen.getByText(/2 open sessions end/)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Release notes' }));
    expect(await screen.findByText('Faster startup')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Restart now' }));
    await waitFor(() => expect(api.calls.slice(-2)).toEqual(['install', 'restart']));
  });

  it('"Later" hides the banner', async () => {
    const { u } = controller();
    render(UpdateBanner, { props: { updates: u } });
    await u.check();
    await fireEvent.click(await screen.findByRole('button', { name: 'Later' }));
    await waitFor(() => expect(screen.queryByTestId('update-banner')).toBeNull());
  });

  it('shows a subtle progress line while downloading', async () => {
    const { u } = controller({ delayMs: 20 });
    render(UpdateBanner, { props: { updates: u } });
    const p = u.check();
    const line = await screen.findByTestId('update-downloading');
    expect(line).toHaveTextContent(/Downloading update v9\.9\.9/);
    expect(screen.queryByRole('button', { name: 'Restart now' })).toBeNull();
    await p;
    expect(await screen.findByRole('button', { name: 'Restart now' })).toBeInTheDocument();
  });

  it('holds the restart while an agent turn is running', async () => {
    const { u, api } = controller({}, () => true);
    render(UpdateBanner, { props: { updates: u } });
    await u.check();
    expect(await screen.findByText(/would interrupt it/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Restart now' })).toBeNull();
    // Forcing needs a second, explicit click.
    await fireEvent.click(screen.getByRole('button', { name: 'Restart anyway…' }));
    expect(api.calls).not.toContain('install');
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.getByRole('button', { name: 'Restart anyway…' })).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Restart anyway…' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Interrupt the agent and restart' }));
    await waitFor(() => expect(api.calls.slice(-2)).toEqual(['install', 'restart']));
  });

  it('points to the download page when the install cannot update itself', async () => {
    const { u } = controller({ scenario: 'manual' });
    render(UpdateBanner, { props: { updates: u } });
    await u.check();
    expect(await screen.findByText(/Linux package/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'https://ruckc.github.io/ruckcode/' })).toHaveAttribute(
      'href',
      'https://ruckc.github.io/ruckcode/'
    );
    expect(screen.queryByRole('button', { name: 'Restart now' })).toBeNull();
  });

  it('offers a download when automatic downloads are off', async () => {
    const api = new FakeUpdates({ delayMs: 0 });
    const u = new UpdateController(api, {
      storage: { getItem: () => '0', setItem: () => {} }
    });
    render(UpdateBanner, { props: { updates: u } });
    await u.check();
    await fireEvent.click(await screen.findByRole('button', { name: 'Download' }));
    expect(await screen.findByRole('button', { name: 'Restart now' })).toBeInTheDocument();
  });

  it('mentions WSL on Windows and offers a retry after a failed download', async () => {
    const win = controller();
    render(UpdateBanner, { props: { updates: win.u, platform: 'windows' } });
    await win.u.check();
    expect(await screen.findByText(/WSL project offers to update rc/)).toBeInTheDocument();
  });

  it('shows a failed download with Retry', async () => {
    const { u } = controller({ scenario: 'faildownload' });
    render(UpdateBanner, { props: { updates: u } });
    await u.check();
    expect(await screen.findByText(/signature verification failed/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument();
  });
});

describe('Updates settings tab', () => {
  it('toggles automatic downloads and checks on demand', async () => {
    window.history.replaceState({}, '', '/?mock=1');
    const { u, api } = controller({ scenario: 'uptodate' });
    render(SettingsDialog, {
      props: {
        api: new FakeSetup({ delayMs: 0 }),
        target: { project: '/demo/project' },
        tab: 'updates',
        updates: u,
        onclose: () => {},
        onaddprovider: () => {}
      }
    });
    const box = await screen.findByRole('checkbox', { name: 'Automatically download updates' });
    expect(box).toBeChecked();
    await fireEvent.click(box);
    expect(u.auto).toBe(false);
    await fireEvent.click(screen.getByRole('button', { name: 'Check for updates' }));
    await waitFor(() =>
      expect(screen.getByTestId('update-status')).toHaveTextContent('up to date')
    );
    expect(api.calls).toEqual(['check']);
  });

  it('has no Updates tab without an updater', async () => {
    window.history.replaceState({}, '', '/?mock=1');
    render(SettingsDialog, {
      props: {
        api: new FakeSetup({ delayMs: 0 }),
        target: { project: '/demo/project' },
        onclose: () => {},
        onaddprovider: () => {}
      }
    });
    await screen.findByRole('tab', { name: 'Project' });
    expect(screen.queryByRole('tab', { name: 'Updates' })).toBeNull();
  });
});
