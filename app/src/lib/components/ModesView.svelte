<script lang="ts">
  import {
    Badge,
    Button,
    Callout,
    CheckboxGroup,
    RadioGroup,
    ReorderList,
    Select,
    Tag
  } from '@ruckc/truss-ui';
  import { untrack } from 'svelte';
  import type { ModesApi, TemplateInfo } from '../host/modes';
  import type { SetupTarget } from '../host/setup';
  import { displayName, issuesAt } from '../modes/doc';
  import { ModesEditor } from '../state/modes.svelte';
  import { PatchText } from '@ruckc/truss-code/lite';
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

  const addOptions = $derived([
    { value: 'blank', label: 'Blank mode' },
    { value: 'duplicate', label: 'Duplicate selected', disabled: !editor.selectedMode },
    {
      group: 'Lifecycle templates',
      options: fullTemplates.map((t) => ({
        value: `full:${t.mode.id}`,
        label: displayName(t.mode)
      }))
    },
    {
      group: 'Minimal templates',
      options: minimalTemplates.map((t) => ({
        value: `minimal:${t.mode.id}`,
        label: displayName(t.mode)
      }))
    }
  ]);
  const defaultOptions = $derived([
    ...(editor.doc.modes.some((m) => m.id === editor.doc.defaultMode)
      ? []
      : [{ value: editor.doc.defaultMode, label: editor.doc.defaultMode || '(none)' }]),
    ...editor.doc.modes.map((m) => ({ value: m.id, label: `${displayName(m)} (${m.id})` }))
  ]);
  const defaultError = $derived(
    issuesAt(editor.issues, 'defaultMode')
      .map((i) => i.message)
      .join(' ') || undefined
  );
  const initOptions = $derived([
    {
      value: 'full',
      label: `Full lifecycle (${fullTemplates.map((t) => t.mode.id).join(' → ')})`
    },
    {
      value: 'minimal',
      label: `Minimal (${minimalTemplates.map((t) => t.mode.id).join(' → ')})`
    },
    { value: 'subset', label: 'Choose which modes' }
  ]);
  const sevIntent = (s: string) =>
    s === 'error' ? 'danger' : s === 'warning' ? 'warning' : 'neutral';

  const modeHasError = (i: number) =>
    issuesAt(editor.issues, `modes[${i}]`).some((x) => x.severity === 'error');

  /** Index of the mode an issue path points at, or null. */
  function issueMode(path: string): string | null {
    const m = /^modes\[(\d+)\]/.exec(path);
    return m ? (editor.doc.modes[Number(m[1])]?.id ?? null) : null;
  }

  // ----- creating modes.toml -----
  let initKind = $state('full');
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
    <div class="pad stack">
      <Callout intent="danger">{editor.loadError}</Callout>
      <div class="actions">
        <Button variant="outlined" onclick={() => editor.load()}>Retry</Button>
      </div>
    </div>
  {:else if !editor.editable}
    <div class="pad setup">
      {#if editor.status === 'broken'}
        <h2>modes.toml cannot be read</h2>
        <Callout intent="danger">{editor.view?.error}</Callout>
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
      {#if editor.problem}<Callout intent="danger">{editor.problem}</Callout>{/if}
      {#if editor.initialized}<Callout intent="success" role="status">{editor.initialized}</Callout
        >{/if}
      <RadioGroup label="Template" options={initOptions} bind:value={initKind} />
      {#if initKind === 'subset'}
        <CheckboxGroup
          label="Modes to create"
          options={fullTemplates.map((t) => ({
            value: t.mode.id,
            label: t.mode.id,
            description: t.mode.description
          }))}
          bind:value={subset}
        />
      {/if}
      <div class="actions">
        {#if editor.status === 'broken'}
          {#if confirmOverwrite}
            <Button
              intent="danger"
              disabled={editor.busy || (initKind === 'subset' && !subset.length)}
              onclick={() => create(true)}>Overwrite modes.toml</Button
            >
            <Button variant="outlined" intent="neutral" onclick={() => (confirmOverwrite = false)}
              >Cancel</Button
            >
          {:else}
            <Button variant="outlined" intent="danger" onclick={() => (confirmOverwrite = true)}
              >Recreate from template...</Button
            >
          {/if}
          <Button variant="outlined" intent="neutral" onclick={() => editor.load()}>Reload</Button>
        {:else}
          <Button
            disabled={editor.busy || (initKind === 'subset' && !subset.length)}
            onclick={() => create(false)}
            >{editor.busy ? 'Creating...' : 'Create modes.toml'}</Button
          >
          <Button variant="outlined" intent="neutral" onclick={() => editor.startBlank()}
            >Start from a blank file</Button
          >
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
        {#if editor.dirty}<Badge intent="warning">Unsaved changes</Badge>{/if}
        {#if editor.errors}<Badge intent="danger" data-testid="error-count"
            >{editor.errors} error{editor.errors === 1 ? '' : 's'}</Badge
          >{/if}
        {#if editor.warnings}<Badge intent="warning" data-testid="warning-count"
            >{editor.warnings} warning{editor.warnings === 1 ? '' : 's'}</Badge
          >{/if}
        {#if !editor.errors && !editor.warnings && !editor.dirty}<Badge intent="success"
            >No problems</Badge
          >{/if}
      </div>
      <div class="actions">
        <Button
          variant="outlined"
          intent="neutral"
          disabled={!editor.dirty || editor.busy}
          onclick={() => editor.discard()}>Discard changes</Button
        >
        <Button
          disabled={!editor.canReview}
          title={editor.errors ? 'Fix the errors first' : ''}
          onclick={() => editor.review()}>Review &amp; save</Button
        >
      </div>
    </header>

    {#if editor.serverNote}<div class="banner" role="status">
        <Callout intent="warning">{editor.serverNote}</Callout>
      </div>{/if}
    {#if editor.step === 'saved' && !editor.dirty}
      <div class="banner" role="status">
        <Callout intent="success">
          {editor.savedChanged ? 'Saved' : 'Nothing changed in'} <code>{editor.savedPath}</code>.
          Mode changes apply to new sessions; running sessions keep the modes they started with.
          {#snippet actions()}
            {#if canReconnect}
              <Button size="sm" variant="outlined" onclick={() => onreconnect?.()}
                >Reconnect the agent now</Button
              >
            {/if}
          {/snippet}
        </Callout>
      </div>
    {/if}

    {#if editor.step === 'review'}
      <div class="review pad">
        <h2>Review changes</h2>
        {#if editor.problem}<Callout intent="danger">{editor.problem}</Callout>{/if}
        {#if editor.diff}
          <p class="muted">
            Only the lines below change; comments and formatting elsewhere in <code>modes.toml</code
            > stay as they are.
          </p>
          <PatchText text={editor.diff} label="Changes to modes.toml" maxHeight="62vh" />
        {:else}
          <p class="muted">No changes to write.</p>
        {/if}
        {#if editor.rewritten.length}
          <Callout intent="warning" role="status">
            Written out fresh (their layout cannot be edited in place, so comments in them are
            lost):
            {editor.rewritten.join(', ')}.
          </Callout>
        {/if}
        <div class="actions">
          <Button
            variant="outlined"
            intent="neutral"
            disabled={editor.busy}
            onclick={() => editor.backToEdit()}>Back to editing</Button
          >
          <Button disabled={editor.busy} onclick={() => editor.save()}
            >{editor.busy ? 'Saving...' : 'Save modes.toml'}</Button
          >
        </div>
      </div>
    {:else}
      <div class="cols">
        <nav class="list" aria-label="Modes">
          <Button
            variant={editor.selected === null ? 'tonal' : 'outlined'}
            intent="neutral"
            aria-current={editor.selected === null ? 'true' : undefined}
            onclick={() => editor.select(null)}>Overview &amp; flow</Button
          >
          <ReorderList
            label="Modes list"
            items={editor.doc.modes}
            getKey={(m) => m.id}
            getLabel={(m) => m.id}
            selected={editor.selected ?? undefined}
            buttons
            handles
            onselect={(id) => editor.select(id)}
            onreorder={(from, to) => editor.move(editor.doc.modes[from].id, to)}
          >
            {#snippet item(m, i)}
              <span class="nm">{displayName(m)}</span>
              <code>{m.id}</code>
              {#if m.id === editor.doc.defaultMode}<Tag size="sm">default</Tag>{/if}
              {#if m.originalId === undefined}<Tag size="sm" intent="success">new</Tag>{/if}
              {#if modeHasError(i)}<span class="dot" title="Has errors" aria-label="has errors"
                ></span>{/if}
            {/snippet}
          </ReorderList>
          <div class="add">
            <div class="grow">
              <Select
                size="sm"
                searchable={false}
                ariaLabel="Add mode from template"
                options={addOptions}
                bind:value={addChoice}
              />
            </div>
            <Button size="sm" variant="outlined" icon="plus" onclick={addMode}>Add mode</Button>
          </div>
        </nav>

        <div class="detail">
          {#if editor.selectedMode}
            <div class="mhead">
              <h2>{displayName(editor.selectedMode)} <code>{editor.selectedMode.id}</code></h2>
              <Button
                size="sm"
                variant="outlined"
                intent="neutral"
                onclick={() => editor.selected && editor.duplicate(editor.selected)}
                >Duplicate</Button
              >
              <Button
                size="sm"
                variant="outlined"
                intent="neutral"
                disabled={editor.selectedMode.id === editor.doc.defaultMode}
                onclick={() => editor.selected && editor.setDefault(editor.selected)}
                >Make default</Button
              >
            </div>
            <ModeForm {editor} />
          {:else}
            <h2>Overview</h2>
            <div class="def">
              <Select
                label="Default mode"
                searchable={false}
                options={defaultOptions}
                value={editor.doc.defaultMode}
                error={defaultError}
                onchange={(v) => (editor.doc.defaultMode = v)}
              />
            </div>
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
                  <li>
                    <Badge intent={sevIntent(i.severity)} dot={false}>{i.severity}</Badge>
                    {#if id}<Button size="sm" variant="minimal" onclick={() => editor.select(id)}
                        >{id}</Button
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
    padding: var(--tr-space-4) var(--tr-space-5);
    overflow: auto;
  }
  .stack {
    display: grid;
    gap: var(--tr-space-2);
    align-content: start;
  }
  .setup {
    max-width: 640px;
    display: grid;
    gap: var(--tr-space-2);
    align-content: start;
  }
  .setup h2,
  .review h2 {
    margin: 0;
    font-size: var(--tr-text-lg);
  }
  .bar {
    display: flex;
    gap: var(--tr-space-3);
    align-items: center;
    flex-wrap: wrap;
    padding: var(--tr-space-2) var(--tr-space-4);
    border-bottom: 1px solid var(--tr-outline-variant);
  }
  .title {
    display: flex;
    gap: var(--tr-space-3);
    align-items: baseline;
    min-width: 0;
    flex: 1;
  }
  .path {
    font-size: var(--tr-text-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .status {
    display: flex;
    gap: var(--tr-space-2);
  }
  .actions {
    display: flex;
    gap: var(--tr-space-2);
    flex-wrap: wrap;
    margin: 0;
  }
  .banner {
    padding: var(--tr-space-2) var(--tr-space-4) 0;
  }
  .cols {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 270px minmax(0, 1fr);
  }
  .list {
    border-right: 1px solid var(--tr-outline-variant);
    overflow-y: auto;
    padding: var(--tr-space-2);
    display: grid;
    gap: var(--tr-space-2);
    align-content: start;
  }
  .nm {
    font-weight: 600;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--tr-danger);
    align-self: center;
  }
  .add {
    display: flex;
    gap: var(--tr-space-1);
    flex-wrap: wrap;
    align-items: center;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .detail {
    overflow-y: auto;
    padding: var(--tr-space-3) var(--tr-space-5) 40px;
    min-width: 0;
  }
  .detail h2 {
    margin: 0 0 var(--tr-space-2);
    font-size: var(--tr-text-lg);
  }
  .detail h3 {
    margin: var(--tr-space-4) 0 var(--tr-space-1);
    font-size: var(--tr-text-md);
  }
  .mhead {
    display: flex;
    gap: var(--tr-space-2);
    align-items: center;
    flex-wrap: wrap;
    margin-bottom: var(--tr-space-2);
  }
  .mhead h2 {
    margin: 0;
    flex: 1;
  }
  .def {
    max-width: 320px;
  }
  .small {
    font-size: var(--tr-text-sm);
    margin: 0;
  }
  .problems {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: var(--tr-text-sm);
  }
  .problems li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--tr-space-1) var(--tr-space-2);
  }
  .review {
    display: grid;
    gap: var(--tr-space-2);
    align-content: start;
  }
  @media (max-width: 760px) {
    .cols {
      grid-template-columns: minmax(0, 1fr);
    }
    .list {
      border-right: none;
      border-bottom: 1px solid var(--tr-outline-variant);
      max-height: 40vh;
    }
  }
</style>
