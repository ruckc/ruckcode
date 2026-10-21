/**
 * Setup and keys: types (mirroring the JSON of `ruckcode_core::setup_api`, camelCase) and the API
 * the UI uses. Implementations: Tauri commands (local host, or `rc ... --json` inside WSL) and an
 * in-memory fake for browser mock mode. Secrets only go *into* `listModels`, `apply`/`preview`
 * (`plan.newKey.secret`) and `keysAdd`; nothing here ever returns one.
 */

/** A quick-fill endpoint URL (`id` doubles as the suggested profile name). */
export interface EndpointSuggestion {
  id: string;
  label: string;
  url: string;
  /** The API flavor this endpoint speaks. */
  kind: ApiKind;
}

/** `anthropic` (Messages API) or `openai` (any OpenAI-compatible server). */
export type ApiKind = 'anthropic' | 'openai';

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
  modes: ModesState;
}

export interface ModelView {
  id: string;
  displayName: string | null;
}

export type ModelsAuth = { keyName: string } | { secret: string };

export interface ProbeAttempt {
  kind: ApiKind;
  url: string;
  status: number | null;
  error: string | null;
}

/** Result of probing an endpoint: the detected API flavor, its models and an explanation. */
export interface ProbeResult {
  kind: ApiKind | null;
  models: ModelView[];
  attempts: ProbeAttempt[];
  /** e.g. `Anthropic API ✓ (3 models)` or what went wrong and how to fix it. */
  message: string;
}

export interface PlanInput {
  profileName: string;
  provider: ApiKind;
  baseUrl: string | null;
  model: string;
  models: string[];
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
  /** The quick-fill endpoint suggestions. */
  presets(): Promise<EndpointSuggestion[]>;
  state(t: SetupTarget): Promise<SetupState>;
  listModels(
    t: SetupTarget,
    req: { kind: string; baseUrl: string; auth?: ModelsAuth }
  ): Promise<ModelView[]>;
  /**
   * Check `baseUrl` with the key and detect the API flavor (Anthropic Messages or
   * OpenAI-compatible); `force` checks only that flavor. Resolves with `kind: null` and an
   * explanation when nothing answered.
   */
  probe(
    t: SetupTarget,
    req: { baseUrl: string; force?: ApiKind; auth?: ModelsAuth }
  ): Promise<ProbeResult>;
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
export const PROFILE_NAME_RE = /^[A-Za-z0-9._-]+$/;

/**
 * Validate an endpoint typed by the user (mirrors `setup::validate_endpoint`): absolute http(s)
 * URL with a host and no spaces; returned trimmed and without trailing slashes.
 */
export function validateEndpoint(raw: string): { url: string } | { error: string } {
  const u = raw.trim();
  if (!u) return { error: 'The endpoint URL is required.' };
  if (!/^https?:\/\//i.test(u))
    return { error: 'The endpoint must start with http:// or https://.' };
  if (/\s/.test(u)) return { error: 'The endpoint must not contain spaces.' };
  const host = hostAndPort(u).host;
  if (!host) return { error: 'The endpoint needs a host, e.g. https://example.com/v1.' };
  return { url: u.replace(/\/+$/, '') };
}

export function hostAndPort(url: string): { host: string; port: string } {
  const rest = url.replace(/^[a-z]+:\/\//i, '');
  const authority = rest.split(/[/?#]/)[0].split('@').pop() ?? '';
  const m = /^\[([^\]]*)\](?::(\d+))?$/.exec(authority) ?? /^([^:]*)(?::(\d+))?$/.exec(authority);
  return { host: (m?.[1] ?? '').toLowerCase(), port: m?.[2] ?? '' };
}

const LOCAL_HOSTS = ['localhost', '127.0.0.1', '0.0.0.0', '::1'];

/** True for loopback, private-range and `*.local` hosts (these usually need no API key). */
export function isLocalEndpoint(url: string): boolean {
  const { host } = hostAndPort(url);
  if (LOCAL_HOSTS.includes(host) || host.endsWith('.local') || host.startsWith('127.')) return true;
  const o = host.split('.').map(Number);
  if (o.length !== 4 || o.some((n) => !Number.isInteger(n))) return false;
  return o[0] === 10 || (o[0] === 192 && o[1] === 168) || (o[0] === 172 && o[1] >= 16 && o[1] < 32);
}

/** The suggestion an endpoint points at (same host; local servers: same port). */
export function suggestionForUrl(
  suggestions: EndpointSuggestion[],
  url: string
): EndpointSuggestion | undefined {
  const a = hostAndPort(url);
  return suggestions.find((s) => {
    const b = hostAndPort(s.url);
    return LOCAL_HOSTS.includes(b.host)
      ? LOCAL_HOSTS.includes(a.host) && a.port === b.port
      : a.host === b.host;
  });
}

/** The endpoint hinted by the shape of an API key (a suggestion only). */
export function suggestionForKey(
  suggestions: EndpointSuggestion[],
  key: string
): EndpointSuggestion | undefined {
  const id = key.startsWith('sk-ant-')
    ? 'anthropic'
    : key.startsWith('sk-or-')
      ? 'openrouter'
      : key.startsWith('sk-')
        ? 'openai'
        : null;
  return id ? suggestions.find((s) => s.id === id) : undefined;
}

/** Default profile name: anthropic, openai, openrouter, ollama, lmstudio, vllm or the hostname. */
export function suggestedProfileName(
  suggestions: EndpointSuggestion[],
  kind: ApiKind,
  url: string
): string {
  const { host } = hostAndPort(url);
  if (kind === 'anthropic' && (!host || host === 'api.anthropic.com')) return 'anthropic';
  if (kind !== 'anthropic') {
    const s = suggestionForUrl(suggestions, url);
    if (s) return s.id;
  }
  const name = host.replace(/[^a-z0-9]/g, '-').replace(/^-+|-+$/g, '');
  return name || (kind === 'anthropic' ? 'anthropic' : 'openai-compatible');
}

/** True when the user config has a usable profile (mirrors `needs_onboarding`). */
export function usable(state: SetupState): boolean {
  return !state.needsOnboarding;
}
