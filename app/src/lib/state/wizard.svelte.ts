import {
  ENV_NAME_RE,
  KEY_NAME_RE,
  PROFILE_NAME_RE,
  type ModelView,
  type ModelsAuth,
  type PlanInput,
  type PresetInfo,
  type ReportView,
  type SetupApi,
  type SetupState,
  type SetupTarget
} from '../host/setup';

export type Step =
  'provider' | 'endpoint' | 'auth' | 'profile' | 'models' | 'modes' | 'review' | 'done';
export const STEPS: Step[] = [
  'provider',
  'endpoint',
  'auth',
  'profile',
  'models',
  'modes',
  'review'
];
export const STEP_LABEL: Record<Step, string> = {
  provider: 'Provider',
  endpoint: 'Endpoint',
  auth: 'Authentication',
  profile: 'Profile',
  models: 'Models',
  modes: 'Modes',
  review: 'Review',
  done: 'Done'
};

export type SettingsTab = 'providers' | 'keys' | 'project';

export type AuthKind = 'env' | 'existing' | 'new' | 'none';
export type ModelsPhase = 'idle' | 'loading' | 'ready' | 'error';

/** Value of a mode's model dropdown: `''` is "profile default", else `profile::model`. */
export const DEFAULT_CHOICE = '';
export const choiceValue = (profile: string, model: string) => `${profile}::${model}`;
export function parseChoice(v: string): { profile: string; model: string } | null {
  const i = v.indexOf('::');
  return v && i >= 0 ? { profile: v.slice(0, i), model: v.slice(i + 2) } : null;
}

export function isHttpUrl(s: string): boolean {
  try {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- only parsed, never reactive
    const u = new URL(s.trim());
    return u.protocol === 'http:' || u.protocol === 'https:';
  } catch {
    return false;
  }
}

export interface ModeRow {
  id: string;
  description: string;
  /** The value the dropdown starts with (the mode's current setting). */
  initial: string;
}

/**
 * The setup wizard's state machine (steps mirror the TUI). Nothing here touches the DOM, and the
 * new key's secret is wiped from state as soon as it has been saved (`clearSecrets`).
 */
export class SetupWizard {
  readonly api: SetupApi;
  readonly target: SetupTarget;
  readonly presets: PresetInfo[];
  state = $state<SetupState>() as SetupState;

  step = $state<Step>('provider');
  busy = $state(false);
  error = $state<string | null>(null);

  preset = $state<PresetInfo | null>(null);
  endpoint = $state('');
  authKind = $state<AuthKind>('env');
  envName = $state('');
  existingKey = $state('');
  newKeyName = $state('');
  newKeySecret = $state('');
  newKeyDescription = $state('');
  keyScope = $state<'user' | 'project'>('user');
  profileName = $state('');
  overwrite = $state(false);

  modelsPhase = $state<ModelsPhase>('idle');
  modelsError = $state<string | null>(null);
  fetched = $state<ModelView[]>([]);
  manualModels = $state<string[]>([]);
  selected = $state<string[]>([]);
  defaultModel = $state('');
  filter = $state('');
  manualText = $state('');

  choices = $state<Record<string, string>>({});
  initializing = $state(false);
  setDefault = $state(true);

  diff = $state('');
  diffError = $state<string | null>(null);
  report = $state<ReportView | null>(null);

  private fetchKey = '';
  private generation = 0;

  constructor(api: SetupApi, target: SetupTarget, presets: PresetInfo[], state: SetupState) {
    this.api = api;
    this.target = target;
    this.presets = presets;
    this.state = state;
    this.setDefault = state.profiles.length === 0 || state.needsOnboarding;
    this.resetChoices();
  }

  // ---- derived -------------------------------------------------------------------------

  get stepIndex() {
    return STEPS.indexOf(this.step);
  }

  get profileExists() {
    return this.state.profiles.some((p) => p.name === this.profileName.trim());
  }

  get keyNames() {
    return this.state.keys.keys.map((k) => k.name);
  }

  get envSet(): boolean {
    return !!this.state.env[this.envName.trim()];
  }

  /** All models the user can pick from: fetched plus manually added. */
  get available(): string[] {
    const ids = this.fetched.map((m) => m.id);
    for (const m of this.manualModels) if (!ids.includes(m)) ids.push(m);
    return ids;
  }

  get filtered(): string[] {
    const q = this.filter.trim().toLowerCase();
    return q ? this.available.filter((m) => m.toLowerCase().includes(q)) : this.available;
  }

  get modeRows(): ModeRow[] {
    const m = this.state.modes;
    if (m.status !== 'ready') return [];
    return m.modes.map((x) => ({
      id: x.id,
      description: x.description,
      initial: x.model ? choiceValue(x.profile ?? '', x.model) : DEFAULT_CHOICE
    }));
  }

