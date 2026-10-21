import type { LineTransport } from '../acp/transport';

type Json = Record<string, unknown>;

export interface FakeAgentOptions {
  /** Delay between streamed chunks in ms (0 for tests). */
  delayMs?: number;
  /** Share sessions between agent instances (so a "reconnect" finds them again). */
  store?: FakeStore;
}

interface StoredSession {
  id: string;
  /** `null` matches any project (seed data). */
  cwd: string | null;
  title: string;
  updatedAt: string;
  mode: string;
  model: string;
  history: Json[];
  open: boolean;
}

const MODELS = ['mock-large', 'mock-small', 'mock-reasoning'];
const MODES = [
  { id: 'explore', name: 'explore', description: 'Read-only exploration' },
  { id: 'plan', name: 'plan', description: 'Write the plan' },
  { id: 'implement', name: 'implement', description: 'Make the change' }
];

const text = (t: string): Json => ({ type: 'text', text: t });
const chunk = (kind: string, t: string): Json => ({ sessionUpdate: kind, content: text(t) });

/** Sessions that outlive an agent connection; seeded with two old conversations. */
export class FakeStore {
  sessions = new Map<string, StoredSession>();
  nextSession = 1;
  used = 0;

  constructor() {
    const old = (id: string, title: string, ago: number, history: Json[]) =>
      this.sessions.set(id, {
        id,
        cwd: null,
        title,
        updatedAt: new Date(Date.now() - ago).toISOString(),
        mode: 'implement',
        model: MODELS[0],
        history,
        open: false
      });
    old('old-1', 'Fix the login bug', 3_600_000, [
      chunk('user_message_chunk', 'Fix the login bug in src/auth.rs'),
      chunk('agent_thought_chunk', 'The failing path is probably the token refresh.'),
      {
        sessionUpdate: 'tool_call',
        toolCallId: 'old-t1',
        title: 'Read src/auth.rs',
        kind: 'read',
        status: 'completed',
        locations: [{ path: 'src/auth.rs', line: 40 }],
        rawInput: { file_path: 'src/auth.rs' },
        content: [
          { type: 'content', content: text('fn refresh(token: &str) -> bool {\n  true\n}') }
        ]
      },
      {
        sessionUpdate: 'tool_call',
        toolCallId: 'old-t2',
        title: 'Edit src/auth.rs',
        kind: 'edit',
        status: 'completed',
        locations: [{ path: 'src/auth.rs' }],
        content: [
          {
            type: 'diff',
            path: 'src/auth.rs',
            oldText: 'fn refresh(token: &str) -> bool {\n  true\n}\n',
            newText: 'fn refresh(token: &str) -> bool {\n  !token.is_empty()\n}\n'
          }
        ]
      },
      chunk(
        'agent_message_chunk',
        'Fixed: `refresh` now rejects empty tokens.\n\n```rust\nfn refresh(token: &str) -> bool {\n  !token.is_empty()\n}\n```\n'
      ),
      {
        sessionUpdate: 'plan',
        entries: [
          { content: 'Find the bug', priority: 'high', status: 'completed' },
          { content: 'Fix it', priority: 'high', status: 'completed' }
        ]
      }
    ]);
    old('old-2', 'Add dark mode', 86_400_000, [
      chunk('user_message_chunk', 'Add dark mode to the settings page'),
      chunk('agent_message_chunk', 'Added a **dark mode** toggle.'),
      {
        sessionUpdate: 'notice',
        severity: 'info',
        title: 'Model changed',
        description: 'mock-small'
      }
    ]);
  }
}

/**
 * A scripted in-memory ACP agent behind a {@link LineTransport}. It speaks enough of the protocol
 * to drive the whole UI (and the client/controller tests) without a real agent: sessions with
 * list/load/resume/close, modes, a `model` config option, plans, diffs, usage and notices.
 *
 * Prompts select a script: `mock:write <path> <text>` (permission + diff), `mock:ask` (one
 * question), `mock:askmulti` (three questions), `mock:mode <id>` (SwitchMode permission),
 * `mock:edit` (diff without permission), `mock:bash` (long output), `mock:md` (markdown),
 * `mock:notice`, `mock:slow` (long stream, for cancel and queueing), `mock:error`,
 * `mock:crash`. Anything else is echoed back as streamed text.
 */
