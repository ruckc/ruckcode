import { describe, expect, it } from 'vitest';
import type { SessionNotification } from '@agentclientprotocol/sdk';
import { FakeAgent } from '../host/fake-agent';
import { applyUpdate, emptyModel } from '../state/reducer';
import { AcpClient, type ClientHandlers } from './client';
import type { AskUserQuestionResult } from './ext';

function setup(overrides: Partial<ClientHandlers> = {}) {
  const agent = new FakeAgent();
  const model = emptyModel();
  const updates: SessionNotification[] = [];
  const handlers: ClientHandlers = {
    onSessionUpdate: (n) => {
      updates.push(n);
      applyUpdate(model, n.update);
    },
    requestPermission: async (req) => ({
      outcome: { outcome: 'selected', optionId: req.options[0].optionId }
    }),
    askUserQuestion: async () => ({ answers: [], cancelled: true }),
    ...overrides
  };
  const client = new AcpClient(agent, handlers);
  return { agent, client, model, updates };
}

describe('AcpClient over a fake transport', () => {
  it('advertises ruckcode capabilities in initialize', async () => {
    const { agent, client } = setup();
    const init = await client.initialize();
    expect(init.agentInfo?.name).toBe('fake-agent');
    expect(agent.clientCapabilities).toMatchObject({
      session: { notices: {} },
      _meta: { ruckcode: { askUserQuestion: true, notices: true } }
    });
    const first = agent.received[0];
    expect(first.method).toBe('initialize');
    expect((first.params as { protocolVersion: number }).protocolVersion).toBe(1);
  });

  it('creates a session and reports modes', async () => {
    const { client } = setup();
    await client.initialize();
    const s = await client.newSession('/proj');
    expect(s.sessionId).toBe('sess-1');
    expect(s.modes?.currentModeId).toBe('implement');
    expect(s.modes?.availableModes.map((m) => m.id)).toEqual(['explore', 'plan', 'implement']);
  });

  it('streams a prompt turn into the chat model', async () => {
    const { client, model, updates } = setup();
    await client.initialize();
    const { sessionId } = await client.newSession('/proj');
    const res = await client.prompt(sessionId, 'hello');
    expect(res.stopReason).toBe('end_turn');
    expect(updates.length).toBeGreaterThan(3);
    const kinds = model.items.map((i) => i.kind);
    expect(kinds).toContain('thought');
    expect(kinds).toContain('assistant');
    const assistant = model.items.find((i) => i.kind === 'assistant');
    expect(assistant && 'text' in assistant && assistant.text).toBe('You said: hello. ');
    expect(model.plan).toHaveLength(3);
    expect(model.usage).toEqual({ used: 120, size: 200000 });
    expect(model.items.at(-1)).toMatchObject({ kind: 'notice', title: 'Turn finished' });
  });

  it('round-trips session/request_permission (allow)', async () => {
    let seen: string[] = [];
    const { client, model } = setup({
      requestPermission: async (req) => {
        seen = req.options.map((o) => o.kind);
        return { outcome: { outcome: 'selected', optionId: 'allow' } };
      }
    });
    await client.initialize();
    const { sessionId } = await client.newSession('/proj');
    await client.prompt(sessionId, 'mock:write a.txt hi');
    expect(seen).toEqual(['allow_once', 'allow_always', 'reject_once']);
    const tool = model.items.find((i) => i.kind === 'tool');
    expect(tool).toMatchObject({ title: 'Write a.txt', toolKind: 'edit', status: 'completed' });
    expect(tool && 'output' in tool && tool.output).toBe('wrote hi');
  });

  it('round-trips session/request_permission (reject and cancel)', async () => {
    for (const outcome of [
      { outcome: 'selected' as const, optionId: 'reject' },
      { outcome: 'cancelled' as const }
    ]) {
      const { client, model } = setup({ requestPermission: async () => ({ outcome }) });
      await client.initialize();
      const { sessionId } = await client.newSession('/proj');
      await client.prompt(sessionId, 'mock:write a.txt hi');
      expect(model.items.find((i) => i.kind === 'tool')).toMatchObject({ status: 'failed' });
    }
  });

  it('round-trips the _ruckcode/ask_user_question extension', async () => {
    let asked: unknown;
    const { client, model, agent } = setup({
      askUserQuestion: async (req): Promise<AskUserQuestionResult> => {
        asked = req;
        return {
          cancelled: false,
          answers: [{ question: req.questions[0].question, selected: ['sqlite'], other: null }]
        };
      }
    });
    await client.initialize();
    const { sessionId } = await client.newSession('/proj');
    await client.prompt(sessionId, 'mock:ask');
    expect(asked).toMatchObject({
      questions: [{ question: 'Which database?', multiSelect: false }]
    });
    const reply = agent.received.find((m) => m.result && 'answers' in (m.result as object));
    expect(reply?.result).toEqual({
      cancelled: false,
      answers: [{ question: 'Which database?', selected: ['sqlite'], other: null }]
    });
    expect(model.items.find((i) => i.kind === 'assistant')).toMatchObject({
      text: 'You chose sqlite. '
    });
  });

  it('sends session/cancel as a notification and set_mode as a request', async () => {
    const { client, agent, model } = setup();
    await client.initialize();
    const { sessionId } = await client.newSession('/proj');
    await client.setMode(sessionId, 'explore');
    expect(model.currentModeId).toBe('explore');
    await client.cancel(sessionId);
    await new Promise((r) => setTimeout(r, 0));
    const cancel = agent.received.find((m) => m.method === 'session/cancel');
    expect(cancel).toBeDefined();
    expect(cancel && 'id' in cancel).toBe(false);
  });

  it('surfaces JSON-RPC errors from the agent', async () => {
    const { client } = setup();
    await client.initialize();
    await expect(client.unknown('nope/missing')).rejects.toThrow(/method not found/);
  });
});