  /** Every profile's models (existing profiles, with this one replacing a same-named one). */
  get choiceGroups(): { profile: string; models: string[] }[] {
    const name = this.profileName.trim();
    const groups = this.state.profiles
      .filter((p) => p.name !== name)
      .map((p) => ({
        profile: p.name,
        models: [p.model, ...p.models].filter((m, i, a) => a.indexOf(m) === i)
      }));
    groups.push({ profile: name, models: [...this.selected] });
    return groups.filter((g) => g.models.length);
  }

  /** The error that blocks leaving the current step, or null. */
  get problem(): string | null {
    switch (this.step) {
      case 'provider':
        return this.preset ? null : 'Choose a provider.';
      case 'endpoint':
        return isHttpUrl(this.endpoint) ? null : 'Enter a full http:// or https:// URL.';
      case 'auth':
        if (this.authKind === 'env')
          return ENV_NAME_RE.test(this.envName.trim())
            ? null
            : 'Enter an environment variable name.';
        if (this.authKind === 'existing')
          return this.existingKey ? null : 'Choose a saved key (or add a new one).';
        if (this.authKind === 'new') {
          if (!KEY_NAME_RE.test(this.newKeyName.trim()))
            return 'Key name: letters, digits, ".", "-" or "_".';
          return this.newKeySecret.trim() ? null : 'Enter the API key.';
        }
        return null;
      case 'profile':
        if (!PROFILE_NAME_RE.test(this.profileName.trim()))
          return 'Profile name: letters, digits, ".", "-" or "_".';
        if (this.profileExists && !this.overwrite)
          return `Profile "${this.profileName.trim()}" exists: confirm that it should be overwritten.`;
        return null;
      case 'models':
        if (this.modelsPhase === 'loading') return 'Loading models...';
        if (!this.selected.length) return 'Select at least one model.';
        return this.selected.includes(this.defaultModel) ? null : 'Pick a default model.';
      case 'review':
        return this.diffError ?? (this.busy ? 'Saving...' : null);
      default:
        return null;
    }
  }

  get canNext() {
    return this.problem === null && !this.busy;
  }

  // ---- inputs ----------------------------------------------------------------------------

  choosePreset(id: string) {
    const p = this.presets.find((x) => x.id === id);
    if (!p) return;
    const changed = this.preset?.id !== p.id;
    this.preset = p;
    if (changed) {
      this.endpoint = p.baseUrl ?? '';
      this.envName = p.apiKeyEnv ?? '';
      this.authKind = p.apiKeyEnv ? 'env' : 'none';
      this.profileName = this.freeProfileName(p.id);
      this.overwrite = false;
      this.fetchKey = '';
    }
  }

  private freeProfileName(base: string) {
    // Re-running setup for the same provider updates that profile; offer the name as is.
    return base;
  }

  private resetChoices() {
    this.choices = {};
    for (const r of this.modeRows) this.choices[r.id] = r.initial;
  }

  setAuth(kind: AuthKind) {
    this.authKind = kind;
    if (kind === 'existing' && !this.existingKey) this.existingKey = this.keyNames[0] ?? '';
    if (kind !== 'new') this.wipeSecret();
  }

  private wipeSecret() {
    this.newKeySecret = '';
  }

  /** Wipe every secret held by the wizard (after saving, or when closing). */
  clearSecrets() {
    this.wipeSecret();
  }

  // ---- navigation ------------------------------------------------------------------------

  async next() {
    if (!this.canNext) return;
    this.error = null;
    switch (this.step) {
      case 'provider':
        this.step = 'endpoint';
        break;
      case 'endpoint':
        this.step = 'auth';
        break;
      case 'auth':
        this.step = 'profile';
        break;
      case 'profile':
        this.step = 'models';
        void this.fetchModels();
        break;
      case 'models':
        this.resetChoicesIfNeeded();
        this.step = 'modes';
        break;
      case 'modes':
        this.step = 'review';
        await this.loadPreview();
        break;
      case 'review':
        await this.apply();
        break;
      default:
    }
  }

  back() {
    this.error = null;
    const i = this.stepIndex;
    if (i > 0) this.step = STEPS[i - 1];
  }

  goto(step: Step) {
    if (STEPS.indexOf(step) < this.stepIndex) this.step = step;
  }

  private resetChoicesIfNeeded() {
    for (const r of this.modeRows) if (!(r.id in this.choices)) this.choices[r.id] = r.initial;
  }

  // ---- models ----------------------------------------------------------------------------

  private authForFetch(): ModelsAuth | undefined {
    switch (this.authKind) {
      case 'env':
        return { env: this.envName.trim() };
      case 'existing':
        return { keyName: this.existingKey };
      case 'new':
        return { secret: this.newKeySecret.trim() };
      default:
        return undefined;
    }
  }

  private currentFetchKey() {
    return [
      this.preset?.kind,
      this.endpoint.trim(),
      this.authKind,
      this.authKind === 'env'
        ? this.envName
        : this.authKind === 'existing'
          ? this.existingKey
          : this.authKind === 'new'
            ? this.newKeySecret
            : ''
    ].join('\0');
  }