export class FakeAgent implements LineTransport {
  /** Every line received from the client, parsed. */
  readonly received: Json[] = [];
  clientCapabilities: Json | undefined;
  readonly store: FakeStore;
  private listeners = new Set<(line: string) => void>();
  private closers = new Set<() => void>();
  private nextId = 1000;
  private pendingReplies = new Map<number, (msg: Json) => void>();
  private cancelled = false;
  private dead = false;
  private delayMs: number;

  constructor(opts: FakeAgentOptions = {}) {
    this.delayMs = opts.delayMs ?? 0;
    this.store = opts.store ?? new FakeStore();
  }

  onLine(cb: (line: string) => void): () => void {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }

  onClose(cb: () => void): () => void {
    this.closers.add(cb);
    return () => this.closers.delete(cb);
  }

  send(line: string): void {
    if (this.dead) return;
    const msg = JSON.parse(line) as Json;
    this.received.push(msg);
    void this.handle(msg);
  }

  close(): void {
    this.dead = true;
    this.listeners.clear();
    this.closers.clear();
  }

  /** Simulate the agent process dying. */
  crash(): void {
    this.dead = true;
    this.listeners.clear();
    const closers = [...this.closers];
    this.closers.clear();
    queueMicrotask(() => closers.forEach((c) => c()));
  }

  private emit(msg: Json): void {
    if (this.dead) return;
    const line = JSON.stringify({ jsonrpc: '2.0', ...msg });
    // Deliver asynchronously like a real pipe.
    queueMicrotask(() => this.listeners.forEach((l) => l(line)));
  }

  private respond(id: unknown, result: unknown): void {
    this.emit({ id, result });
  }

  private fail(id: unknown, code: number, message: string): void {
    this.emit({ id, error: { code, message } });
  }

  private update(sessionId: string, update: Json): void {
    const s = this.store.sessions.get(sessionId);
    if (s && update.sessionUpdate !== 'usage_update') {
      s.history.push(update);
      s.updatedAt = new Date().toISOString();
    }
    this.emit({ method: 'session/update', params: { sessionId, update } });
  }

  private callClient(method: string, params: Json): Promise<Json> {
    const id = this.nextId++;
    return new Promise((resolve) => {
      this.pendingReplies.set(id, resolve);
      this.emit({ id, method, params });
    });
  }

  private async pause(min = 0): Promise<void> {
    const ms = Math.max(this.delayMs, min);
    if (ms > 0) await new Promise((r) => setTimeout(r, ms));
  }

  private modeState(s: StoredSession): Json {
    return { currentModeId: s.mode, availableModes: MODES };
  }

