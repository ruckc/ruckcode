/**
 * Mode/hook editor: types (mirroring the JSON of `ruckcode_core::modes_api`, camelCase) and the API
 * the UI uses. Implementations: Tauri commands (local core, or `rc modes ... --json` inside WSL)
 * and an in-memory fake for browser mock mode.
 */
import type { InitInput, InitResult, SetupTarget } from './setup';

export type OnFailure = 'ignore' | 'block' | 'fix_turn';
export type PermissionValue = 'allow' | 'ask' | 'deny';

export const HOOK_EVENT_NAMES = [
  'session_start',
  'user_prompt_submit',
  'pre_tool_use',
  'post_tool_use',
  'post_edit',
  'mode_change',
  'stop'
] as const;
export type HookEventName = (typeof HOOK_EVENT_NAMES)[number];

export const DEFAULT_MAX_FIX_TURNS = 2;
export const DEFAULT_TIMEOUT_SECS = 120;

export interface HookDto {
  run: string;
  glob: string | null;
  matcher: string | null;
  onFailure: OnFailure;
  maxFixTurns: number;
  timeoutSecs: number;
}

export interface ModeDto {
  id: string;
  /** The id in the file on disk (set for existing modes); a different `id` renames the mode. */
  originalId?: string;
  name: string | null;
  description: string;
  prompt: string;
  /** null: every built-in tool. */
  tools: string[] | null;
  miseTasks: string[];
  mcpTools: string[];
  next: string[];
  autoAdvanceTo: string[];
  profile: string | null;
  model: string | null;
  permissions: Record<string, PermissionValue>;
  hooks: Partial<Record<HookEventName, HookDto[]>>;
}

export interface ModesDoc {
  defaultMode: string;
  modes: ModeDto[];
}

export interface Issue {
  /** `defaultMode`, `modes`, `modes[2].id`, `modes[2].hooks.post_edit[0].matcher`, ... */
  path: string;
  message: string;
  severity: 'error' | 'warning' | 'info';
}

export interface ToolInfo {
  name: string;
  kind: string;
  readOnly: boolean;
}

export interface HookEventInfo {
  name: HookEventName;
  /** `glob` applies (post_edit). */
  glob: boolean;
  /** `matcher` applies (pre/post_tool_use). */
  matcher: boolean;
  description: string;
}

export interface TemplateInfo {
  set: 'full' | 'minimal';
  mode: ModeDto;
}

export interface Catalog {
  builtinTools: ToolInfo[];
  miseTasks: { name: string; description: string }[];
  mcpServers: string[];
  profiles: { name: string; models: string[] }[];
  defaultProfile: string | null;
  hookEvents: HookEventInfo[];
  permissionValues: PermissionValue[];
  onFailureValues: OnFailure[];
  templates: TemplateInfo[];
}

export interface ModesView {
  status: 'missing' | 'broken' | 'ready';
  path: string;
  error?: string;
  defaultMode?: string;
  modes: ModeDto[];
  /** Merged `[hooks]` of config.toml (display only). */
  globalHooks?: Partial<Record<HookEventName, HookDto[]>>;
  catalog: Catalog;
}

export interface ModesPreview {
  /** Unified diff of modes.toml (empty when nothing changes). */
  diff: string;
  changed: boolean;
  /** Modes written out fresh because their layout could not be edited in place. */
  rewritten: string[];
}

export interface ModesApi {
  get(t: SetupTarget): Promise<ModesView>;
  validate(t: SetupTarget, doc: ModesDoc): Promise<Issue[]>;
  preview(t: SetupTarget, doc: ModesDoc): Promise<ModesPreview>;
  apply(t: SetupTarget, doc: ModesDoc): Promise<{ path: string; changed: boolean }>;
  /** `rc init`: create modes.toml from the templates and the project's mise tasks. */
  initModes(t: SetupTarget, opts?: InitInput): Promise<InitResult>;
}
