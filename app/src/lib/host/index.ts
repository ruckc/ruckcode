import type { LineTransport } from '../acp/transport';
import { connectTauri, inTauri, type HostKind } from './tauri';

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
    const { FakeAgent } = await import('./fake-agent');
    return new FakeAgent({ delayMs: 120 });
  }
  if (mode === 'tauri') return connectTauri(opts.cwd, opts.host, opts.rcPath);
  throw new Error('No agent host: run inside the ruckcode desktop app, or add ?mock=1 for a demo.');
}
