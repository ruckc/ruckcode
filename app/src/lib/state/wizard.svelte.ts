import {
  PROFILE_NAME_RE,
  isLocalEndpoint,
  suggestedProfileName,
  suggestionForUrl,
  validateEndpoint,
  type ApiKind,
  type EndpointSuggestion,
  type ModelView,
  type ModelsAuth,
  type PlanInput,
  type ProbeResult,
  type ReportView,
  type SetupApi,
  type SetupState,
  type SetupTarget
} from '../host/setup';

export type Step =
  'key' | 'endpoint' | 'check' | 'models' | 'profile' | 'modes' | 'review' | 'done';
export const STEPS: Step[] = ['key', 'endpoint', 'check', 'models', 'profile', 'modes', 'review'];
export const STEP_LABEL: Record<Step, string> = {
  key: 'API key',
  endpoint: 'Endpoint',
  check: 'Check',
  models: 'Models',
  profile: 'Profile',
  modes: 'Modes',
  review: 'Review',
  done: 'Done'
};

export type SettingsTab = 'providers' | 'keys' | 'modes' | 'project' | 'updates';

/** `new`: a pasted key (saved as a named key in the per-user credentials file on save). */
export type AuthKind = 'new' | 'existing' | 'none';
export type CheckPhase = 'idle' | 'checking' | 'ok' | 'failed';

/** Value of a mode's model dropdown: `''` is "profile default", else `profile::model`. */
export const DEFAULT_CHOICE = '';
export const choiceValue = (profile: string, model: string) => `${profile}::${model}`;
export function parseChoice(v: string): { profile: string; model: string } | null {
  const i = v.indexOf('::');
  return v && i >= 0 ? { profile: v.slice(0, i), model: v.slice(i + 2) } : null;
}

export interface ModeRow {
  id: string;
  description: string;
  /** The value the dropdown starts with (the mode's current setting). */
  initial: string;
}

/**
 * The setup wizard's state machine (steps mirror the TUI): API key, endpoint URL, a compatibility
 * check that detects the API flavor, models, profile name, per-mode models, review. The pasted
 * key's secret lives only in memory until the final save (`clearSecrets` wipes it).
 */
export class SetupWizard {
  readonly api: SetupApi;
  readonly target: SetupTarget;
  readonly suggestions: EndpointSuggestion[];
  state = $state<SetupState>() as SetupState;

  step = $state<Step>('key');
  busy = $state(false);
  error = $state<string | null>(null);

  authKind = $state<AuthKind>('new');
  existingKey = $state('');
  newKeySecret = $state('');
  newKeyDescription = $state('');
  endpoint = $state('');
  endpointTouched = $state(false);

  checkPhase = $state<CheckPhase>('idle');
  probe = $state<ProbeResult | null>(null);
  checkError = $state<string | null>(null);
  /** Manual override of the detected API flavor. */
  forced = $state<ApiKind | null>(null);
  /** Continue without a model list after a failed check. */
  manual = $state(false);

  profileName = $state('');
  profileTouched = $state(false);
  overwrite = $state(false);

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

  private generation = 0;

