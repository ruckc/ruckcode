import type {
  RequestPermissionRequest,
  RequestPermissionResponse,
  SessionConfigOption,
  SessionModeState,
  SessionNotification
} from '@agentclientprotocol/sdk';
import { SvelteSet } from 'svelte/reactivity';
import { AcpClient } from '../acp/client';
import type { AskUserQuestionRequest, AskUserQuestionResult } from '../acp/ext';
import type { LineTransport } from '../acp/transport';
import { connectHost, getTranscripts, type ConnectOptions } from '../host';
import type { UnfinishedSession } from '../host/transcripts';
import { applyUpdate, emptyModel, pushTurnEnd, setConfigOptions, type ChatModel } from './reducer';

export type Status = 'disconnected' | 'connecting' | 'connected' | 'lost';

export interface PendingPermission {
  sessionId: string;
  request: RequestPermissionRequest;
  resolve(r: RequestPermissionResponse): void;
}

export interface PendingQuestion {
  sessionId: string | undefined;
  request: AskUserQuestionRequest;
  resolve(r: AskUserQuestionResult): void;
}

export interface SessionSummary {
  id: string;
  title: string;
  updatedAt: string | null;
  mode: string | null;
}

/** One open session: its transcript model plus turn state. */
export interface SessionView {
  id: string;
  model: ChatModel;
  running: boolean;
  /** True while `session/load` replays history. */
  loading: boolean;
  /** Prompts typed while a turn was running; sent in order afterwards. */
  queue: string[];
}

/** Sent by "compact and resume": `/compact` with these instructions, then the continue prompt. */
export const RESUME_COMPACT_PROMPT =
  '/compact This session was interrupted and is being resumed. Summarize it focused on what was in progress when it stopped (the exact step, files and commands involved), what has been completed, and what remains to be done. Drop everything else.';
export const RESUME_CONTINUE_PROMPT =
  'Continue with the work that was in progress and finish what remains.';

export interface ControllerOptions {
  /** Test hook: find unfinished sessions instead of asking the host. */
  unfinished?: (opts: ConnectOptions) => Promise<UnfinishedSession[]>;
  /** Test hook: record a dismissal instead of asking the host. */
  dismissUnfinished?: (opts: ConnectOptions, id: string) => Promise<void>;
  /** Test hook: supply the transport directly instead of going through the host. */
  transport?: () => LineTransport | Promise<LineTransport>;
}

function newView(id: string): SessionView {
  return { id, model: emptyModel(), running: false, loading: false, queue: [] };
}

type SessionSetup = {
  modes?: SessionModeState | null;
  configOptions?: SessionConfigOption[] | null;
};

/** UI-facing state for one agent connection and the sessions open on it. */
export class ChatController {
  status = $state<Status>('disconnected');
  error = $state('');
  agentName = $state('');
  cwd = $state('');
  sessions = $state<Record<string, SessionView>>({});
  activeId = $state<string | null>(null);
  /** Stored sessions from `session/list`, newest first. */
  stored = $state.raw<SessionSummary[]>([]);
  permissions = $state.raw<PendingPermission[]>([]);
  questions = $state.raw<PendingQuestion[]>([]);
  /** Prompts sent this run, oldest first (for up/down recall). */
  history = $state.raw<string[]>([]);
  /** Sessions left interrupted or with open tasks, offered once after connecting. */
  unfinished = $state.raw<UnfinishedSession[]>([]);

  private client: AcpClient | null = null;
  /** Sessions closed here: late updates for them (e.g. available commands) are dropped. */
  private closed = new SvelteSet<string>();
  private opts: ConnectOptions | null = null;
  private canLoad = true;

  constructor(private readonly hooks: ControllerOptions = {}) {}