  private configOptions(s: StoredSession): Json[] {
    return [
      {
        id: 'model',
        name: 'Model',
        category: 'model',
        type: 'select',
        currentValue: s.model,
        options: MODELS.map((m) => ({ value: m, name: m }))
      }
    ];
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
    const sid = String(params.sessionId ?? '');
    const sessions = this.store.sessions;
    switch (msg.method) {
      case 'initialize':
        this.clientCapabilities = params.clientCapabilities as Json | undefined;
        this.respond(id, {
          protocolVersion: 1,
          agentCapabilities: {
            loadSession: true,
            sessionCapabilities: { list: {}, resume: {}, close: {} }
          },
          agentInfo: { name: 'fake-agent', version: '0.0.0' },
          authMethods: []
        });
        break;
      case 'session/new': {
        const sessionId = `sess-${this.store.nextSession++}`;
        const s: StoredSession = {
          id: sessionId,
          cwd: String(params.cwd ?? ''),
          title: '',
          updatedAt: new Date().toISOString(),
          mode: 'implement',
          model: MODELS[0],
          history: [],
          open: true
        };
        sessions.set(sessionId, s);
        this.respond(id, {
          sessionId,
          modes: this.modeState(s),
          configOptions: this.configOptions(s)
        });
        break;
      }
      case 'session/list': {
        const cwd = params.cwd as string | undefined;
        const list = [...sessions.values()]
          .filter((s) => s.title && (!cwd || s.cwd === null || s.cwd === cwd))
          .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
          .map((s) => ({
            sessionId: s.id,
            cwd: s.cwd ?? cwd ?? '/',
            title: s.title,
            updatedAt: s.updatedAt,
            _meta: { ruckcode: { mode: s.mode, createdAt: s.updatedAt, firstPrompt: s.title } }
          }));
        this.respond(id, { sessions: list });
        break;
      }
      case 'session/load':
      case 'session/resume': {
        const s = sessions.get(sid);
        if (!s) return this.fail(id, -32602, `unknown session ${sid}`);
        s.open = true;
        if (msg.method === 'session/load') {
          for (const u of s.history) {
            this.emit({ method: 'session/update', params: { sessionId: sid, update: u } });
          }
        }
        this.respond(id, { modes: this.modeState(s), configOptions: this.configOptions(s) });
        break;
      }
      case 'session/close': {
        const s = sessions.get(sid);
        if (!s) return this.fail(id, -32602, `unknown session ${sid}`);
        s.open = false;
        this.cancelled = true;
        this.respond(id, {});
        break;
      }
      case 'session/set_mode': {
        const s = sessions.get(sid);
        if (s) s.mode = String(params.modeId);
        this.update(sid, { sessionUpdate: 'current_mode_update', currentModeId: params.modeId });
        this.respond(id, {});
        break;
      }
      case 'session/set_config_option': {
        const s = sessions.get(sid);
        if (!s || params.configId !== 'model' || !MODELS.includes(String(params.value))) {
          return this.fail(id, -32602, 'unsupported config option or value');
        }
        s.model = String(params.value);
        this.update(sid, {
          sessionUpdate: 'config_option_update',
          configOptions: this.configOptions(s)
        });
        this.update(sid, {
          sessionUpdate: 'notice',
          severity: 'info',
          title: 'Model changed',
          description: s.model
        });
        this.respond(id, { configOptions: this.configOptions(s) });
        break;
      }
      case 'session/cancel':
        this.cancelled = true;
        break;
      case 'session/prompt':
        await this.prompt(id, params);
        break;
      default:
        if (id !== undefined) this.fail(id, -32601, `method not found: ${msg.method}`);
    }
  }

  private async text(sid: string, kind: string, t: string, minDelay = 0): Promise<void> {
    for (const word of t.split(/(?<= |\n)/)) {
      if (this.cancelled) return;
      this.update(sid, chunk(kind, word));
      await this.pause(minDelay);
    }
  }

  private permissionOptions(): Json[] {
    return [
      { optionId: 'allow', name: 'Allow', kind: 'allow_once' },
      { optionId: 'always', name: 'Always allow', kind: 'allow_always' },
      { optionId: 'reject', name: 'Reject', kind: 'reject_once' }
    ];
  }

