import type {
  SessionConfigOption,
  SessionUpdate,
  StopReason,
  ToolCallContent
} from '@agentclientprotocol/sdk';

export type ToolStatus = 'pending' | 'in_progress' | 'completed' | 'failed';

export interface DiffContent {
  path: string;
  /** `null` for a new file. */
  oldText: string | null;
  newText: string;
}

export interface ToolLocation {
  path: string;
  line?: number | null;
}

export type ToolItem = {
  kind: 'tool';
  id: string;
  title: string;
  toolKind: string;
  status: ToolStatus;
  rawInput: unknown;
  locations: ToolLocation[];
  /** Plain text output (text content blocks joined). */
  output: string;
  diffs: DiffContent[];
};

export type ChatItem =
  | { kind: 'user'; text: string }
  | { kind: 'assistant'; text: string }
  | { kind: 'thought'; text: string }
  | ToolItem
  | { kind: 'notice'; severity: string; title: string; description: string }
  | { kind: 'turn_end'; stopReason: string };

export interface PlanEntry {
  content: string;
  status: string;
  priority: string;
}

export interface ModeInfo {
  id: string;
  name: string;
  description?: string;
}

export interface ConfigChoice {
  value: string;
  name: string;
  description?: string;
}

/** A select-type session config option, flattened for the UI. */
export interface ConfigOptionView {
  id: string;
  name: string;
  category: string | null;
  currentValue: string;
  options: ConfigChoice[];
}

export interface ChatModel {
  items: ChatItem[];
  plan: PlanEntry[];
  modes: ModeInfo[];
  currentModeId: string | null;
  usage: { used: number; size: number } | null;
  /** Config option ids to current values (e.g. model). */
  config: Record<string, string>;
  configOptions: ConfigOptionView[];
  title: string | null;
  /** Slash commands the agent handles (`/clear`, `/compact`, ...). */
  commands: CommandInfo[];
}

export interface CommandInfo {
  name: string;
  description: string;
  /** Hint for the free-text argument, when the command takes one. */
  hint: string | null;
}

export function emptyModel(): ChatModel {
  return {
    items: [],
    plan: [],
    modes: [],
    currentModeId: null,
    usage: null,
    config: {},
    configOptions: [],
    title: null,
    commands: []
  };
}

/** Split tool content into text output and diffs. */
function splitContent(content: ToolCallContent[] | null | undefined): {
  output: string;
  diffs: DiffContent[];
} {
  const texts: string[] = [];
  const diffs: DiffContent[] = [];
  for (const c of content ?? []) {
    if (c.type === 'content' && c.content.type === 'text') texts.push(c.content.text);
    else if (c.type === 'diff') {
      diffs.push({ path: c.path, oldText: c.oldText ?? null, newText: c.newText });
    } else if (c.type === 'terminal') texts.push(`[terminal ${c.terminalId}]`);
  }
  return { output: texts.join('\n'), diffs };
}

/** Flatten ACP config options (select, possibly grouped) into the shape the UI renders. */
export function viewConfigOptions(opts: SessionConfigOption[] | null | undefined) {
  const out: ConfigOptionView[] = [];
  for (const o of opts ?? []) {
    if (o.type !== 'select') continue;
    const choices: ConfigChoice[] = [];
    for (const c of o.options as unknown as Record<string, unknown>[]) {
      const nested = c.options as Record<string, unknown>[] | undefined;
      for (const x of nested ?? [c]) {
        choices.push({
          value: String(x.value),
          name: String(x.name ?? x.value),
          description: x.description ? String(x.description) : undefined
        });
      }
    }
    out.push({
      id: o.id,
      name: o.name,
      category: o.category ?? null,
      currentValue: String(o.currentValue),
      options: choices
    });
  }
  return out;
}

/** Replace the model's config options (from a session response or a `config_option_update`). */
export function setConfigOptions(m: ChatModel, opts: SessionConfigOption[] | null | undefined) {
  m.configOptions = viewConfigOptions(opts);
  m.config = Object.fromEntries(m.configOptions.map((o) => [o.id, o.currentValue]));
}

/** Append streamed text to the trailing item of the same kind, or start a new one. */
function appendText(items: ChatItem[], kind: 'assistant' | 'thought' | 'user', text: string) {
  const last = items[items.length - 1];
  if (last && last.kind === kind) last.text += text;
  else items.push({ kind, text });
}

export function pushTurnEnd(m: ChatModel, stopReason: StopReason | string): void {
  m.items.push({ kind: 'turn_end', stopReason });
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
    case 'tool_call': {
      const { output, diffs } = splitContent(u.content);
      m.items.push({
        kind: 'tool',
        id: u.toolCallId,
        title: u.title,
        toolKind: u.kind ?? 'other',
        status: u.status ?? 'pending',
        rawInput: u.rawInput ?? null,
        locations: (u.locations ?? []).map((l) => ({ path: l.path, line: l.line })),
        output,
        diffs
      });
      break;
    }
    case 'tool_call_update': {
      const t = m.items.find((i) => i.kind === 'tool' && i.id === u.toolCallId);
      if (t && t.kind === 'tool') {
        if (u.title) t.title = u.title;
        if (u.kind) t.toolKind = u.kind;
        if (u.status) t.status = u.status;
        if (u.rawInput !== undefined && u.rawInput !== null) t.rawInput = u.rawInput;
        if (u.locations) t.locations = u.locations.map((l) => ({ path: l.path, line: l.line }));
        // A present `content` replaces what is shown; absent means unchanged.
        if (u.content) {
          const { output, diffs } = splitContent(u.content);
          t.output = output;
          t.diffs = diffs;
        }
      }
      break;
    }
    case 'plan':
      m.plan = u.entries.map((e) => ({
        content: e.content,
        status: e.status,
        priority: e.priority
      }));
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
      setConfigOptions(m, u.configOptions);
      break;
    case 'available_commands_update':
      m.commands = (u.availableCommands ?? []).map((c) => ({
        name: c.name,
        description: c.description,
        hint: c.input && 'hint' in c.input ? c.input.hint : null
      }));
      break;
    case 'session_info_update':
      if (typeof u.title === 'string') m.title = u.title;
      break;
    default:
      break;
  }
}