  get active(): SessionView | null {
    return this.activeId ? (this.sessions[this.activeId] ?? null) : null;
  }
  get connected(): boolean {
    return this.status === 'connected';
  }
  get permission(): PendingPermission | null {
    return this.permissions[0] ?? null;
  }
  get question(): PendingQuestion | null {
    return this.questions[0] ?? null;
  }
  /** Open sessions not yet in the stored list (never prompted), then the stored ones. */
  get sidebar(): (SessionSummary & { open: boolean })[] {
    const known = this.stored.map((s) => s.id);
    const fresh = Object.values(this.sessions)
      .filter((v) => !known.includes(v.id))
      .map((v) => ({
        id: v.id,
        title: v.model.title ?? 'New session',
        updatedAt: null,
        mode: v.model.currentModeId,
        open: true
      }));
    return [...fresh, ...this.stored.map((s) => ({ ...s, open: s.id in this.sessions }))];
  }

  // ---- connection -------------------------------------------------------------------

  async connect(opts: ConnectOptions): Promise<void> {
    this.opts = opts;
    this.cwd = opts.cwd;
    this.sessions = {};
    this.activeId = null;
    this.stored = [];
    this.unfinished = [];
    await this.open(null);
    if (this.connected) await this.offerUnfinished();
  }

  private async offerUnfinished(): Promise<void> {
    const opts = this.opts;
    if (!opts) return;
    try {
      const find =
        this.hooks.unfinished ??
        (async (o: ConnectOptions) =>
          (await getTranscripts()).unfinished({ project: o.cwd, distro: o.distro }));
      const found = await find(opts);
      // Nothing to offer for a session that is already open.
      this.unfinished = found.filter((u) => !(u.id in this.sessions));
    } catch {
      // The offer is a convenience; never let it break connecting.
      this.unfinished = [];
    }
  }

  /** Forget one offer (the dialog's Dismiss), and tell the host not to offer it again. */
  async dismissUnfinished(id: string): Promise<void> {
    this.unfinished = this.unfinished.filter((u) => u.id !== id);
    const opts = this.opts;
    if (!opts) return;
    try {
      if (this.hooks.dismissUnfinished) await this.hooks.dismissUnfinished(opts, id);
      else await (await getTranscripts()).dismiss({ project: opts.cwd, distro: opts.distro }, id);
    } catch (e) {
      this.fail(e);
    }
  }

  /** Close the offer without touching the sessions (start fresh). */
  closeUnfinishedOffer(): void {
    this.unfinished = [];
  }

  /**
   * Continue an unfinished session. With `compact`, first summarize it to what was in progress
   * and what remains (the agent's `/compact`), then carry on with the rest.
   */
  async resumeUnfinished(id: string, compact: boolean): Promise<void> {
    this.unfinished = [];
    await this.openSession(id);
    const v = this.sessions[id];
    if (!compact || !v || this.activeId !== id) return;
    v.queue.push(RESUME_CONTINUE_PROMPT);
    await this.send(RESUME_COMPACT_PROMPT);
  }

  /** Reconnect after the agent went away, resuming the sessions that were open. */
  async reconnect(): Promise<void> {
    if (!this.opts) return;
    const ids = Object.keys(this.sessions);
    const wasActive = this.activeId;
    await this.open(ids.length ? { ids, active: wasActive } : null);
  }

