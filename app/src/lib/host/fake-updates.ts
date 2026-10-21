import type { UpdateCheck, UpdatesApi } from './updates';

export type UpdateScenario =
  /** A newer version that installs itself. */
  | 'available'
  /** A newer version that must be downloaded by hand (deb/msi install). */
  | 'manual'
  | 'unconfigured'
  | 'uptodate'
  /** The update site cannot be reached. */
  | 'error'
  /** The download fails. */
  | 'faildownload';

export interface FakeUpdatesOptions {
  scenario?: UpdateScenario;
  /** Delay between download progress steps (ms). */
  delayMs?: number;
  version?: string;
}

/**
 * Scripted updater for browser mock mode (`?mock=1&update=1`; `update=manual|unconfigured|
 * uptodate|error|faildownload` pick the other scenarios, `updatedelay=<ms>` the progress speed).
 * `calls` and `window.__ruckcodeUpdateCalls` record what the UI asked for.
 */
export class FakeUpdates implements UpdatesApi {
  readonly calls: string[] = [];
  private scenario: UpdateScenario;
  private delayMs: number;
  private version: string;

  constructor(opts: FakeUpdatesOptions = {}) {
    this.scenario = opts.scenario ?? 'available';
    this.delayMs = opts.delayMs ?? 30;
    this.version = opts.version ?? '9.9.9';
  }

  private record(call: string) {
    this.calls.push(call);
    if (typeof window !== 'undefined') {
      ((window as unknown as { __ruckcodeUpdateCalls?: string[] }).__ruckcodeUpdateCalls ??=
        []).push(call);
    }
  }

  async check(): Promise<UpdateCheck> {
    this.record('check');
    await new Promise((r) => setTimeout(r, this.delayMs));
    switch (this.scenario) {
      case 'unconfigured':
        return { state: 'notConfigured', reason: 'This build has no update signing key.' };
      case 'uptodate':
        return { state: 'upToDate', version: '0.1.0' };
      case 'error':
        throw new Error('Could not fetch a valid release JSON from the remote');
      default:
        return {
          state: 'available',
          version: this.version,
          current: '0.1.0',
          notes: '### Features\n\n- Faster startup\n- Update notes in the app',
          date: '2026-01-02T03:04:05Z',
          manualReason:
            this.scenario === 'manual'
              ? 'This copy was installed from a Linux package, which cannot update itself.'
              : null,
          downloadPage: 'https://ruckc.github.io/ruckcode/'
        };
    }
  }

  async download(onProgress: (downloaded: number, total: number | null) => void): Promise<void> {
    this.record('download');
    const total = 4000;
    for (let done = 0; done < total;) {
      await new Promise((r) => setTimeout(r, this.delayMs));
      done = Math.min(total, done + 1000);
      onProgress(done, total);
      if (this.scenario === 'faildownload' && done >= 2000) {
        throw new Error('signature verification failed');
      }
    }
  }

  async install(): Promise<void> {
    this.record('install');
  }

  async restart(): Promise<void> {
    this.record('restart');
  }
}
