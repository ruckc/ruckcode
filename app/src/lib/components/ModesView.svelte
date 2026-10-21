<script lang="ts">
  import { untrack } from 'svelte';
  import type { ModesApi, TemplateInfo } from '../host/modes';
  import type { SetupTarget } from '../host/setup';
  import { displayName, issuesAt } from '../modes/doc';
  import { ModesEditor } from '../state/modes.svelte';
  import DiffText from './DiffText.svelte';
  import FlowDiagram from './FlowDiagram.svelte';
  import HooksEditor from './HooksEditor.svelte';
  import ModeForm from './ModeForm.svelte';

  let {
    api,
    target,
    ondirty,
    canReconnect = false,
    onreconnect
  }: {
    api: ModesApi;
    target: SetupTarget;
    /** Called when the draft starts or stops differing from the file (the host app guards navigation with it). */
    ondirty?: (dirty: boolean) => void;
    /** The agent is connected, so it can be reconnected to pick up the new modes. */
    canReconnect?: boolean;
    onreconnect?: () => void;
  } = $props();

  const editor = untrack(() => new ModesEditor(api, target));
  void editor.load();

  $effect(() => {
    ondirty?.(editor.dirty);
  });
  // Validate on the host a moment after each edit.
  $effect(() => {
    void editor.fingerprint;
    editor.scheduleValidate();
  });
  $effect(() => () => editor.destroy());

  const cat = $derived(editor.catalog);
  const fullTemplates = $derived(cat?.templates.filter((t) => t.set === 'full') ?? []);
  const minimalTemplates = $derived(cat?.templates.filter((t) => t.set === 'minimal') ?? []);

  let addChoice = $state('blank');
  function addMode() {
    if (addChoice === 'blank') editor.addBlank();
    else if (addChoice === 'duplicate') {
      if (editor.selected) editor.duplicate(editor.selected);
    } else {
      const [set, id] = addChoice.split(':');
      const t = cat?.templates.find((x: TemplateInfo) => x.set === set && x.mode.id === id);
      if (t) editor.addTemplate(t);
    }
  }

  // ----- reordering by drag and drop -----
  let dragFrom = $state<number | null>(null);
  let dragOver = $state<number | null>(null);
  function drop(to: number) {
    if (dragFrom !== null) {
      const id = editor.doc.modes[dragFrom]?.id;
      if (id) editor.move(id, to);
    }
    dragFrom = dragOver = null;
  }

  const modeHasError = (i: number) =>
    issuesAt(editor.issues, `modes[${i}]`).some((x) => x.severity === 'error');

  /** Index of the mode an issue path points at, or null. */
  function issueMode(path: string): string | null {
    const m = /^modes\[(\d+)\]/.exec(path);
    return m ? (editor.doc.modes[Number(m[1])]?.id ?? null) : null;
  }

  // ----- creating modes.toml -----
  let initKind = $state<'full' | 'minimal' | 'subset'>('full');
  let subset = $state<string[]>([]);
  let confirmOverwrite = $state(false);
  function create(force: boolean) {
    const minimal = initKind === 'minimal';
    const modes = initKind === 'subset' ? subset : [];
    void editor.initProject({ minimal, modes, force });
  }
  $effect(() => {
    if (!subset.length && fullTemplates.length) subset = fullTemplates.map((t) => t.mode.id);
  });
</script>