  private async open(restore: { ids: string[]; active: string | null } | null): Promise<void> {
    this.error = '';
    this.status = 'connecting';
    try {
      await this.teardown();
      const transport = this.hooks.transport
        ? await this.hooks.transport()
        : await connectHost(this.opts!);
      const client = new AcpClient(transport, {
        onSessionUpdate: (n: SessionNotification) => this.onUpdate(n),
        requestPermission: (request) =>
          new Promise((resolve) => {
            const p: PendingPermission = {
              sessionId: request.sessionId,
              request,
              resolve: (r) => {
                this.permissions = this.permissions.filter((x) => x !== p);
                resolve(r);
              }
            };
            this.permissions = [...this.permissions, p];
          }),
        askUserQuestion: (request) =>
          new Promise((resolve) => {
            const q: PendingQuestion = {
              sessionId: request.sessionId,
              request,
              resolve: (r) => {
                this.questions = this.questions.filter((x) => x !== q);
                resolve(r);
              }
            };
            this.questions = [...this.questions, q];
          })
      });
      this.client = client;
      void client.closed.then(() => this.onClosed(client, transport.closeReason));
      const init = await client.initialize();
      this.agentName = init.agentInfo?.name ?? 'agent';
      this.canLoad = init.agentCapabilities?.loadSession ?? false;
      this.status = 'connected';
      if (restore) {
        const keep: Record<string, SessionView> = {};
        for (const id of restore.ids) {
          const v = this.sessions[id];
          try {
            this.applySetup(v, await client.resumeSession(id, this.cwd));
            keep[id] = v;
          } catch {
            // session no longer exists on the agent; drop it
          }
        }
        this.sessions = keep;
        this.activeId = restore.active && keep[restore.active] ? restore.active : null;
      }
      await this.refreshSessions();
      if (!this.active) await this.newSession();
    } catch (e) {
      this.fail(e);
      await this.teardown();
      this.status = 'disconnected';
    }
  }

  async disconnect(): Promise<void> {
    await this.teardown();
    this.status = 'disconnected';
    this.sessions = {};
    this.activeId = null;
    this.stored = [];
  }

  private async teardown(): Promise<void> {
    const c = this.client;
    this.client = null;
    this.cancelDialogs();
    for (const v of Object.values(this.sessions)) v.running = false;
    await c?.close().catch(() => {});
  }

  private onClosed(client: AcpClient, reason?: string): void {
    if (this.client !== client) return; // closed on purpose
    this.client = null;
    this.status = 'lost';
    this.error = reason
      ? `Lost connection to the agent: ${reason}`
      : 'Lost connection to the agent. It may have crashed or exited.';
    this.cancelDialogs();
    for (const v of Object.values(this.sessions)) {
      v.running = false;
      v.queue = [];
    }
  }

  private cancelDialogs(sessionId?: string): void {
    for (const p of this.permissions) {
      if (!sessionId || p.sessionId === sessionId) p.resolve({ outcome: { outcome: 'cancelled' } });
    }
    for (const q of this.questions) {
      if (!sessionId || q.sessionId === sessionId) q.resolve({ answers: [], cancelled: true });
    }
  }

  // ---- sessions ---------------------------------------------------------------------

  private onUpdate(n: SessionNotification): void {
    if (this.closed.has(n.sessionId)) return;
    const v = this.sessions[n.sessionId] ?? (this.sessions[n.sessionId] = newView(n.sessionId));
    applyUpdate(v.model, n.update);
  }

  private applySetup(v: SessionView, res: SessionSetup): void {
    if (res.modes) {
      v.model.modes = res.modes.availableModes.map((m) => ({
        id: m.id,
        name: m.name,
        description: m.description ?? undefined
      }));
      v.model.currentModeId = res.modes.currentModeId;
    }
    if (res.configOptions) setConfigOptions(v.model, res.configOptions);
  }

  async refreshSessions(): Promise<void> {
    if (!this.client) return;
    try {
      const res = await this.client.listSessions(this.cwd);
      this.stored = res.sessions.map((s) => {
        const meta = (s._meta as { ruckcode?: { mode?: string; firstPrompt?: string } } | null)
          ?.ruckcode;
        return {
          id: s.sessionId,
          title: s.title || meta?.firstPrompt || 'Untitled session',
          updatedAt: s.updatedAt ?? null,
          mode: meta?.mode ?? null
        };
      });
    } catch {
      // the agent may not support session/list; the sidebar then shows open sessions only
    }
  }

  async newSession(): Promise<void> {
    if (!this.client) return;
    try {
      const res = await this.client.newSession(this.cwd);
      const v =
        this.sessions[res.sessionId] ?? (this.sessions[res.sessionId] = newView(res.sessionId));
      this.applySetup(v, res);
      this.activeId = res.sessionId;
    } catch (e) {
      this.fail(e);
    }
  }

