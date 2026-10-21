import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentEvent } from './tauri';

// A stand-in for the Tauri IPC layer: records invokes and exposes the agent channel.
const ipc = vi.hoisted(() => ({
  calls: [] as { cmd: string; args: Record<string, unknown> }[],
  channel: undefined as { onmessage: (e: unknown) => void } | undefined,
  failSend: false,
  respond: undefined as ((m: { id: number; method: string }) => void) | undefined
}));

vi.mock('@tauri-apps/api/core', () => ({
  Channel: class {
    onmessage: (e: unknown) => void = () => {};
  },
  invoke: vi.fn(async (cmd: string, args: Record<string, unknown> = {}) => {
    ipc.calls.push({ cmd, args });
    if (cmd === 'agent_connect') {
      ipc.channel = args.onEvent as typeof ipc.channel;
      return 7;
    }
    if (cmd === 'agent_send' && ipc.failSend) throw new Error('no such connection: 7');
    if (cmd === 'agent_send') {
      const m = JSON.parse(args.line as string);
      if (m.id !== undefined && m.method) ipc.respond?.(m);
    }
    return undefined;
  })
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

import { connectTauri } from './tauri';
import { ChatController } from '../state/chat.svelte';

const emit = (e: AgentEvent) => ipc.channel!.onmessage(e);

beforeEach(() => {
  ipc.calls = [];
  ipc.channel = undefined;
  ipc.failSend = false;
  ipc.respond = undefined;
});

describe('connectTauri', () => {
  it('passes the host, distro and cwd to agent_connect', async () => {
    await connectTauri({ cwd: 'C:\\x', host: 'wsl', distro: 'Ubuntu' });
    expect(ipc.calls[0]).toMatchObject({
      cmd: 'agent_connect',
      args: { cwd: 'C:\\x', host: 'wsl', distro: 'Ubuntu', rcPath: null }
    });
  });

  it('delivers lines, then closes with the exit message after the last line', async () => {
    const t = await connectTauri({ cwd: '/p' });
    const seen: string[] = [];
    t.onLine((l) => seen.push(`line:${l}`));
    t.onClose?.((reason) => seen.push(`close:${reason}`));
    emit({ event: 'line', line: '{"a":1}' });
    emit({ event: 'line', line: '{"a":2}' });
    emit({ event: 'exit', code: 3, message: '`rc acp` exited with code 3: boom' });
    emit({ event: 'line', line: 'ignored?' });
    expect(seen.slice(0, 3)).toEqual([
      'line:{"a":1}',
      'line:{"a":2}',
      'close:`rc acp` exited with code 3: boom'
    ]);
    expect(t.closeReason).toBe('`rc acp` exited with code 3: boom');
  });

  it('closes only once, and not at all after an explicit close()', async () => {
    const t = await connectTauri({ cwd: '/p' });
    const onClose = vi.fn();
    t.onClose?.(onClose);
    emit({ event: 'exit', code: 0, message: 'a' });
    emit({ event: 'exit', code: 0, message: 'b' });
    expect(onClose).toHaveBeenCalledTimes(1);

    const t2 = await connectTauri({ cwd: '/p' });
    const onClose2 = vi.fn();
    t2.onClose?.(onClose2);
    await t2.close?.();
    emit({ event: 'exit', code: 0, message: 'late' });
    expect(onClose2).not.toHaveBeenCalled();
    expect(ipc.calls.some((c) => c.cmd === 'agent_disconnect')).toBe(true);
  });

  it('treats a failed agent_send as a closed connection', async () => {
    const t = await connectTauri({ cwd: '/p' });
    const onClose = vi.fn();
    t.onClose?.(onClose);
    ipc.failSend = true;
    vi.spyOn(console, 'error').mockImplementation(() => {});
    t.send('{}');
    await vi.waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it('shows the lost-connection state with the exit message immediately', async () => {
    const chat = new ChatController({
      transport: () => connectTauri({ cwd: '/p', host: 'wsl', distro: 'U' })
    });
    // a tiny agent: answer every request the UI sends
    ipc.respond = (m) => {
      const results: Record<string, unknown> = {
        initialize: {
          protocolVersion: 1,
          agentCapabilities: {},
          agentInfo: { name: 'rc', version: '0' }
        },
        'session/list': { sessions: [] },
        'session/new': { sessionId: 's1' }
      };
      queueMicrotask(() =>
        emit({
          event: 'line',
          line: JSON.stringify({
            jsonrpc: '2.0',
            id: m.id,
            result: results[m.method as string] ?? {}
          })
        })
      );
    };
    await chat.connect({ cwd: '/p' });
    expect(chat.status).toBe('connected');
    emit({ event: 'exit', code: 137, message: '`rc acp (U)` exited with code 137: Killed' });
    await vi.waitFor(() => expect(chat.status).toBe('lost'));
    expect(chat.error).toContain('exited with code 137: Killed');
  });
});
