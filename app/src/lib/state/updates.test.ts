import { afterEach, describe, expect, it, vi } from 'vitest';
import { FakeUpdates, type FakeUpdatesOptions } from '../host/fake-updates';
import { CHECK_INTERVAL_MS, UpdateController } from './updates.svelte';

function memory(initial: Record<string, string> = {}) {
  const data = { ...initial };
  return {
    data,
    getItem: (k: string) => data[k] ?? null,
    setItem: (k: string, v: string) => void (data[k] = v)
  };
}

function make(
  opts: FakeUpdatesOptions = {},
  extra: { busy?: () => boolean; storage?: object } = {}
) {
  const api = new FakeUpdates({ delayMs: 0, ...opts });
  const storage = (extra.storage ?? memory()) as ReturnType<typeof memory>;
  const u = new UpdateController(api, { isBusy: extra.busy, storage });
  return { api, u, storage };
}

afterEach(() => {
  vi.useRealTimers();
});

describe('UpdateController', () => {
  it('downloads in the background and then waits for a restart', async () => {
    const { u, api } = make({ delayMs: 60 });
    const p = u.check();
    await vi.waitFor(() => expect(u.phase).toBe('downloading'));
    await vi.waitFor(() => expect(u.progress).toBeGreaterThan(0));
    expect(u.progress).toBeLessThan(1);
    await p;
    expect(u.phase).toBe('ready');
    expect(u.version).toBe('9.9.9');
    expect(u.notes).toContain('Faster startup');
    expect(u.progress).toBe(1);
    expect(api.calls).toEqual(['check', 'download']);
    expect(u.visible).toBe(true);
  });

  it('with automatic downloads off, asks first', async () => {
    const { u, api, storage } = make({}, { storage: memory({ 'ruckcode.autoUpdate': '0' }) });
    expect(u.auto).toBe(false);
    await u.check();
    expect(u.phase).toBe('available');
    expect(api.calls).toEqual(['check']);
    await u.download();
    expect(u.phase).toBe('ready');
    u.setAuto(true);
    expect(storage.data['ruckcode.autoUpdate']).toBe('1');
  });

  it('turning automatic downloads on picks up a pending update', async () => {
    const { u, api } = make({}, { storage: memory({ 'ruckcode.autoUpdate': '0' }) });
    await u.check();
    u.setAuto(true);
    await vi.waitFor(() => expect(u.phase).toBe('ready'));
    expect(api.calls).toContain('download');
  });

  it('never downloads an update this install cannot apply', async () => {
    const { u, api } = make({ scenario: 'manual' });
    await u.check();
    expect(u.phase).toBe('manual');
    expect(u.reason).toContain('Linux package');
    expect(u.downloadPage).toBe('https://ruckc.github.io/ruckcode/');
    expect(api.calls).toEqual(['check']);
  });

  it('reports unconfigured and up-to-date quietly', async () => {
    const a = make({ scenario: 'unconfigured' });
    await a.u.check();
    expect(a.u.phase).toBe('unconfigured');
    expect(a.u.visible).toBe(false);
    const b = make({ scenario: 'uptodate' });
    await b.u.check();
    expect(b.u.phase).toBe('uptodate');
    expect(b.u.visible).toBe(false);
  });

  it('a failed background check is quiet, a manual one shows the failure', async () => {
    const { u } = make({ scenario: 'error' });
    await u.check();
    expect(u.phase).toBe('idle');
    expect(u.error).toContain('release JSON');
    expect(u.visible).toBe(false);
    await u.check(true);
    expect(u.phase).toBe('error');
    expect(u.visible).toBe(false); // no version known: only Settings shows it
  });

  it('a failed download is visible and can be retried', async () => {
    const { u } = make({ scenario: 'faildownload' });
    await u.check();
    expect(u.phase).toBe('error');
    expect(u.error).toContain('signature');
    expect(u.visible).toBe(true);
  });

  it('refuses to restart while an agent turn runs, unless forced', async () => {
    let busy = true;
    const { u, api } = make({}, { busy: () => busy });
    await u.check();
    expect(await u.restartNow()).toBe(false);
    expect(api.calls).not.toContain('install');
    expect(u.phase).toBe('ready');
    busy = false;
    expect(await u.restartNow()).toBe(true);
    expect(api.calls.slice(-2)).toEqual(['install', 'restart']);
    // Forced restart goes through even when busy.
    const f = make({}, { busy: () => true });
    await f.u.check();
    expect(await f.u.restartNow(true)).toBe(true);
  });

  it('does nothing to restart before an update is downloaded', async () => {
    const { u, api } = make({ scenario: 'manual' });
    await u.check();
    expect(await u.restartNow(true)).toBe(false);
    expect(api.calls).toEqual(['check']);
  });

  it('"Later" hides the banner until a manual check', async () => {
    const { u } = make();
    await u.check();
    u.later();
    expect(u.visible).toBe(false);
    expect(u.phase).toBe('ready');
    await u.check(); // a periodic check leaves a staged update alone
    expect(u.visible).toBe(false);
  });

  it('checks every six hours, but leaves a downloaded update alone', async () => {
    vi.useFakeTimers();
    const { u, api } = make({ scenario: 'uptodate' });
    u.start();
    await vi.advanceTimersByTimeAsync(1);
    expect(api.calls).toEqual(['check']);
    await vi.advanceTimersByTimeAsync(CHECK_INTERVAL_MS);
    expect(api.calls).toEqual(['check', 'check']);
    u.stop();
    await vi.advanceTimersByTimeAsync(CHECK_INTERVAL_MS);
    expect(api.calls).toHaveLength(2);
  });

  it('survives blocked storage', async () => {
    const throwing = {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      }
    };
    const { u } = make({}, { storage: throwing });
    expect(u.auto).toBe(true);
    expect(() => u.setAuto(false)).not.toThrow();
    expect(u.auto).toBe(false);
  });
});
