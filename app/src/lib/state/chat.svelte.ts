import type {
  RequestPermissionRequest,
  RequestPermissionResponse,
  SessionNotification
} from '@agentclientprotocol/sdk';
import { AcpClient } from '../acp/client';
import type { AskUserQuestionRequest, AskUserQuestionResult } from '../acp/ext';
import { connectHost, type ConnectOptions } from '../host';
import { applyUpdate, emptyModel, type ChatModel } from './reducer';

export type Status = 'disconnected' | 'connecting' | 'ready' | 'busy';

export interface PendingPermission {
  request: RequestPermissionRequest;
  resolve(r: RequestPermissionResponse): void;
}

export interface PendingQuestion {
  request: AskUserQuestionRequest;
  resolve(r: AskUserQuestionResult): void;
}

/** UI-facing state for one agent connection and one session. */
export class ChatController {
  model = $state<ChatModel>(emptyModel());
  status = $state<Status>('disconnected');
  error = $state('');
  sessionId = $state<string | null>(null);
  agentName = $state('');
  modes = $state<{ id: string; name: string }[]>([]);
  permission = $state.raw<PendingPermission | null>(null);
  question = $state.raw<PendingQuestion | null>(null);

  private client: AcpClient | null = null;

  async connect(opts: ConnectOptions): Promise<void> {
    this.error = '';
    this.status = 'connecting';
    try {
      await this.disconnect();
      this.model = emptyModel();
      const transport = await connectHost(opts);
      const client = new AcpClient(transport, {
        onSessionUpdate: (n: SessionNotification) => applyUpdate(this.model, n.update),
        requestPermission: (request) =>
          new Promise((resolve) => {
            this.permission = {
              request,
              resolve: (r) => {
                this.permission = null;
                resolve(r);
              }
            };
          }),
        askUserQuestion: (request) =>
          new Promise((resolve) => {
            this.question = {
              request,
              resolve: (r) => {
                this.question = null;
                resolve(r);
              }
            };
          })
      });
      this.client = client;
      const init = await client.initialize();
      this.agentName = init.agentInfo?.name ?? 'agent';
      const session = await client.newSession(opts.cwd);
      this.sessionId = session.sessionId;
      this.modes = (session.modes?.availableModes ?? []).map((m) => ({ id: m.id, name: m.name }));
      this.model.currentModeId = session.modes?.currentModeId ?? null;
      this.status = 'ready';
    } catch (e) {
      this.fail(e);
      await this.disconnect();
    }
  }

  async send(text: string): Promise<void> {
    const client = this.client;
    const sid = this.sessionId;
    if (!client || !sid || this.status !== 'ready' || !text.trim()) return;
    this.model.items.push({ kind: 'user', text });
    this.status = 'busy';
    try {
      await client.prompt(sid, text);
    } catch (e) {
      this.fail(e);
    } finally {
      if (this.status === 'busy') this.status = 'ready';
    }
  }

  async cancel(): Promise<void> {
    if (!this.client || !this.sessionId) return;
    // Unblock any dialog the agent is waiting on, then tell it to stop.
    this.permission?.resolve({ outcome: { outcome: 'cancelled' } });
    this.question?.resolve({ answers: [], cancelled: true });
    await this.client.cancel(this.sessionId).catch((e) => this.fail(e));
  }

  async setMode(modeId: string): Promise<void> {
    if (!this.client || !this.sessionId) return;
    try {
      await this.client.setMode(this.sessionId, modeId);
    } catch (e) {
      this.fail(e);
    }
  }

  async disconnect(): Promise<void> {
    const c = this.client;
    this.client = null;
    this.permission?.resolve({ outcome: { outcome: 'cancelled' } });
    this.question?.resolve({ answers: [], cancelled: true });
    this.sessionId = null;
    if (this.status !== 'connecting') this.status = 'disconnected';
    await c?.close().catch(() => {});
  }

  private fail(e: unknown): void {
    this.error = e instanceof Error ? e.message : String(e);
    if (this.status === 'connecting') this.status = 'disconnected';
  }
}
