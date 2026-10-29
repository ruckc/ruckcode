import { Channel, invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type { LineTransport } from '../acp/transport';
import type { ModesApi } from './modes';
import type { SetupApi } from './setup';
import type { TranscriptsApi } from './transcripts';
import type { UpdateCheck, UpdatesApi } from './updates';
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

function withProgress(
  command: string,
  distro: string,
  onProgress: (line: string) => void
): Promise<RcStatus> {
  const onProgressChannel = new Channel<string>();
  onProgressChannel.onmessage = onProgress;
  return invoke<RcStatus>(command, { distro, onProgress: onProgressChannel });
}

export const tauriWsl: WslApi = {
  listDistros: () => invoke<WslDistro[]>('wsl_list_distros'),
  checkRc: (distro) => invoke<RcStatus>('wsl_check_rc', { distro }),
  installRc: (distro, onProgress) => withProgress('wsl_install_rc', distro, onProgress),
  installBundledRc: (distro, onProgress) =>
    withProgress('wsl_install_bundled_rc', distro, onProgress),
  ensureRc: (distro, onProgress) => withProgress('wsl_ensure_rc', distro, onProgress),
  translatePath: (distro, path) => invoke<string>('wsl_translate_path', { distro, path })
};

type UpdateDownloadEvent =
  | { event: 'started'; data: { contentLength: number | null } }
  | { event: 'progress'; data: { chunkLength: number } }
  | { event: 'finished' };

/** In-app updates through the Tauri backend (`update_*` commands; the plugin itself stays closed). */
export const tauriUpdates: UpdatesApi = {
  check: () => invoke<UpdateCheck>('update_check'),
  async download(onProgress) {
    const channel = new Channel<UpdateDownloadEvent>();
    let done = 0;
    let total: number | null = null;
    channel.onmessage = (ev) => {
      if (ev.event === 'started') total = ev.data.contentLength;
      else if (ev.event === 'progress') done += ev.data.chunkLength;
      onProgress(done, total);
    };
    await invoke('update_download', { onEvent: channel });
  },
  install: () => invoke('update_install'),
  restart: () => invoke('update_restart')
};

/** Setup and keys through the Tauri backend (local core, or `rc --json` inside WSL). */
export const tauriSetup: SetupApi = {
  presets: () => invoke('setup_presets'),
  state: (t) => invoke('setup_state', { project: t.project, distro: t.distro ?? null }),
  listModels: async (t, req) => {
    const r = await invoke<{ models: { id: string; displayName: string | null }[] }>(
      'list_models',
      {
        project: t.project,
        distro: t.distro ?? null,
        kind: req.kind,
        baseUrl: req.baseUrl,
        auth: req.auth ?? null
      }
    );
    return r.models;
  },
  probe: (t, req) =>
    invoke('setup_probe', {
      project: t.project,
      distro: t.distro ?? null,
      baseUrl: req.baseUrl,
      force: req.force ?? null,
      auth: req.auth ?? null
    }),
  preview: (t, plan) =>
    invoke('setup_preview', { project: t.project, distro: t.distro ?? null, plan }),
  apply: (t, plan) => invoke('setup_apply', { project: t.project, distro: t.distro ?? null, plan }),
  keysList: (t) => invoke('keys_list', { project: t.project, distro: t.distro ?? null }),
  keysAdd: async (t, name, secret, description) => {
    await invoke('keys_add', {
      project: t.project,
      distro: t.distro ?? null,
      name,
      secret,
      description: description ?? null
    });
  },
  keysRemove: (t, name) =>
    invoke('keys_remove', { project: t.project, distro: t.distro ?? null, name }),
  keysUse: async (t, name, profile, projectScope) => {
    await invoke('keys_use', {
      project: t.project,
      distro: t.distro ?? null,
      name,
      profile,
      projectScope
    });
  },
  initModes: (t, opts) =>
    invoke('init_modes', { project: t.project, distro: t.distro ?? null, opts: opts ?? null })
};

/** The mode editor through the Tauri backend (local core, or `rc modes --json` inside WSL). */
export const tauriModes: ModesApi = {
  get: (t) => invoke('modes_get', { project: t.project, distro: t.distro ?? null }),
  validate: (t, doc) =>
    invoke('modes_validate', { project: t.project, distro: t.distro ?? null, doc }),
  preview: (t, doc) =>
    invoke('modes_preview', { project: t.project, distro: t.distro ?? null, doc }),
  apply: (t, doc) => invoke('modes_apply', { project: t.project, distro: t.distro ?? null, doc }),
  initModes: (t, opts) =>
    invoke('init_modes', { project: t.project, distro: t.distro ?? null, opts: opts ?? null })
};

/** Transcripts through the Tauri backend (local core, or `rc sessions --json` inside WSL). */
export const tauriTranscripts: TranscriptsApi = {
  list: (t, query) =>
    invoke('transcripts_list', {
      project: t.project,
      distro: t.distro ?? null,
      query: query ?? null
    }),
  read: (t, id, opts) =>
    invoke('transcripts_read', {
      project: t.project,
      distro: t.distro ?? null,
      id,
      offset: opts?.offset ?? null,
      limit: opts?.limit ?? null
    }),
  search: (t, text, limit) =>
    invoke('transcripts_search', {
      project: t.project,
      distro: t.distro ?? null,
      text,
      limit: limit ?? null
    }),
  unfinished: (t) =>
    invoke('transcripts_unfinished', { project: t.project, distro: t.distro ?? null }),
  dismiss: async (t, id) => {
    await invoke('transcripts_dismiss', { project: t.project, distro: t.distro ?? null, id });
  }
};
