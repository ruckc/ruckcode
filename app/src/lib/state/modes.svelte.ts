import type { InitInput } from '../host/setup';
import type { SetupTarget } from '../host/setup';
import type {
  Catalog,
  Issue,
  ModeDto,
  ModesApi,
  ModesDoc,
  ModesView,
  TemplateInfo
} from '../host/modes';
import {
  clone,
  countBy,
  deleteMode,
  docsEqual,
  duplicateOf,
  emptyMode,
  mergeIssues,
  modeFromTemplate,
  modeIndex,
  moveItem,
  normalizeDoc,
  referencesTo,
  renameMode,
  uniqueId,
  validateDoc,
  ID_RE,
  type Reference
} from '../modes/doc';

export type Step = 'edit' | 'review' | 'saved';

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/**
 * State of the mode/hook editor: the draft document, live validation (instant client-side checks,
 * then the host's answer, debounced), the review (diff) step and saving. Nothing here touches the
 * DOM. The draft is edited in place by the form components (`bind:` on `doc`), so everything that
 * depends on it is derived from `fingerprint`.
 */
export class ModesEditor {
  phase = $state<'loading' | 'ready' | 'error'>('loading');
  loadError = $state<string | null>(null);
  view = $state<ModesView | null>(null);
  /** The document as stored. */
  original = $state<ModesDoc>({ defaultMode: '', modes: [] });
  /** The draft being edited. */
  doc = $state<ModesDoc>({ defaultMode: '', modes: [] });
  /** Editing a file that does not exist yet (started from scratch). */
  creating = $state(false);
  /** The selected mode's id; null shows the overview (default mode and flow). */
  selected = $state<string | null>(null);
  serverIssues = $state<Issue[] | null>(null);
  serverNote = $state<string | null>(null);

  step = $state<Step>('edit');
  diff = $state('');
  rewritten = $state<string[]>([]);
  busy = $state(false);
  problem = $state<string | null>(null);
  savedPath = $state<string | null>(null);
  savedChanged = $state(false);
  /** Set by `initProject`. */
  initialized = $state<string | null>(null);

  private timer: ReturnType<typeof setTimeout> | undefined;
  private seq = 0;
  private validatedFor = '';

  constructor(
    readonly api: ModesApi,
    readonly target: SetupTarget,
    private readonly debounceMs = 250
  ) {}

  // ----- derived -----

  readonly catalog = $derived<Catalog | null>(this.view?.catalog ?? null);
  readonly status = $derived(this.view?.status ?? 'missing');
  /** True when the draft can be edited (a file, or a new one started from scratch). */
  readonly editable = $derived(this.status === 'ready' || this.creating);
  readonly fingerprint = $derived(JSON.stringify(normalizeDoc(this.doc)));
  readonly dirty = $derived(this.editable && !docsEqual(this.doc, this.original));
  readonly clientIssues = $derived<Issue[]>(
    this.editable ? validateDoc(this.doc, this.catalog) : []
  );
  readonly issues = $derived<Issue[]>(
    this.serverIssues && this.validatedFor === this.fingerprint
      ? this.serverIssues
      : this.clientIssues
  );
  readonly errors = $derived(countBy(this.issues, 'error'));
  readonly warnings = $derived(countBy(this.issues, 'warning'));
  readonly canReview = $derived(this.dirty && this.errors === 0 && !this.busy);
  readonly selectedMode = $derived<ModeDto | null>(
    this.doc.modes.find((m) => m.id === this.selected) ?? null
  );
  readonly selectedIndex = $derived(this.doc.modes.findIndex((m) => m.id === this.selected));

  // ----- loading -----

  async load(keepSelection = false) {
    if (!keepSelection) this.phase = 'loading';
    this.loadError = null;
    try {
      const v = await this.api.get(this.target);
      this.view = v;
      this.creating = false;
      const doc: ModesDoc =
        v.status === 'ready'
          ? { defaultMode: v.defaultMode ?? '', modes: v.modes }
          : { defaultMode: '', modes: [] };
      this.original = clone(doc);
      this.doc = clone(doc);
      if (!keepSelection || !this.doc.modes.some((m) => m.id === this.selected))
        this.selected = null;
      this.serverIssues = null;
      this.step = 'edit';
      this.phase = 'ready';
    } catch (e) {
      this.loadError = message(e);
      this.phase = 'error';
    }
  }

  /** `rc init`: create modes.toml from the templates and the project's mise tasks. */
  async initProject(opts: InitInput) {
    this.busy = true;
    this.problem = null;
    try {
      const r = await this.api.initModes(this.target, opts);
      await this.load();
      this.initialized = `Wrote ${r.path} (${r.tasksDiscovered} mise task${r.tasksDiscovered === 1 ? '' : 's'} discovered).`;
    } catch (e) {
      this.problem = message(e);
    } finally {
      this.busy = false;
    }
  }

