import { Channel, invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type { LineTransport } from '../acp/transport';

/** True when running inside the Tauri webview (as opposed to a plain browser). */
export function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export type HostKind = 'local' | 'subprocess';

/**
 * Connect to an agent hosted by the Tauri backend. Lines from the agent arrive on a Tauri
 * `Channel`; lines to the agent go through the `agent_send` command.
 */
export async function connectTauri(
  cwd: string,
  host: HostKind = 'local',
  rcPath?: string
): Promise<LineTransport> {
  const listeners = new Set<(line: string) => void>();
  const closers = new Set<() => void>();
  const channel = new Channel<string>();
  channel.onmessage = (line) => listeners.forEach((l) => l(line));
  const id = await invoke<number>('agent_connect', {
    cwd,
    host,
    rcPath: rcPath ?? null,
    onLine: channel
  });
  return {
    send(line) {
      // A failed send means the agent is gone (exited or disconnected): surface it as a close.
      invoke('agent_send', { id, line }).catch((e) => {
        console.error('agent_send failed', e);
        closers.forEach((c) => c());
      });
    },
    onLine(cb) {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    onClose(cb) {
      closers.add(cb);
      return () => closers.delete(cb);
    },
    async close() {
      listeners.clear();
      closers.clear();
      await invoke('agent_disconnect', { id });
    }
  };
}

/** Native folder picker; returns the chosen directory or null. */
export async function pickFolder(): Promise<string | null> {
  const dir = await open({ directory: true, multiple: false });
  return typeof dir === 'string' ? dir : null;
}
