import type { LineTransport } from '../acp/transport';
import {
  connectTauri,
  inTauri,
  pickFolder,
  tauriPlatform,
  tauriSetup,
  tauriTranscripts,
  tauriWsl,
  type HostKind
} from './tauri';
import type { SetupApi } from './setup';
import type { TranscriptsApi } from './transcripts';
import type { WslApi } from './wsl';

export type { HostKind };

let mockStore: import('./fake-agent').FakeStore | undefined;
let mockWsl: WslApi | undefined;
let mockSetup: SetupApi | undefined;
let mockTranscripts: TranscriptsApi | undefined;

export type HostMode = 'tauri' | 'mock' | 'none';

export function hostMode(): HostMode {
  if (typeof window !== 'undefined' && new URLSearchParams(location.search).has('mock')) {
    return 'mock';
  }
  return inTauri() ? 'tauri' : 'none';
}

export interface ConnectOptions {
  cwd: string;
  host?: HostKind;
  rcPath?: string;
  /** WSL distro (host `wsl`). */
  distro?: string;
}

/** Open a transport to an agent for the current environment. */
export async function connectHost(opts: ConnectOptions): Promise<LineTransport> {
  const mode = hostMode();
  if (mode === 'mock') {
    // Dev/demo only: a scripted agent, split into its own chunk.
    const { FakeAgent, FakeStore } = await import('./fake-agent');
    const delay = Number(new URLSearchParams(location.search).get('delay') ?? 40);
    // One store per page so a "reconnect" finds the sessions again.
    mockStore ??= new FakeStore();
    // Let tests and demos see what the UI asked for.
    ((window as unknown as { __ruckcodeConnects?: ConnectOptions[] }).__ruckcodeConnects ??=
      []).push({ ...opts });
    return new FakeAgent({ delayMs: Number.isFinite(delay) ? delay : 40, store: mockStore });
  }
  if (mode === 'tauri') return connectTauri(opts);
  throw new Error('No agent host: run inside the ruckcode desktop app, or add ?mock=1 for a demo.');
}

/**
 * `windows`, `macos` or `linux`. In mock mode, `?platform=windows` pretends to be Windows so the
 * WSL2 UI can be exercised in a browser; otherwise the mock reports `linux`.
 */
export async function getPlatform(): Promise<string> {
  const mode = hostMode();
  if (mode === 'mock') return new URLSearchParams(location.search).get('platform') ?? 'linux';
  if (mode === 'tauri') return tauriPlatform().catch(() => 'linux');
  return 'linux';
}

/** The WSL2 API for this environment (a scripted fake in mock mode). */
export async function getWsl(): Promise<WslApi> {
  if (hostMode() === 'mock') {
    const { FakeWsl } = await import('./fake-wsl');
    const delay = Number(new URLSearchParams(location.search).get('delay') ?? 120);
    mockWsl ??= new FakeWsl({ delayMs: Number.isFinite(delay) ? Math.min(delay, 120) : 120 });
    return mockWsl;
  }
  return tauriWsl;
}

/** Native folder picker, or in mock mode a canned answer so Browse can be tested. */
export async function browseFolder(opts: {
  host: HostKind;
  distro?: string;
}): Promise<string | null> {
  if (hostMode() === 'mock') {
    return opts.host === 'wsl' && opts.distro
      ? `\\\\wsl.localhost\\${opts.distro}\\home\\dev\\picked`
      : '/demo/picked';
  }
  const start =
    opts.host === 'wsl' && opts.distro ? `\\\\wsl.localhost\\${opts.distro}\\` : undefined;
  return pickFolder(start);
}

/**
 * The setup/keys API (a fake with in-memory state in mock mode). `?setup=fresh` starts with no
 * profile and no modes.toml so the first-run wizard opens.
 */
export async function getSetup(): Promise<SetupApi> {
  if (hostMode() === 'mock') {
    const { FakeSetup } = await import('./fake-setup');
    const q = new URLSearchParams(location.search);
    const delay = Number(q.get('setupdelay') ?? 150);
    mockSetup ??= new FakeSetup({
      fresh: q.get('setup') === 'fresh',
      delayMs: Number.isFinite(delay) ? delay : 150
    });
    return mockSetup;
  }
  return tauriSetup;
}

/** The transcript browser API (seeded in-memory transcripts in mock mode). */
export async function getTranscripts(): Promise<TranscriptsApi> {
  if (hostMode() === 'mock') {
    const { FakeTranscripts } = await import('./fake-transcripts');
    const delay = Number(new URLSearchParams(location.search).get('transcriptdelay') ?? 60);
    mockTranscripts ??= new FakeTranscripts({ delayMs: Number.isFinite(delay) ? delay : 60 });
    return mockTranscripts;
  }
  return tauriTranscripts;
}
