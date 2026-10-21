import { Channel, invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type { LineTransport } from '../acp/transport';
import type { RcStatus, WslApi, WslDistro } from './wsl';

/** True when running inside the Tauri webview (as opposed to a plain browser). */
export function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export type HostKind = 'local' | 'subprocess' | 'wsl';

/** What the Rust shell sends on the agent channel: every ACP line, then one final `exit`. */
export type AgentEvent =
  { event: 'line'; line: string } | { event: 'exit'; code: number | null; message: string };

export interface TauriConnect {
  cwd: string;
  host?: HostKind;
  rcPath?: string;
  distro?: string;
}

/**
 * Connect to an agent hosted by the Tauri backend. Events from the agent arrive on a Tauri
 * `Channel` (ordered, so the final `exit` always follows the last line); lines to the agent go
 * through the `agent_send` command.
 */
export async function connectTauri(opts: TauriConnect): Promise<LineTransport> {
  const listeners = new Set<(line: string) => void>();
  const closers = new Set<(reason?: string) => void>();
  let ended = false;
  const transport: LineTransport = {
    send(line) {
      // A failed send means the agent is gone (exited or disconnected): surface it as a close.
      invoke('agent_send', { id, line }).catch((e) => {
        console.error('agent_send failed', e);
        end('The connection to the agent was lost.');
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
      ended = true;
      listeners.clear();
      closers.clear();
      await invoke('agent_disconnect', { id });
    }
  };
  function end(reason: string) {
    if (ended) return;
    ended = true;
    transport.closeReason = reason;
    closers.forEach((c) => c(reason));
  }
  const channel = new Channel<AgentEvent>();
  channel.onmessage = (ev) => {
    if (ev.event === 'line') listeners.forEach((l) => l(ev.line));
    else end(ev.message);
  };
  const id = await invoke<number>('agent_connect', {
    cwd: opts.cwd,
    host: opts.host ?? 'local',
    rcPath: opts.rcPath ?? null,
    distro: opts.distro ?? null,
    onEvent: channel
  });
  return transport;
}

/** Native folder picker; returns the chosen directory or null. */
export async function pickFolder(defaultPath?: string): Promise<string | null> {
  const dir = await open({ directory: true, multiple: false, defaultPath });
  return typeof dir === 'string' ? dir : null;
}

export async function tauriPlatform(): Promise<string> {
  return invoke<string>('platform');
}

export const tauriWsl: WslApi = {
  listDistros: () => invoke<WslDistro[]>('wsl_list_distros'),
  checkRc: (distro) => invoke<RcStatus>('wsl_check_rc', { distro }),
  installRc(distro, onProgress) {
    const onProgressChannel = new Channel<string>();
    onProgressChannel.onmessage = onProgress;
    return invoke<RcStatus>('wsl_install_rc', { distro, onProgress: onProgressChannel });
  },
  translatePath: (distro, path) => invoke<string>('wsl_translate_path', { distro, path })
};
