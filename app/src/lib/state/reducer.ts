import type { SessionUpdate, ToolCallContent } from '@agentclientprotocol/sdk';

export type ToolStatus = 'pending' | 'in_progress' | 'completed' | 'failed';

export type ChatItem =
  | { kind: 'user'; text: string }
  | { kind: 'assistant'; text: string }
  | { kind: 'thought'; text: string }
  | {
      kind: 'tool';
      id: string;
      title: string;
      toolKind: string;
      status: ToolStatus;
      output: string;
    }
  | { kind: 'notice'; severity: string; title: string; description: string };

export interface PlanEntry {
  content: string;
  status: string;
}

export interface ChatModel {
  items: ChatItem[];
  plan: PlanEntry[];
  currentModeId: string | null;
  usage: { used: number; size: number } | null;
  /** Config option ids to current values (e.g. model). */
  config: Record<string, string>;
}

export function emptyModel(): ChatModel {
  return { items: [], plan: [], currentModeId: null, usage: null, config: {} };
}

function contentText(content: ToolCallContent[] | null | undefined): string {
  if (!content) return '';
  return content
    .map((c) => {
      if (c.type === 'content' && c.content.type === 'text') return c.content.text;
      if (c.type === 'diff') return `diff ${c.path}`;
      if (c.type === 'terminal') return `terminal ${c.terminalId}`;
      return '';
    })
    .filter(Boolean)
    .join('\n');
}

/** Append streamed text to the trailing item of the same kind, or start a new one. */
function appendText(items: ChatItem[], kind: 'assistant' | 'thought' | 'user', text: string) {
  const last = items[items.length - 1];
  if (last && last.kind === kind) last.text += text;
  else items.push({ kind, text });
}

/** Apply one `session/update` to the model (mutates; safe on a Svelte `$state` proxy). */
export function applyUpdate(m: ChatModel, u: SessionUpdate): void {
  switch (u.sessionUpdate) {
    case 'agent_message_chunk':
      if (u.content.type === 'text') appendText(m.items, 'assistant', u.content.text);
      break;
    case 'agent_thought_chunk':
      if (u.content.type === 'text') appendText(m.items, 'thought', u.content.text);
      break;
    case 'user_message_chunk':
      if (u.content.type === 'text') appendText(m.items, 'user', u.content.text);
      break;
    case 'tool_call':
      m.items.push({
        kind: 'tool',
        id: u.toolCallId,
        title: u.title,
        toolKind: u.kind ?? 'other',
        status: u.status ?? 'pending',
        output: contentText(u.content)
      });
      break;
    case 'tool_call_update': {
      const t = m.items.find((i) => i.kind === 'tool' && i.id === u.toolCallId);
      if (t && t.kind === 'tool') {
        if (u.title) t.title = u.title;
        if (u.kind) t.toolKind = u.kind;
        if (u.status) t.status = u.status;
        // A present `content` replaces what is shown; absent means unchanged.
        if (u.content) t.output = contentText(u.content);
      }
      break;
    }
    case 'plan':
      m.plan = u.entries.map((e) => ({ content: e.content, status: e.status }));
      break;
    case 'current_mode_update':
      m.currentModeId = u.currentModeId;
      break;
    case 'usage_update':
      m.usage = { used: u.used, size: u.size };
      break;
    case 'notice':
      m.items.push({
        kind: 'notice',
        severity: u.severity,
        title: u.title,
        description: u.description ?? ''
      });
      break;
    case 'config_option_update':
      for (const o of u.configOptions) {
        if ('currentValue' in o && typeof o.currentValue === 'string') {
          m.config[o.id] = o.currentValue;
        }
      }
      break;
    default:
      break;
  }
}
