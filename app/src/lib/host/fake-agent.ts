import type { LineTransport } from '../acp/transport';

type Json = Record<string, unknown>;

export interface FakeAgentOptions {
  /** Delay between streamed chunks in ms (0 for tests). */
  delayMs?: number;
}

/**
 * A scripted in-memory ACP agent behind a {@link LineTransport}. It speaks just enough of the
 * protocol to drive the UI and the client tests without a real agent. Prompts follow the mock
 * model's conventions: `mock:write <path> <text>` asks for permission, `mock:ask` asks a
 * question, anything else is echoed back as streamed text.
 */
export class FakeAgent implements LineTransport {
  /** Every line received from the client, parsed. */
  readonly received: Json[] = [];
  clientCapabilities: Json | undefined;
  private listeners = new Set<(line: string) => void>();
  private nextId = 1000;
  private pendingReplies = new Map<number, (msg: Json) => void>();
  private cancelled = false;
  private mode = 'implement';
  private delayMs: number;

  constructor(opts: FakeAgentOptions = {}) {
    this.delayMs = opts.delayMs ?? 0;
  }

  onLine(cb: (line: string) => void): () => void {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }

  send(line: string): void {
    const msg = JSON.parse(line) as Json;
    this.received.push(msg);
    void this.handle(msg);
  }

  close(): void {
    this.listeners.clear();
  }

  private emit(msg: Json): void {
    const line = JSON.stringify({ jsonrpc: '2.0', ...msg });
    // Deliver asynchronously like a real pipe.
    queueMicrotask(() => this.listeners.forEach((l) => l(line)));
  }

  private respond(id: unknown, result: unknown): void {
    this.emit({ id, result });
  }

  private update(sessionId: string, update: Json): void {
    this.emit({ method: 'session/update', params: { sessionId, update } });
  }

  private callClient(method: string, params: Json): Promise<Json> {
    const id = this.nextId++;
    return new Promise((resolve) => {
      this.pendingReplies.set(id, resolve);
      this.emit({ id, method, params });
    });
  }

  private async pause(): Promise<void> {
    if (this.delayMs > 0) await new Promise((r) => setTimeout(r, this.delayMs));
  }

  private async handle(msg: Json): Promise<void> {
    const id = msg.id as number | string | undefined;
    // Response to one of our requests to the client.
    if (typeof id === 'number' && msg.method === undefined && this.pendingReplies.has(id)) {
      this.pendingReplies.get(id)!(msg);
      this.pendingReplies.delete(id);
      return;
    }
    const params = (msg.params ?? {}) as Json;
    switch (msg.method) {
      case 'initialize':
        this.clientCapabilities = params.clientCapabilities as Json | undefined;
        this.respond(id, {
          protocolVersion: 1,
          agentCapabilities: { loadSession: false },
          agentInfo: { name: 'fake-agent', version: '0.0.0' },
          authMethods: []
        });
        break;
      case 'session/new':
        this.respond(id, {
          sessionId: 'sess-1',
          modes: {
            currentModeId: this.mode,
            availableModes: [
              { id: 'explore', name: 'explore' },
              { id: 'implement', name: 'implement' }
            ]
          }
        });
        break;
      case 'session/set_mode':
        this.mode = String(params.modeId);
        this.update(String(params.sessionId), {
          sessionUpdate: 'current_mode_update',
          currentModeId: this.mode
        });
        this.respond(id, {});
        break;
      case 'session/cancel':
        this.cancelled = true;
        break;
      case 'session/prompt':
        await this.prompt(id, params);
        break;
      default:
        if (id !== undefined) {
          this.emit({ id, error: { code: -32601, message: `method not found: ${msg.method}` } });
        }
    }
  }

  private async text(sid: string, kind: string, text: string): Promise<void> {
    for (const word of text.split(/(?<= )/)) {
      if (this.cancelled) return;
      this.update(sid, { sessionUpdate: kind, content: { type: 'text', text: word } });
      await this.pause();
    }
  }

  private async prompt(id: unknown, params: Json): Promise<void> {
    const sid = String(params.sessionId);
    const blocks = (params.prompt ?? []) as { type: string; text?: string }[];
    const text = blocks.map((b) => b.text ?? '').join('');
    this.cancelled = false;

    await this.text(sid, 'agent_thought_chunk', 'Looking at the request. ');
    if (text.startsWith('mock:write ')) {
      const [, path = 'a.txt', ...rest] = text.split(' ');
      const toolCallId = 'call-1';
      this.update(sid, {
        sessionUpdate: 'tool_call',
        toolCallId,
        title: `Write ${path}`,
        kind: 'edit'
      });
      const reply = await this.callClient('session/request_permission', {
        sessionId: sid,
        toolCall: { toolCallId, title: `Write ${path}`, kind: 'edit' },
        options: [
          { optionId: 'allow', name: 'Allow', kind: 'allow_once' },
          { optionId: 'always', name: 'Always allow', kind: 'allow_always' },
          { optionId: 'reject', name: 'Reject', kind: 'reject_once' }
        ]
      });
      const outcome = (reply.result as Json | undefined)?.outcome as Json | undefined;
      const ok = outcome?.outcome === 'selected' && outcome.optionId !== 'reject';
      this.update(sid, {
        sessionUpdate: 'tool_call_update',
        toolCallId,
        status: ok ? 'completed' : 'failed',
        content: [
          {
            type: 'content',
            content: { type: 'text', text: ok ? `wrote ${rest.join(' ')}` : 'rejected by user' }
          }
        ]
      });
      await this.text(
        sid,
        'agent_message_chunk',
        ok ? `Wrote ${path}. ` : `Did not write ${path}. `
      );
    } else if (text.startsWith('mock:ask')) {
      const reply = await this.callClient('_ruckcode/ask_user_question', {
        sessionId: sid,
        questions: [
          {
            question: 'Which database?',
            header: 'DB',
            multiSelect: false,
            options: [{ label: 'sqlite', description: 'embedded' }, { label: 'postgres' }]
          }
        ]
      });
      const result = reply.result as { cancelled?: boolean; answers?: { selected: string[] }[] };
      const picked = result?.answers?.[0]?.selected?.join(', ') ?? '';
      await this.text(
        sid,
        'agent_message_chunk',
        result?.cancelled ? 'Question cancelled. ' : `You chose ${picked}. `
      );
    } else {
      this.update(sid, {
        sessionUpdate: 'plan',
        entries: [
          { content: 'Read the request', priority: 'medium', status: 'completed' },
          { content: 'Answer', priority: 'medium', status: 'in_progress' }
        ]
      });
      await this.text(sid, 'agent_message_chunk', `You said: ${text}. `);
    }
    this.update(sid, { sessionUpdate: 'usage_update', used: 120, size: 200000 });
    this.update(sid, { sessionUpdate: 'notice', severity: 'info', title: 'Turn finished' });
    this.respond(id, { stopReason: this.cancelled ? 'cancelled' : 'end_turn' });
  }
}