  constructor(
    api: SetupApi,
    target: SetupTarget,
    suggestions: EndpointSuggestion[],
    state: SetupState
  ) {
    this.api = api;
    this.target = target;
    this.suggestions = suggestions;
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

  /** The API flavor in use: the manual override, else the detected one, else a guess. */
  get kind(): ApiKind {
    return (
      this.forced ??
      this.probe?.kind ??
      suggestionForUrl(this.suggestions, this.endpoint)?.kind ??
      'openai'
    );
  }

  /** Inline validation message for the endpoint field (null when valid or still empty). */
  get endpointError(): string | null {
    if (!this.endpoint.trim()) return null;
    const v = validateEndpoint(this.endpoint);
    return 'error' in v ? v.error : null;
  }

  /** Name for a pasted key: the profile name, made unique among the saved keys. */
  get newKeyName(): string {
    const base = this.profileName.trim() || 'default';
    let name = base;
    for (let i = 2; this.keyNames.includes(name); i++) name = `${base}-${i}`;
    return name;
  }

  /** Keyless is the default only for local/LAN endpoints. */
  get keylessHint(): boolean {
    return this.authKind === 'none' && !isLocalEndpoint(this.endpoint);
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
      case 'key':
        if (this.authKind === 'existing')
          return this.existingKey ? null : 'Choose a saved key (or paste a new one).';
        if (this.authKind === 'new')
          return this.newKeySecret.trim()
            ? null
            : 'Paste your API key (or choose "No key" for a local server).';
        return null;
      case 'endpoint': {
        if (!this.endpoint.trim()) return 'The endpoint URL is required.';
        const v = validateEndpoint(this.endpoint);
        return 'error' in v ? v.error : null;
      }
      case 'check':
        if (this.checkPhase === 'checking') return 'Checking the endpoint...';
        return this.checkPhase === 'ok' ? null : 'The endpoint has not been verified yet.';
      case 'profile':
        if (!PROFILE_NAME_RE.test(this.profileName.trim()))
          return 'Profile name: letters, digits, ".", "-" or "_".';
        if (this.profileExists && !this.overwrite)
          return `Profile "${this.profileName.trim()}" exists: confirm that it should be overwritten.`;
        return null;
      case 'models':
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

  private resetChoices() {
    this.choices = {};
    for (const r of this.modeRows) this.choices[r.id] = r.initial;
  }

  setAuth(kind: AuthKind) {
    this.authKind = kind;
    if (kind === 'existing' && !this.existingKey) this.existingKey = this.keyNames[0] ?? '';
    if (kind !== 'new') this.wipeSecret();
    this.invalidateCheck();
  }

  /** The pasted key changed: re-check before moving on. */
  keyEdited() {
    this.invalidateCheck();
  }

  setEndpoint(url: string) {
    this.endpoint = url;
    this.endpointTouched = true;
    this.invalidateCheck();
  }

  private invalidateCheck() {
    this.generation++;
    this.checkPhase = 'idle';
    this.probe = null;
    this.checkError = null;
    this.forced = null;
    this.manual = false;
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
      case 'key':
        this.step = 'endpoint';
        break;
      case 'endpoint': {
        const v = validateEndpoint(this.endpoint);
        if ('url' in v) this.endpoint = v.url;
        this.step = 'check';
        if (this.checkPhase !== 'ok') void this.runCheck();
        break;
      }
      case 'check':
        this.enterModels();
        break;
      case 'models':
        this.step = 'profile';
        break;
      case 'profile':
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
    // Never leave a probe running behind the user's back.
    if (this.step === 'endpoint' && this.checkPhase === 'checking') {
      this.generation++;
      this.checkPhase = 'idle';
    }
  }

  goto(step: Step) {
    if (STEPS.indexOf(step) < this.stepIndex) {
      this.step = step;
      if (step === 'endpoint' && this.checkPhase === 'checking') {
        this.generation++;
        this.checkPhase = 'idle';
      }
    }
  }

  private resetChoicesIfNeeded() {
    for (const r of this.modeRows) if (!(r.id in this.choices)) this.choices[r.id] = r.initial;
  }

  // ---- check -----------------------------------------------------------------------------

  private authForProbe(): ModelsAuth | undefined {
    switch (this.authKind) {
      case 'existing':
        return { keyName: this.existingKey };
      case 'new':
        return { secret: this.newKeySecret.trim() };
      default:
        return undefined;
    }
  }

  /** Probe the endpoint with the key; `force` checks only that API flavor (manual override). */
  async runCheck(force: ApiKind | null = null) {
    const gen = ++this.generation;
    this.forced = force;
    this.checkPhase = 'checking';
    this.checkError = null;
    this.manual = false;
    try {
      const r = await this.api.probe(this.target, {
        baseUrl: this.endpoint.trim(),
        force: force ?? undefined,
        auth: this.authForProbe()
      });
      if (gen !== this.generation) return;
      this.probe = r;
      if (r.kind) {
        this.fetched = r.models;
        this.checkPhase = 'ok';
      } else {
        this.fetched = [];
        this.checkPhase = 'failed';
        this.checkError = r.message;
      }
    } catch (e) {
      if (gen !== this.generation) return;
      this.probe = null;
      this.fetched = [];
      this.checkPhase = 'failed';
      this.checkError = e instanceof Error ? e.message : String(e);
    }
  }

  /** Check only the other API flavor than the one currently assumed. */
  overrideKind() {
    void this.runCheck(this.kind === 'anthropic' ? 'openai' : 'anthropic');
  }

  /** After a failed check: continue and type the model ids by hand. */
  continueManually() {
    this.manual = true;
    this.enterModels();
  }

  private enterModels() {
    if (!this.profileTouched) {
      this.profileName = suggestedProfileName(this.suggestions, this.kind, this.endpoint);
    }
    this.initSelection();
    this.step = 'models';
  }

  // ---- models ----------------------------------------------------------------------------

  private initSelection() {
    const existing = this.state.profiles.find((p) => p.name === this.profileName.trim());
    const keep = this.selected.filter((m) => this.available.includes(m));
    if (keep.length) {
      this.selected = keep;
    } else if (existing) {
      this.selected = existing.models.filter((m) => this.available.includes(m));
    }
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
            name: this.newKeyName,
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
      provider: this.kind,
      baseUrl: this.endpoint.trim(),
      model: this.defaultModel,
      models: [...this.selected],
      apiKeyName: this.authKind === 'existing' ? this.existingKey : newKey ? newKey.name : null,
      newKey,
      keyScope: 'user',
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
