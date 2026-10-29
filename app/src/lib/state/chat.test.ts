import { describe, expect, it, vi } from 'vitest';
import { FakeAgent, FakeStore } from '../host/fake-agent';
import { ChatController } from './chat.svelte';

function setup() {
  const store = new FakeStore();
  const agents: FakeAgent[] = [];
  const chat = new ChatController({
    transport: () => {
      const a = new FakeAgent({ store });
      agents.push(a);
      return a;
    }
  });
  return { chat, agents, store };
}

async function ready() {
  const t = setup();
  await t.chat.connect({ cwd: '/proj' });
  return t;
}

const kinds = (c: ReturnType<typeof setup>['chat']) => c.active!.model.items.map((i) => i.kind);

describe('ChatController', () => {
  it('connects, opens a session and lists stored ones', async () => {
    const { chat } = await ready();
    expect(chat.status).toBe('connected');
    expect(chat.agentName).toBe('fake-agent');
    expect(chat.active?.id).toBe('sess-1');
    expect(chat.active?.model.modes.map((m) => m.id)).toEqual(['explore', 'plan', 'implement']);
    expect(chat.active?.model.config.model).toBe('mock-large');
    expect(chat.stored.map((s) => s.id)).toEqual(['old-1', 'old-2']);
    expect(chat.stored[0]).toMatchObject({ title: 'Fix the login bug', mode: 'implement' });
    // fresh session shows first, then stored ones
    expect(chat.sidebar.map((s) => s.id)).toEqual(['sess-1', 'old-1', 'old-2']);
  });

  it('runs a turn and records user text, reply, notice and the stop reason', async () => {
    const { chat } = await ready();
    await chat.send('hello');
    expect(kinds(chat)).toEqual(['user', 'thought', 'assistant', 'notice', 'turn_end']);
    const items = chat.active!.model.items;
    expect(items[0]).toMatchObject({ kind: 'user', text: 'hello' });
    expect(items.at(-1)).toMatchObject({ kind: 'turn_end', stopReason: 'end_turn' });
    expect(chat.active!.running).toBe(false);
    expect(chat.active!.model.plan).toHaveLength(3);
    expect(chat.active!.model.usage).toEqual({ used: 120, size: 200000 });
    // the first prompt names the session in the stored list
    await vi.waitFor(() => expect(chat.stored[0]).toMatchObject({ id: 'sess-1', title: 'hello' }));
  });

  it('ignores empty prompts and records history without duplicates', async () => {
    const { chat } = await ready();
    await chat.send('   ');
    await chat.send('a');
    await chat.send('b');
    await chat.send('a');
    expect(chat.history).toEqual(['b', 'a']);
    expect(chat.active!.model.items.filter((i) => i.kind === 'user')).toHaveLength(3);
  });

  it('queues prompts typed during a turn and sends them in order afterwards', async () => {
    const { chat } = await ready();
    const first = chat.send('mock:ask');
    await vi.waitFor(() => expect(chat.question).not.toBeNull());
    expect(chat.active!.running).toBe(true);
    await chat.send('second');
    await chat.send('third');
    expect(chat.active!.queue).toEqual(['second', 'third']);
    chat.unqueue(1);
    expect(chat.active!.queue).toEqual(['second']);
    chat.question!.resolve({
      cancelled: false,
      answers: [{ question: 'Which database?', selected: ['sqlite'], other: null }]
    });
    await first;
    await vi.waitFor(() => expect(chat.active!.running).toBe(false));
    const users = chat.active!.model.items.filter((i) => i.kind === 'user').map((i) => i.text);
    expect(users).toEqual(['mock:ask', 'second']);
    expect(chat.active!.queue).toEqual([]);
  });

  it('shows permission requests one at a time and sends the choice', async () => {
    const { chat, agents } = await ready();
    const turn = chat.send('mock:write a.txt hi');
    await vi.waitFor(() => expect(chat.permission).not.toBeNull());
    const req = chat.permission!.request;
    expect(req.toolCall.kind).toBe('edit');
    expect(req.options.map((o) => o.kind)).toEqual(['allow_once', 'allow_always', 'reject_once']);
    expect(req.toolCall.content?.[0]).toMatchObject({ type: 'diff', path: 'a.txt' });
    chat.permission!.resolve({ outcome: { outcome: 'selected', optionId: 'allow' } });
    await turn;
    expect(chat.permission).toBeNull();
    const tool = chat.active!.model.items.find((i) => i.kind === 'tool');
    expect(tool).toMatchObject({ status: 'completed', diffs: [{ path: 'a.txt' }] });
    expect(agents[0].received.some((m) => m.method === 'session/prompt')).toBe(true);
  });

  it('queues several permission requests (one per session) and cancel clears them', async () => {
    const { chat } = await ready();
    const t1 = chat.send('mock:write a.txt 1');
    await vi.waitFor(() => expect(chat.permissions).toHaveLength(1));
    await chat.newSession();
    const t2 = chat.send('mock:write b.txt 2');
    await vi.waitFor(() => expect(chat.permissions).toHaveLength(2));
    expect(chat.permission!.request.toolCall.title).toBe('Write a.txt');
    chat.permission!.resolve({ outcome: { outcome: 'selected', optionId: 'reject' } });
    expect(chat.permission!.request.toolCall.title).toBe('Write b.txt');
    await chat.cancel();
    await Promise.all([t1, t2]);
    expect(chat.permissions).toEqual([]);
    expect(chat.active!.model.items.at(-1)).toMatchObject({
      kind: 'turn_end',
      stopReason: 'cancelled'
    });
  });

  it('cancel ends a turn blocked on a pending question', async () => {
    const { chat, agents } = await ready();
    const turn = chat.send('mock:ask');
    await vi.waitFor(() => expect(chat.question).not.toBeNull());
    await chat.cancel();
    await turn;
    expect(chat.question).toBeNull();
    expect(chat.active!.running).toBe(false);
    expect(agents[0].received.some((m) => m.method === 'session/cancel')).toBe(true);
  });

  it('applies a SwitchMode proposal on approval', async () => {
    const { chat } = await ready();
    const turn = chat.send('mock:mode explore');
    await vi.waitFor(() => expect(chat.permission).not.toBeNull());
    expect(chat.permission!.request.toolCall).toMatchObject({
      kind: 'switch_mode',
      rawInput: { mode_id: 'explore' }
    });
    chat.permission!.resolve({ outcome: { outcome: 'selected', optionId: 'allow' } });
    await turn;
    expect(chat.active!.model.currentModeId).toBe('explore');
  });

  it('switches modes and the model, and reflects agent updates', async () => {
    const { chat } = await ready();
    await chat.setMode('plan');
    expect(chat.active!.model.currentModeId).toBe('plan');
    await chat.setConfig('model', 'mock-small');
    expect(chat.active!.model.config.model).toBe('mock-small');
    expect(chat.active!.model.configOptions[0].currentValue).toBe('mock-small');
    expect(chat.active!.model.items.at(-1)).toMatchObject({
      kind: 'notice',
      title: 'Model changed'
    });
    await chat.setConfig('model', 'nope');
    expect(chat.error).toMatch(/unsupported/);
  });

  it('loads an old session with its history and switches between open sessions', async () => {
    const { chat, agents } = await ready();
    await chat.openSession('old-1');
    expect(chat.activeId).toBe('old-1');
    const items = chat.active!.model.items;
    expect(items.map((i) => i.kind)).toEqual(['user', 'thought', 'tool', 'tool', 'assistant']);
    expect(items[3]).toMatchObject({ kind: 'tool', diffs: [{ path: 'src/auth.rs' }] });
    expect(chat.active!.model.plan).toHaveLength(2);
    expect(chat.active!.loading).toBe(false);
    expect(agents[0].received.filter((m) => m.method === 'session/load')).toHaveLength(1);
    // back to the first session and to the old one again: no second load
    await chat.openSession('sess-1');
    expect(chat.activeId).toBe('sess-1');
    await chat.openSession('old-1');
    expect(agents[0].received.filter((m) => m.method === 'session/load')).toHaveLength(1);
    expect(chat.sidebar.find((s) => s.id === 'old-1')?.open).toBe(true);
  });

  it('keeps each session separate while another is running', async () => {
    const { chat } = await ready();
    const first = chat.active!;
    await chat.send('one');
    await chat.newSession();
    expect(chat.activeId).toBe('sess-2');
    expect(chat.active!.model.items).toEqual([]);
    expect(first.model.items.length).toBeGreaterThan(0);
  });

  it('closes sessions and falls back to another or none', async () => {
    const { chat, agents } = await ready();
    await chat.newSession();
    await chat.closeSession('sess-2');
    expect(chat.activeId).toBe('sess-1');
    await chat.closeSession('sess-1');
    expect(chat.activeId).toBeNull();
    expect(agents[0].received.filter((m) => m.method === 'session/close')).toHaveLength(2);
  });

  it('surfaces a prompt error as a notice and an error turn end', async () => {
    const { chat } = await ready();
    await chat.send('mock:error');
    const items = chat.active!.model.items;
    expect(items.at(-2)).toMatchObject({ kind: 'notice', severity: 'error' });
    expect(items.at(-1)).toMatchObject({ kind: 'turn_end', stopReason: 'error' });
    expect(chat.active!.running).toBe(false);
  });

  it('reports a failed connection', async () => {
    const chat = new ChatController({
      transport: () => {
        throw new Error('no host');
      }
    });
    await chat.connect({ cwd: '/x' });
    expect(chat.status).toBe('disconnected');
    expect(chat.error).toBe('no host');
  });

  it('detects an agent crash and reconnects, resuming the open sessions', async () => {
    const { chat, agents } = await ready();
    await chat.send('hi');
    const turn = chat.send('mock:crash');
    await vi.waitFor(() => expect(chat.status).toBe('lost'));
    await turn;
    expect(chat.error).toMatch(/Lost connection/);
    expect(chat.active!.running).toBe(false);
    await chat.reconnect();
    expect(chat.status).toBe('connected');
    expect(chat.error).toBe('');
    expect(agents).toHaveLength(2);
    expect(chat.activeId).toBe('sess-1');
    expect(chat.active!.model.items.length).toBeGreaterThan(0); // transcript kept on screen
    expect(agents[1].received.some((m) => m.method === 'session/resume')).toBe(true);
    await chat.send('again');
    expect(chat.active!.model.items.at(-1)).toMatchObject({ kind: 'turn_end' });
  });

  it('shows the exit reason of an agent that ended', async () => {
    const { chat } = await ready();
    void chat.send('mock:exit 7 permission denied');
    await vi.waitFor(() => expect(chat.status).toBe('lost'));
    expect(chat.error).toBe(
      'Lost connection to the agent: `rc acp` exited with code 7: permission denied'
    );
  });

  it('disconnect clears sessions without flagging a lost connection', async () => {
    const { chat } = await ready();
    await chat.disconnect();
    expect(chat.status).toBe('disconnected');
    expect(chat.error).toBe('');
    expect(chat.active).toBeNull();
  });
});