  /** Fetch the live model list (skipped when nothing changed since the last successful fetch). */
  async fetchModels(force = false) {
    const key = this.currentFetchKey();
    if (!force && this.modelsPhase === 'ready' && key === this.fetchKey) return;
    const gen = ++this.generation;
    this.modelsPhase = 'loading';
    this.modelsError = null;
    try {
      const list = await this.api.listModels(this.target, {
        kind: this.preset?.kind ?? 'openai',
        baseUrl: this.endpoint.trim(),
        auth: this.authForFetch()
      });
      if (gen !== this.generation) return;
      this.fetched = list;
      this.fetchKey = key;
      this.modelsPhase = 'ready';
      this.initSelection();
    } catch (e) {
      if (gen !== this.generation) return;
      this.modelsError = e instanceof Error ? e.message : String(e);
      this.modelsPhase = 'error';
      this.fetchKey = '';
    }
  }

  private initSelection() {
    const existing = this.state.profiles.find((p) => p.name === this.profileName.trim());
    const keep = this.selected.filter((m) => this.available.includes(m));
    if (keep.length) {
      this.selected = keep;
    } else if (existing) {
      this.selected = existing.models.filter((m) => this.available.includes(m));
    }
    if (!this.selected.length && this.available.length) this.selected = [];
    if (!this.selected.includes(this.defaultModel))
      this.defaultModel =
        (existing && this.selected.includes(existing.model) ? existing.model : this.selected[0]) ??
        '';
  }

  toggleModel(id: string) {
    this.selected = this.selected.includes(id)
      ? this.selected.filter((m) => m !== id)
      : [...this.selected, id];
    if (!this.selected.includes(this.defaultModel)) this.defaultModel = this.selected[0] ?? '';
    else if (!this.defaultModel) this.defaultModel = this.selected[0] ?? '';
  }

  /** Select every model matching the filter (or deselect them all when all are selected). */
  toggleAllFiltered() {
    const f = this.filtered;
    const all = f.length > 0 && f.every((m) => this.selected.includes(m));
    this.selected = all
      ? this.selected.filter((m) => !f.includes(m))
      : [...this.selected, ...f.filter((m) => !this.selected.includes(m))];
    if (!this.selected.includes(this.defaultModel)) this.defaultModel = this.selected[0] ?? '';
  }

  /** Manual entry: comma/space/newline separated ids are added and selected. */
  addManual() {
    const ids = this.manualText
      .split(/[\s,]+/)
      .map((s) => s.trim())
      .filter(Boolean);
    for (const id of ids) {
      if (!this.available.includes(id)) this.manualModels = [...this.manualModels, id];
      if (!this.selected.includes(id)) this.selected = [...this.selected, id];
    }
    this.manualText = '';
    if (!this.defaultModel) this.defaultModel = this.selected[0] ?? '';
    if (this.modelsPhase === 'error') this.modelsPhase = 'ready';
  }

  // ---- modes -----------------------------------------------------------------------------

  async runInit() {
    this.initializing = true;
    this.error = null;
    try {
      const r = await this.api.initModes(this.target);
      this.state = { ...this.state, modes: r.modes };
      this.resetChoices();
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    } finally {
      this.initializing = false;
    }
  }

  // ---- review / save ---------------------------------------------------------------------

  /** The plan for the backend. Contains the new key's secret: pass it on, never store it. */
  plan(): PlanInput {
    const name = this.profileName.trim();
    const newKey =
      this.authKind === 'new'
        ? {
            name: this.newKeyName.trim(),
            secret: this.newKeySecret.trim(),
            description: this.newKeyDescription.trim() || null
          }
        : null;
    const modeModels = this.modeRows
      .filter((r) => (this.choices[r.id] ?? r.initial) !== r.initial)
      .map((r) => {
        const c = parseChoice(this.choices[r.id]);
        return { mode: r.id, profile: c?.profile ?? null, model: c?.model ?? null };
      });
    return {
      profileName: name,
      provider: this.preset?.kind ?? 'openai',
      baseUrl: this.endpoint.trim(),
      model: this.defaultModel,
      models: [...this.selected],
      apiKeyEnv: this.authKind === 'env' ? this.envName.trim() : null,
      apiKeyName: this.authKind === 'existing' ? this.existingKey : newKey ? newKey.name : null,
      newKey,
      keyScope: this.authKind === 'existing' || this.authKind === 'new' ? this.keyScope : 'user',
      setDefault: this.setDefault,
      modeModels
    };
  }

  async loadPreview() {
    this.diff = '';
    this.diffError = null;
    this.busy = true;
    try {
      this.diff = (await this.api.preview(this.target, this.plan())).diff;
    } catch (e) {
      this.diffError = e instanceof Error ? e.message : String(e);
    } finally {
      this.busy = false;
    }
  }

  async apply() {
    this.busy = true;
    this.error = null;
    try {
      this.report = await this.api.apply(this.target, this.plan());
      this.clearSecrets();
      this.state = await this.api.state(this.target);
      this.step = 'done';
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    } finally {
      this.busy = false;
    }
  }
}
