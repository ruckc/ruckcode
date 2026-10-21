import {
  client as createClient,
  methods,
  PROTOCOL_VERSION,
  type ClientConnection,
  type InitializeResponse,
  type ListSessionsResponse,
  type LoadSessionResponse,
  type NewSessionResponse,
  type ResumeSessionResponse,
  type SetSessionConfigOptionResponse,
  type PromptResponse,
  type RequestPermissionRequest,
  type RequestPermissionResponse,
  type SessionNotification
} from '@agentclientprotocol/sdk';
import {
  ASK_USER_QUESTION,
  CLIENT_CAPABILITIES,
  askUserQuestionRequestSchema,
  type AskUserQuestionRequest,
  type AskUserQuestionResult
} from './ext';
import { transportToStream, type LineTransport } from './transport';

/** What the UI provides to the ACP client: sinks for agent-initiated traffic. */
export interface ClientHandlers {
  onSessionUpdate(n: SessionNotification): void;
  requestPermission(req: RequestPermissionRequest): Promise<RequestPermissionResponse>;
  askUserQuestion(req: AskUserQuestionRequest): Promise<AskUserQuestionResult>;
}

export interface ClientInfo {
  name: string;
  version: string;
}

/**
 * ACP client on top of the official SDK. Owns one connection to one agent, over any
 * {@link LineTransport}.
 */
export class AcpClient {
  private connection: ClientConnection;

  constructor(
    private readonly transport: LineTransport,
    handlers: ClientHandlers,
    private readonly info: ClientInfo = { name: 'ruckcode-app', version: '0.1.0' }
  ) {
    const app = createClient({ name: info.name })
      .onNotification(methods.client.session.update, ({ params }) => {
        handlers.onSessionUpdate(params);
      })
      .onRequest(methods.client.session.requestPermission, ({ params }) =>
        handlers.requestPermission(params)
      )
      .onRequest(
        ASK_USER_QUESTION,
        (params: unknown) => askUserQuestionRequestSchema.parse(params),
        ({ params }) => handlers.askUserQuestion(params)
      );
    this.connection = app.connect(transportToStream(transport));
  }

  get closed(): Promise<void> {
    return this.connection.closed;
  }

  initialize(): Promise<InitializeResponse> {
    return this.connection.agent.request(methods.agent.initialize, {
      protocolVersion: PROTOCOL_VERSION,
      clientCapabilities: structuredClone(CLIENT_CAPABILITIES),
      clientInfo: { name: this.info.name, version: this.info.version }
    });
  }

  newSession(cwd: string): Promise<NewSessionResponse> {
    return this.connection.agent.request(methods.agent.session.new, { cwd, mcpServers: [] });
  }

  listSessions(cwd: string): Promise<ListSessionsResponse> {
    return this.connection.agent.request(methods.agent.session.list, { cwd });
  }

  /** Reopen a stored session and replay its history as `session/update` notifications. */
  loadSession(sessionId: string, cwd: string): Promise<LoadSessionResponse> {
    return this.connection.agent.request(methods.agent.session.load, {
      sessionId,
      cwd,
      mcpServers: []
    });
  }

  /** Reopen a stored session without replaying history. */
  resumeSession(sessionId: string, cwd: string): Promise<ResumeSessionResponse> {
    return this.connection.agent.request(methods.agent.session.resume, {
      sessionId,
      cwd,
      mcpServers: []
    });
  }

  closeSession(sessionId: string): Promise<unknown> {
    return this.connection.agent.request(methods.agent.session.close, { sessionId });
  }

  setConfigOption(
    sessionId: string,
    configId: string,
    value: string
  ): Promise<SetSessionConfigOptionResponse> {
    return this.connection.agent.request(methods.agent.session.setConfigOption, {
      sessionId,
      configId,
      value
    });
  }

  prompt(sessionId: string, text: string): Promise<PromptResponse> {
    return this.connection.agent.request(methods.agent.session.prompt, {
      sessionId,
      prompt: [{ type: 'text', text }]
    });
  }

  cancel(sessionId: string): Promise<void> {
    return this.connection.agent.notify(methods.agent.session.cancel, { sessionId });
  }

  setMode(sessionId: string, modeId: string): Promise<unknown> {
    return this.connection.agent.request(methods.agent.session.setMode, { sessionId, modeId });
  }

  /** Escape hatch for extension methods (`_ruckcode/*`) and tests. */
  unknown(method: string, params: unknown = {}): Promise<unknown> {
    return this.connection.agent.request(method, params);
  }

  async close(): Promise<void> {
    this.connection.close();
    await this.transport.close?.();
  }
}