  private async prompt(id: unknown, params: Json): Promise<void> {
    const sid = String(params.sessionId);
    const s = this.store.sessions.get(sid);
    if (!s) return this.fail(id, -32602, `unknown session ${sid}`);
    const blocks = (params.prompt ?? []) as { type: string; text?: string }[];
    const input = blocks.map((b) => b.text ?? '').join('');
    this.cancelled = false;
    if (!s.title) s.title = input.split('\n')[0].slice(0, 80);
    s.history.push(chunk('user_message_chunk', input));
    s.updatedAt = new Date().toISOString();

    if (input.startsWith('mock:crash')) return this.crash();
    if (input.startsWith('mock:error')) return this.fail(id, -32000, 'mock agent failure');

    await this.text(sid, 'agent_thought_chunk', 'Looking at the request. ');
    if (input.startsWith('mock:write ')) {
      const [, path = 'a.txt', ...rest] = input.split(' ');
      const body = rest.join(' ');
      const diff = {
        type: 'diff',
        path,
        ...(path.startsWith('new') ? {} : { oldText: `old contents of ${path}\nsecond line\n` }),
        newText: `${body}\nsecond line\n`
      };
      const toolCallId = `call-${this.nextId++}`;
      const title = `Write ${path}`;
      const rawInput = { file_path: path, content: body };
      this.update(sid, {
        sessionUpdate: 'tool_call',
        toolCallId,
        title,
        kind: 'edit',
        rawInput,
        locations: [{ path }]
      });
      const reply = await this.callClient('session/request_permission', {
        sessionId: sid,
        toolCall: { toolCallId, title, kind: 'edit', rawInput, content: [diff] },
        options: this.permissionOptions()
      });
      const outcome = (reply.result as Json | undefined)?.outcome as Json | undefined;
      const ok = outcome?.outcome === 'selected' && outcome.optionId !== 'reject';
      this.update(sid, {
        sessionUpdate: 'tool_call_update',
        toolCallId,
        status: ok ? 'completed' : 'failed',
        content: [
          { type: 'content', content: text(ok ? `wrote ${body}` : 'rejected by user') },
          ...(ok ? [diff] : [])
        ]
      });
      await this.text(
        sid,
        'agent_message_chunk',
        ok ? `Wrote ${path}. ` : `Did not write ${path}. `
      );
    } else if (input.startsWith('mock:askmulti')) {
      const reply = await this.callClient('_ruckcode/ask_user_question', {
        sessionId: sid,
        questions: [
          {
            question: 'Which database?',
            header: 'DB',
            multiSelect: false,
            options: [
              { label: 'sqlite (Recommended)', description: 'embedded, zero config' },
              { label: 'postgres', description: 'networked' }
            ]
          },
          {
            question: 'Which features?',
            header: 'Features',
            multiSelect: true,
            options: [
              { label: 'auth', description: 'login and sessions' },
              { label: 'metrics' },
              { label: 'search' }
            ]
          },
          {
            question: 'Anything else?',
            multiSelect: false,
            options: [{ label: 'No' }, { label: 'Yes' }]
          }
        ]
      });
      const result = reply.result as {
        cancelled?: boolean;
        answers?: { selected: string[]; other: string | null }[];
      };
      const summary = (result?.answers ?? [])
        .map((a) => [...a.selected, ...(a.other ? [`other: ${a.other}`] : [])].join(', '))
        .join(' | ');
      await this.text(
        sid,
        'agent_message_chunk',
        result?.cancelled ? 'Questions cancelled. ' : `Answers: ${summary}. `
      );
    } else if (input.startsWith('mock:ask')) {
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
    } else if (input.startsWith('mock:mode')) {
      const target = input.split(' ')[1] || 'explore';
      const toolCallId = `call-${this.nextId++}`;
      const title = `Switch mode to ${target}`;
      const rawInput = { mode_id: target, reason: 'The exploration is done; time to switch.' };
      this.update(sid, {
        sessionUpdate: 'tool_call',
        toolCallId,
        title,
        kind: 'switch_mode',
        rawInput
      });
      const reply = await this.callClient('session/request_permission', {
        sessionId: sid,
        toolCall: { toolCallId, title, kind: 'switch_mode', rawInput },
        options: [
          { optionId: 'allow', name: 'Allow', kind: 'allow_once' },
          { optionId: 'reject', name: 'Reject', kind: 'reject_once' }
        ]
      });
      const outcome = (reply.result as Json | undefined)?.outcome as Json | undefined;
      const ok = outcome?.outcome === 'selected' && outcome.optionId === 'allow';
      if (ok) {
        s.mode = target;
        this.update(sid, { sessionUpdate: 'current_mode_update', currentModeId: target });
      }
      this.update(sid, {
        sessionUpdate: 'tool_call_update',
        toolCallId,
        status: ok ? 'completed' : 'failed'
      });
      await this.text(
        sid,
        'agent_message_chunk',
        ok ? `Switched to ${target}. ` : `Staying in ${s.mode}. `
      );
    } else if (input.startsWith('mock:edit')) {
      const toolCallId = `call-${this.nextId++}`;
      this.update(sid, {
        sessionUpdate: 'tool_call',
        toolCallId,
        title: 'Edit src/lib.rs',
        kind: 'edit',
        status: 'in_progress',
        locations: [{ path: 'src/lib.rs', line: 3 }],
        rawInput: {
          file_path: 'src/lib.rs',
          old_string: 'a + b',
          new_string: 'a.saturating_add(b)'
        }
      });
      await this.pause();
      this.update(sid, {
        sessionUpdate: 'tool_call_update',
        toolCallId,
        status: 'completed',
        content: [
          {
            type: 'diff',
            path: 'src/lib.rs',
            oldText:
              'pub fn add(a: u32, b: u32) -> u32 {\n    a + b\n}\n\npub fn sub(a: u32, b: u32) -> u32 {\n    a - b\n}\n',
            newText:
              'pub fn add(a: u32, b: u32) -> u32 {\n    a.saturating_add(b)\n}\n\npub fn sub(a: u32, b: u32) -> u32 {\n    a - b\n}\n\n// tested\n'
          }
        ]
      });
      await this.text(sid, 'agent_message_chunk', 'Edited `src/lib.rs`. ');
    } else if (input.startsWith('mock:bash')) {
      const toolCallId = `call-${this.nextId++}`;
      this.update(sid, {
        sessionUpdate: 'tool_call',
        toolCallId,
        title: 'Run cargo test',
        kind: 'execute',
        status: 'in_progress',
        rawInput: { command: 'cargo test' }
      });
      await this.pause();
      const out = Array.from({ length: 60 }, (_, i) => `test case_${i} ... ok`).join('\n');
      this.update(sid, {
        sessionUpdate: 'tool_call_update',
        toolCallId,
        status: input.includes('fail') ? 'failed' : 'completed',
        content: [{ type: 'content', content: text(out) }]
      });
      await this.text(sid, 'agent_message_chunk', 'Tests ran. ');
    } else if (input.startsWith('mock:md')) {
      await this.text(
        sid,
        'agent_message_chunk',
        '# Heading\n\nSome **bold**, *italic* and `inline` text with a [link](https://example.com).\n\n- one\n- two\n\n```ts\nconst x: number = 1;\nconsole.log(x);\n```\n\n<script>alert(1)</script>\n'
      );
    } else if (input.startsWith('mock:notice')) {
      this.update(sid, {
        sessionUpdate: 'notice',
        severity: 'warning',
        title: 'Hook output',
        description: 'format hook changed 2 files'
      });
      this.update(sid, { sessionUpdate: 'notice', severity: 'error', title: 'MCP server failed' });
    } else if (input.startsWith('mock:slow')) {
      await this.text(
        sid,
        'agent_message_chunk',
        Array.from({ length: 80 }, (_, i) => `word${i} `).join(''),
        40
      );
    } else {
      this.update(sid, {
        sessionUpdate: 'plan',
        entries: [
          { content: 'Read the request', priority: 'medium', status: 'completed' },
          { content: 'Answer', priority: 'medium', status: 'in_progress' },
          { content: 'Double-check', priority: 'low', status: 'pending' }
        ]
      });
      await this.text(sid, 'agent_message_chunk', `You said: ${input}. `);
    }
    this.store.used += 120;
    this.update(sid, { sessionUpdate: 'usage_update', used: this.store.used, size: 200000 });
    this.update(sid, { sessionUpdate: 'notice', severity: 'info', title: 'Turn finished' });
    this.respond(id, { stopReason: this.cancelled ? 'cancelled' : 'end_turn' });
  }
}
