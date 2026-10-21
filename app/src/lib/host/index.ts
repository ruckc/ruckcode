import type { LineTransport } from '../acp/transport';
import { connectTauri, inTauri, type HostKind } from './tauri';

let mockStore: import('./fake-agent').FakeStore | undefined;

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
    return new FakeAgent({ delayMs: Number.isFinite(delay) ? delay : 40, store: mockStore });
  }
  if (mode === 'tauri') return connectTauri(opts.cwd, opts.host, opts.rcPath);
  throw new Error('No agent host: run inside the ruckcode desktop app, or add ?mock=1 for a demo.');
}
