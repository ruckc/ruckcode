/**
 * Setup and keys: types (mirroring the JSON of `ruckcode_core::setup_api`, camelCase) and the API
 * the UI uses. Implementations: Tauri commands (local host, or `rc ... --json` inside WSL) and an
 * in-memory fake for browser mock mode. Secrets only go *into* `listModels`, `apply`/`preview`
 * (`plan.newKey.secret`) and `keysAdd`; nothing here ever returns one.
 */

export interface PresetInfo {
  id: string;
  label: string;
  kind: 'anthropic' | 'openai';
  /** null: the user must enter a URL. */
  baseUrl: string | null;
  apiKeyEnv: string | null;
}

export interface ProfileInfo {
  name: string;
  provider: string;
  model: string;
  models: string[];
  baseUrl: string | null;
  apiKeyEnv: string | null;
  apiKeyName: string | null;
}

export interface KeyView {
  name: string;
  description: string | null;
  createdAt: string | null;
  /** `…` plus the last four characters. */
  masked: string;
  usedBy: string[];
}

export interface KeysList {
  keys: KeyView[];
  missing: { name: string; usedBy: string }[];
}

export interface ModeInfo {
  id: string;
  description: string;
  profile: string | null;
  model: string | null;
}

export type ModesState =
  | { status: 'missing' }
  | { status: 'broken'; error: string }
  | { status: 'ready'; defaultMode: string; modes: ModeInfo[] };

export interface SetupState {
  needsOnboarding: boolean;
  project: string;
  defaultProfile: string;
  profiles: ProfileInfo[];
  keys: KeysList;
  /** Presence of every env var that may hold a key. */
  env: Record<string, boolean>;
  modes: ModesState;
}

export interface ModelView {
  id: string;
  displayName: string | null;
}

export type ModelsAuth = { env: string } | { keyName: string } | { secret: string };

export interface PlanInput {
  profileName: string;
  provider: 'anthropic' | 'openai';
  baseUrl: string | null;
  model: string;
  models: string[];
  apiKeyEnv: string | null;
  apiKeyName: string | null;
  newKey: { name: string; secret: string; description: string | null } | null;
  keyScope: 'user' | 'project';
  setDefault: boolean;
  /** `profile`/`model` null: the mode uses the profile default. */
  modeModels: { mode: string; profile: string | null; model: string | null }[];
}

export interface ReportView {
  configPath: string;
  projectConfigPath: string | null;
  credentialsPath: string | null;
  modesPath: string | null;
  modesUpdated: number;
}

export interface InitInput {
  minimal?: boolean;
  modes?: string[];
  force?: boolean;
}

export interface InitResult {
  path: string;
  tasksDiscovered: number;
  modes: ModesState;
}

/** The project a request is about: a local path, or a Linux path inside `distro`. */
export interface SetupTarget {
  project: string;
  distro?: string;
}

export interface SetupApi {
  presets(): Promise<PresetInfo[]>;
  state(t: SetupTarget): Promise<SetupState>;
  listModels(
    t: SetupTarget,
    req: { kind: string; baseUrl: string; auth?: ModelsAuth }
  ): Promise<ModelView[]>;
  preview(t: SetupTarget, plan: PlanInput): Promise<{ diff: string }>;
  apply(t: SetupTarget, plan: PlanInput): Promise<ReportView>;
  keysList(t: SetupTarget): Promise<KeysList>;
  keysAdd(t: SetupTarget, name: string, secret: string, description?: string): Promise<void>;
  keysRemove(t: SetupTarget, name: string): Promise<{ stillUsedBy: string[] }>;
  keysUse(
    t: SetupTarget,
    name: string,
    profile: string | null,
    projectScope: boolean
  ): Promise<void>;
  initModes(t: SetupTarget, opts?: InitInput): Promise<InitResult>;
}

export const KEY_NAME_RE = /^[A-Za-z0-9._-]+$/;
export const ENV_NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;
export const PROFILE_NAME_RE = /^[A-Za-z0-9._-]+$/;

/** True when the user config has a usable profile (mirrors `needs_onboarding`). */
export function usable(state: SetupState): boolean {
  return !state.needsOnboarding;
}