<section class="modes" aria-label="Mode editor">
  {#if editor.phase === 'loading'}
    <p class="muted pad" role="status">Loading modes...</p>
  {:else if editor.phase === 'error'}
    <div class="pad">
      <p class="errtext" role="alert">{editor.loadError}</p>
      <button type="button" onclick={() => editor.load()}>Retry</button>
    </div>
  {:else if !editor.editable}
    <div class="pad setup">
      {#if editor.status === 'broken'}
        <h2>modes.toml cannot be read</h2>
        <p class="errtext" role="alert">{editor.view?.error}</p>
        <p class="muted">
          Fix <code>{editor.view?.path}</code> by hand, then reload, or recreate it from a template (this
          overwrites the file).
        </p>
      {:else}
        <h2>No modes.toml</h2>
        <p class="muted">
          <code>.ruckcode/modes.toml</code> defines the agent's modes: tools, permissions, hooks and
          how modes lead into each other. Create it from the built-in templates and this project's
          mise tasks (like <code>rc init</code>), or start from a blank file.
        </p>
      {/if}
      {#if editor.problem}<p class="errtext" role="alert">{editor.problem}</p>{/if}
      {#if editor.initialized}<p class="ok" role="status">{editor.initialized}</p>{/if}
      <fieldset>
        <legend>Template</legend>
        <label class="check"
          ><input type="radio" name="init-kind" value="full" bind:group={initKind} /> Full lifecycle
          ({fullTemplates.map((t) => t.mode.id).join(' → ')})</label
        >
        <label class="check"
          ><input type="radio" name="init-kind" value="minimal" bind:group={initKind} /> Minimal ({minimalTemplates
            .map((t) => t.mode.id)
            .join(' → ')})</label
        >
        <label class="check"
          ><input type="radio" name="init-kind" value="subset" bind:group={initKind} /> Choose which modes</label
        >
        {#if initKind === 'subset'}
          <ul class="plain subset" aria-label="Modes to create">
            {#each fullTemplates as t (t.mode.id)}
              <li>
                <label class="check"
                  ><input type="checkbox" value={t.mode.id} bind:group={subset} />
                  <code>{t.mode.id}</code>
                  <span class="muted">{t.mode.description}</span></label
                >
              </li>
            {/each}
          </ul>
        {/if}
      </fieldset>
      <div class="actions">
        {#if editor.status === 'broken'}
          {#if confirmOverwrite}
            <button
              type="button"
              class="danger"
              disabled={editor.busy || (initKind === 'subset' && !subset.length)}
              onclick={() => create(true)}>Overwrite modes.toml</button
            >
            <button type="button" onclick={() => (confirmOverwrite = false)}>Cancel</button>
          {:else}
            <button type="button" class="danger" onclick={() => (confirmOverwrite = true)}
              >Recreate from template...</button
            >
          {/if}
          <button type="button" onclick={() => editor.load()}>Reload</button>
        {:else}
          <button
            type="button"
            class="primary"
            disabled={editor.busy || (initKind === 'subset' && !subset.length)}
            onclick={() => create(false)}
            >{editor.busy ? 'Creating...' : 'Create modes.toml'}</button
          >
          <button type="button" onclick={() => editor.startBlank()}>Start from a blank file</button>
        {/if}
      </div>
    </div>
  {:else}
    <header class="bar">
      <div class="title">
        <strong>Modes</strong>
        <span class="muted path" title={editor.view?.path}>{editor.view?.path}</span>
      </div>
      <div class="status" aria-live="polite">
        {#if editor.dirty}<span class="chip warn">Unsaved changes</span>{/if}
        {#if editor.errors}<span class="chip err" data-testid="error-count"
            >{editor.errors} error{editor.errors === 1 ? '' : 's'}</span
          >{/if}
        {#if editor.warnings}<span class="chip warn" data-testid="warning-count"
            >{editor.warnings} warning{editor.warnings === 1 ? '' : 's'}</span
          >{/if}
        {#if !editor.errors && !editor.warnings && !editor.dirty}<span class="chip ok"
            >No problems</span
          >{/if}
      </div>
      <div class="actions">
        <button
          type="button"
          disabled={!editor.dirty || editor.busy}
          onclick={() => editor.discard()}>Discard changes</button
        >
        <button
          type="button"
          class="primary"
          disabled={!editor.canReview}
          title={editor.errors ? 'Fix the errors first' : ''}
          onclick={() => editor.review()}>Review &amp; save</button
        >
      </div>
    </header>

    {#if editor.serverNote}<p class="note" role="status">{editor.serverNote}</p>{/if}
    {#if editor.step === 'saved' && !editor.dirty}
      <div class="saved" role="status">
        <span>
          {editor.savedChanged ? 'Saved' : 'Nothing changed in'} <code>{editor.savedPath}</code>.
          Mode changes apply to new sessions; running sessions keep the modes they started with.
        </span>
        {#if canReconnect}
          <button type="button" onclick={() => onreconnect?.()}>Reconnect the agent now</button>
        {/if}
      </div>
    {/if}

    {#if editor.step === 'review'}
      <div class="review pad">
        <h2>Review changes</h2>
        {#if editor.problem}<p class="errtext" role="alert">{editor.problem}</p>{/if}
        {#if editor.diff}
          <p class="muted">
            Only the lines below change; comments and formatting elsewhere in <code>modes.toml</code
            > stay as they are.
          </p>
          <DiffText text={editor.diff} label="Changes to modes.toml" maxHeight="62vh" />
        {:else}
          <p class="muted">No changes to write.</p>
        {/if}
        {#if editor.rewritten.length}
          <p class="note" role="status">
            Written out fresh (their layout cannot be edited in place, so comments in them are
            lost):
            {editor.rewritten.join(', ')}.
          </p>
        {/if}
        <div class="actions">
          <button type="button" disabled={editor.busy} onclick={() => editor.backToEdit()}
            >Back to editing</button
          >
          <button type="button" class="primary" disabled={editor.busy} onclick={() => editor.save()}
            >{editor.busy ? 'Saving...' : 'Save modes.toml'}</button
          >
        </div>
      </div>
    {:else}
      <div class="cols">
        <nav class="list" aria-label="Modes">
          <button
            type="button"
            class="overview"
            aria-current={editor.selected === null ? 'true' : undefined}
            onclick={() => editor.select(null)}>Overview &amp; flow</button
          >
          <ul aria-label="Modes list">
            {#each editor.doc.modes as m, i (m.originalId ?? `new-${i}`)}
              <li
                class:sel={m.id === editor.selected}
                class:over={dragOver === i && dragFrom !== i}
                draggable="true"
                ondragstart={() => (dragFrom = i)}
                ondragover={(e) => {
                  e.preventDefault();
                  dragOver = i;
                }}
                ondrop={(e) => {
                  e.preventDefault();
                  drop(i);
                }}
                ondragend={() => (dragFrom = dragOver = null)}
              >
                <button
                  type="button"
                  class="pick"
                  aria-current={m.id === editor.selected ? 'true' : undefined}
                  onclick={() => editor.select(m.id)}
                >
                  <span class="nm">{displayName(m)}</span>
                  <code>{m.id}</code>
                  {#if m.id === editor.doc.defaultMode}<span class="tag">default</span>{/if}
                  {#if m.originalId === undefined}<span class="tag new">new</span>{/if}
                  {#if modeHasError(i)}<span class="dot" title="Has errors" aria-label="has errors"
                    ></span>{/if}
                </button>
                <span class="mv">
                  <button
                    type="button"
                    class="small"
                    disabled={i === 0}
                    aria-label={`Move ${m.id} up`}
                    onclick={() => editor.move(m.id, i - 1)}>&uarr;</button
                  >
                  <button
                    type="button"
                    class="small"
                    disabled={i === editor.doc.modes.length - 1}
                    aria-label={`Move ${m.id} down`}
                    onclick={() => editor.move(m.id, i + 1)}>&darr;</button
                  >
                </span>
              </li>
            {/each}
          </ul>
          <div class="add">
            <select aria-label="Add mode from template" bind:value={addChoice}>
              <option value="blank">Blank mode</option>
              <option value="duplicate" disabled={!editor.selectedMode}>Duplicate selected</option>
              <optgroup label="Lifecycle templates">
                {#each fullTemplates as t (t.mode.id)}
                  <option value={`full:${t.mode.id}`}>{displayName(t.mode)}</option>
                {/each}
              </optgroup>
              <optgroup label="Minimal templates">
                {#each minimalTemplates as t (t.mode.id)}
                  <option value={`minimal:${t.mode.id}`}>{displayName(t.mode)}</option>
                {/each}
              </optgroup>
            </select>
            <button type="button" onclick={addMode}>Add mode</button>
          </div>
        </nav>

        <div class="detail">
          {#if editor.selectedMode}
            <div class="mhead">
              <h2>{displayName(editor.selectedMode)} <code>{editor.selectedMode.id}</code></h2>
              <button
                type="button"
                class="small"
                onclick={() => editor.selected && editor.duplicate(editor.selected)}
                >Duplicate</button
              >
              <button
                type="button"
                class="small"
                disabled={editor.selectedMode.id === editor.doc.defaultMode}
                onclick={() => editor.selected && editor.setDefault(editor.selected)}
                >Make default</button
              >
            </div>
            <ModeForm {editor} />
          {:else}
            <h2>Overview</h2>
            <label class="def"
              >Default mode
              <select
                bind:value={editor.doc.defaultMode}
                aria-label="Default mode"
                aria-invalid={editor.issues.some((i) => i.path === 'defaultMode')}
              >
                {#if !editor.doc.modes.some((m) => m.id === editor.doc.defaultMode)}
                  <option value={editor.doc.defaultMode}
                    >{editor.doc.defaultMode || '(none)'}</option
                  >
                {/if}
                {#each editor.doc.modes as m (m.id)}<option value={m.id}
                    >{displayName(m)} ({m.id})</option
                  >{/each}
              </select>
            </label>
            {#each issuesAt(editor.issues, 'defaultMode') as i (i.message)}
              <p class="errtext small">{i.message}</p>
            {/each}
            <h3>Flow</h3>
            <p class="muted small">
              Arrows show where <code>SwitchMode</code> can go. Thick arrows auto-advance without asking
              you. Click a mode to edit it.
            </p>
            <FlowDiagram
              modes={editor.doc.modes}
              defaultMode={editor.doc.defaultMode}
              selected={editor.selected}
              onselect={(id) => editor.select(id)}
            />
            {#if editor.issues.length}
              <h3>Problems</h3>
              <ul class="problems" aria-label="Problems">
                {#each editor.issues as i, n (n)}
                  {@const id = issueMode(i.path)}
                  <li class={i.severity}>
                    <span class="sev">{i.severity}</span>
                    {#if id}<button type="button" class="link" onclick={() => editor.select(id)}
                        >{id}</button
                      >{/if}
                    {i.message}
                  </li>
                {/each}
              </ul>
            {/if}
            {#if editor.view?.globalHooks}
              <h3>Project hooks</h3>
              <p class="muted small">
                From <code>config.toml</code>; they run in every mode. Edit them in the file.
              </p>
              <HooksEditor
                hooks={editor.view.globalHooks}
                catalog={cat}
                issues={[]}
                base="globalHooks"
                readonly
              />
            {/if}
          {/if}
        </div>
      </div>
    {/if}
  {/if}
</section>

<style>
  .modes {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .pad {
    padding: 14px 18px;
    overflow: auto;
  }
  .setup {
    max-width: 640px;
    display: grid;
    gap: 8px;
    align-content: start;
  }
  .setup h2,
  .review h2 {
    margin: 0;
    font-size: 16px;
  }
  .bar {
    display: flex;
    gap: 12px;
    align-items: center;
    flex-wrap: wrap;
    padding: 8px 14px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    display: flex;
    gap: 10px;
    align-items: baseline;
    min-width: 0;
    flex: 1;
  }
  .path {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .status {
    display: flex;
    gap: 6px;
  }
  .actions {
    margin: 0;
  }
  .chip {
    font-size: 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0 8px;
  }
  .chip.err {
    color: var(--err);
    border-color: var(--err);
  }
  .chip.warn {
    color: var(--warn);
    border-color: var(--warn);
  }
  .chip.ok {
    color: var(--ok);
    border-color: var(--ok);
  }
  .note {
    margin: 0;
    padding: 6px 14px;
    color: var(--warn);
    font-size: 13px;
  }
  .saved {
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
    margin: 0;
    padding: 6px 14px;
    background: var(--code-bg);
    color: var(--ok);
    font-size: 13px;
  }
  .cols {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 270px minmax(0, 1fr);
  }
  .list {
    border-right: 1px solid var(--border);
    overflow-y: auto;
    padding: 8px;
    display: grid;
    gap: 8px;
    align-content: start;
  }
  .list ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 3px;
  }
  .list li {
    display: flex;
    align-items: center;
    gap: 2px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
  }
  .list li.sel {
    border-color: var(--accent);
  }
  .list li.over {
    border-top: 3px solid var(--accent);
  }
  .pick {
    flex: 1;
    min-width: 0;
    text-align: left;
    border: none;
    background: none;
    display: flex;
    flex-wrap: wrap;
    gap: 2px 6px;
    align-items: baseline;
    cursor: grab;
  }
  .nm {
    font-weight: 600;
  }
  .overview {
    text-align: left;
  }
  .overview[aria-current='true'] {
    border-color: var(--accent);
  }
  .mv {
    display: flex;
    padding-right: 2px;
  }
  .small {
    font-size: 12px;
    padding: 0 6px;
  }
  .tag {
    font-size: 11px;
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0 6px;
    color: var(--muted);
  }
  .tag.new {
    color: var(--ok);
    border-color: var(--ok);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--err);
    align-self: center;
  }
  .add {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .add select {
    flex: 1;
    min-width: 0;
  }
  .detail {
    overflow-y: auto;
    padding: 12px 18px 40px;
    min-width: 0;
  }
  .detail h2 {
    margin: 0 0 6px;
    font-size: 16px;
  }
  .detail h3 {
    margin: 14px 0 4px;
    font-size: 14px;
  }
  .mhead {
    display: flex;
    gap: 8px;
    align-items: baseline;
    flex-wrap: wrap;
    margin-bottom: 8px;
  }
  .mhead h2 {
    margin: 0;
    flex: 1;
  }
  .def {
    display: grid;
    gap: 2px;
    max-width: 320px;
    font-size: 12px;
    color: var(--muted);
  }
  .def select {
    color: var(--text);
    font-size: 14px;
  }
  .problems {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 13px;
  }
  .problems li.error .sev {
    color: var(--err);
  }
  .problems li.warning .sev {
    color: var(--warn);
  }
  .sev {
    font-size: 11px;
    text-transform: uppercase;
    margin-right: 4px;
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent-link);
    text-decoration: underline;
  }
  .errtext {
    color: var(--err);
    margin: 0;
  }
  .small.errtext {
    font-size: 12px;
  }
  .ok {
    color: var(--ok);
    margin: 0;
  }
  fieldset {
    border: 1px solid var(--border);
    border-radius: 6px;
    margin: 0;
    display: grid;
    gap: 4px;
  }
  label.check {
    display: flex;
    gap: 6px;
    align-items: baseline;
  }
  .plain {
    list-style: none;
    margin: 0;
    padding: 0 0 0 20px;
  }
  .review {
    display: grid;
    gap: 8px;
    align-content: start;
  }
  @media (max-width: 760px) {
    .cols {
      grid-template-columns: minmax(0, 1fr);
    }
    .list {
      border-right: none;
      border-bottom: 1px solid var(--border);
      max-height: 40vh;
    }
  }
</style>