  /** Edit a new file from scratch instead of `rc init`. */
  startBlank() {
    const m = emptyMode('default');
    this.creating = true;
    this.original = { defaultMode: '', modes: [] };
    this.doc = { defaultMode: 'default', modes: [m] };
    this.selected = 'default';
    this.step = 'edit';
  }

  discard() {
    this.doc = clone(this.original);
    if (!this.doc.modes.some((m) => m.id === this.selected)) this.selected = null;
    if (this.creating && !this.original.modes.length) {
      this.creating = false;
      this.selected = null;
    }
    this.problem = null;
    this.step = 'edit';
  }

  // ----- validation -----

  /** Ask the host to validate the current draft (after `debounceMs` of quiet). */
  scheduleValidate() {
    clearTimeout(this.timer);
    if (!this.editable) return;
    this.timer = setTimeout(() => void this.validateNow(), this.debounceMs);
  }

  async validateNow() {
    clearTimeout(this.timer);
    if (!this.editable) return;
    const fp = this.fingerprint;
    const n = ++this.seq;
    try {
      const issues = await this.api.validate(this.target, normalizeDoc(this.doc));
      if (n !== this.seq || fp !== this.fingerprint) return; // a newer edit superseded this answer
      this.serverIssues = issues;
      this.validatedFor = fp;
      this.serverNote = null;
    } catch (e) {
      if (n !== this.seq) return;
      this.serverIssues = null;
      this.serverNote = `Could not reach the host to validate: ${message(e)}`;
    }
  }

  destroy() {
    clearTimeout(this.timer);
    this.seq++;
  }

  // ----- document edits -----

  select(id: string | null) {
    this.selected = id;
  }

  private add(m: ModeDto) {
    this.doc.modes.push(m);
    if (!this.doc.defaultMode) this.doc.defaultMode = m.id;
    this.selected = m.id;
    return m.id;
  }

  addBlank(): string {
    return this.add(
      emptyMode(
        uniqueId(
          'mode',
          this.doc.modes.map((m) => m.id)
        )
      )
    );
  }

  addTemplate(t: TemplateInfo): string {
    return this.add(modeFromTemplate(t, this.doc));
  }

  duplicate(id: string): string | null {
    const m = this.doc.modes.find((x) => x.id === id);
    return m ? this.add(duplicateOf(m, this.doc)) : null;
  }

  /** Why `id` cannot be used for `current` (renaming), or null. */
  idProblem(current: string, id: string): string | null {
    if (!id) return 'The id is required.';
    if (!ID_RE.test(id)) return 'Use letters, digits, `-` and `_` (max 64).';
    if (id !== current && this.doc.modes.some((m) => m.id === id))
      return `A mode with id \`${id}\` exists.`;
    return null;
  }

  /** Rename a mode and update every reference to it. */
  rename(oldId: string, newId: string): boolean {
    if (oldId === newId || this.idProblem(oldId, newId)) return false;
    this.doc = renameMode(this.doc, oldId, newId);
    if (this.selected === oldId) this.selected = newId;
    return true;
  }

  references(id: string): Reference[] {
    return referencesTo(this.doc, id);
  }

  remove(id: string) {
    const i = modeIndex(this.doc, id);
    if (i < 0) return;
    this.doc = deleteMode(this.doc, id);
    if (this.selected === id)
      this.selected = this.doc.modes[Math.min(i, this.doc.modes.length - 1)]?.id ?? null;
  }

  /** Move a mode to `index` (the order is the file order, and the default is the first). */
  move(id: string, index: number) {
    const from = modeIndex(this.doc, id);
    if (from < 0) return;
    this.doc.modes = moveItem(this.doc.modes, from, index);
  }

  setDefault(id: string) {
    this.doc.defaultMode = id;
  }

  // ----- review and save -----

  async review() {
    if (!this.canReview) return;
    this.busy = true;
    this.problem = null;
    try {
      const p = await this.api.preview(this.target, normalizeDoc(this.doc));
      this.diff = p.diff;
      this.rewritten = p.rewritten;
      this.step = 'review';
    } catch (e) {
      this.problem = message(e);
    } finally {
      this.busy = false;
    }
  }

  backToEdit() {
    this.step = 'edit';
    this.problem = null;
  }

  async save() {
    this.busy = true;
    this.problem = null;
    try {
      const r = await this.api.apply(this.target, normalizeDoc(this.doc));
      const keep = this.selected;
      await this.load(true);
      this.selected = this.doc.modes.some((m) => m.id === keep) ? keep : null;
      this.savedPath = r.path;
      this.savedChanged = r.changed;
      this.step = 'saved';
    } catch (e) {
      this.problem = message(e);
      this.step = 'review';
    } finally {
      this.busy = false;
    }
  }

  /** Issues for the whole document, with `mergeIssues` available to callers for custom views. */
  static merge = mergeIssues;
}