  /** Show a session: switch to it if open, else load it from storage (replaying history). */
  async openSession(id: string): Promise<void> {
    if (!this.client) return;
    this.closed.delete(id);
    if (this.sessions[id]) {
      this.activeId = id;
      return;
    }
    const previous = this.activeId;
    const v = newView(id);
    v.loading = true;
    this.sessions[id] = v;
    this.activeId = id;
    try {
      const res = this.canLoad
        ? await this.client.loadSession(id, this.cwd)
        : await this.client.resumeSession(id, this.cwd);
      this.applySetup(this.sessions[id], res);
      const title = this.stored.find((s) => s.id === id)?.title;
      if (title && !this.sessions[id].model.title) this.sessions[id].model.title = title;
    } catch (e) {
      delete this.sessions[id];
      this.activeId = previous;
      this.fail(e);
    } finally {
      if (this.sessions[id]) this.sessions[id].loading = false;
    }
  }

  async closeSession(id: string): Promise<void> {
    const v = this.sessions[id];
    if (!v) return;
    this.cancelDialogs(id);
    this.closed.add(id);
    delete this.sessions[id];
    if (this.activeId === id) this.activeId = Object.keys(this.sessions).at(-1) ?? null;
    try {
      await this.client?.closeSession(id);
    } catch (e) {
      this.fail(e);
    }
    await this.refreshSessions();
  }

  // ---- prompting --------------------------------------------------------------------

  /** Send a prompt, or queue it when a turn is running (like the TUI). */
  async send(text: string): Promise<void> {
    const v = this.active;
    if (!v || !this.client || !text.trim() || v.loading) return;
    this.history = [...this.history.filter((h) => h !== text), text];
    if (v.running) {
      v.queue.push(text);
      return;
    }
    await this.runTurn(v, text);
  }

  unqueue(index: number): void {
    this.active?.queue.splice(index, 1);
  }

  private async runTurn(v: SessionView, text: string): Promise<void> {
    const client = this.client;
    if (!client) return;
    if (/^\/clear\s*$/.test(text.trim())) {
      // The agent forgets the history; so does the view.
      v.model.items = [];
      v.model.plan = [];
    } else {
      v.model.items.push({ kind: 'user', text });
    }
    v.running = true;
    try {
      const res = await client.prompt(v.id, text);
      pushTurnEnd(v.model, res.stopReason);
    } catch (e) {
      if (this.client === client) {
        const msg = e instanceof Error ? e.message : String(e);
        v.model.items.push({ kind: 'notice', severity: 'error', title: 'Error', description: msg });
        pushTurnEnd(v.model, 'error');
      }
    } finally {
      v.running = false;
    }
    if (this.client !== client) return;
    const next = v.queue.shift();
    if (next !== undefined && this.sessions[v.id]) await this.runTurn(v, next);
    else void this.refreshSessions();
  }

  async cancel(): Promise<void> {
    const v = this.active;
    if (!this.client || !v || !v.running) return;
    // Unblock any dialog the agent is waiting on, then tell it to stop.
    this.cancelDialogs(v.id);
    await this.client.cancel(v.id).catch((e) => this.fail(e));
  }

  async setMode(modeId: string): Promise<void> {
    const v = this.active;
    if (!this.client || !v) return;
    try {
      await this.client.setMode(v.id, modeId);
    } catch (e) {
      this.fail(e);
    }
  }

  async setConfig(configId: string, value: string): Promise<void> {
    const v = this.active;
    if (!this.client || !v) return;
    try {
      const res = await this.client.setConfigOption(v.id, configId, value);
      if (res.configOptions) setConfigOptions(v.model, res.configOptions);
    } catch (e) {
      this.fail(e);
    }
  }

  dismissError(): void {
    this.error = '';
  }

  private fail(e: unknown): void {
    this.error = e instanceof Error ? e.message : String(e);
  }
}
