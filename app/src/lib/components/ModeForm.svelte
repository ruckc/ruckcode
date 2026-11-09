<script lang="ts">
  import { Badge, Button, Callout, Checkbox, Input, Select, Textarea } from '@ruckc/truss-ui';
  import type { ModesEditor } from '../state/modes.svelte';
  import { displayName, issuesAt, toggle } from '../modes/doc';
  import { globMatches, globProblem, globToRegExp } from '../modes/glob';
  import ChipInput from './ChipInput.svelte';
  import HooksEditor from './HooksEditor.svelte';
  import IssueList from './IssueList.svelte';
  import PermissionsTable from './PermissionsTable.svelte';

  let { editor }: { editor: ModesEditor } = $props();

  const m = $derived(editor.selectedMode);
  const idx = $derived(editor.selectedIndex);
  const p = $derived(`modes[${idx}]`);
  const cat = $derived(editor.catalog);
  const builtin = $derived(cat?.builtinTools ?? []);
  const builtinNames = $derived(builtin.map((t) => t.name));
  const taskNames = $derived(cat?.miseTasks.map((t) => t.name) ?? []);
  const servers = $derived(cat?.mcpServers ?? []);
  const others = $derived(editor.doc.modes.filter((x) => x.id !== m?.id));

  // the id field is edited as a draft and applied with Rename (so references move once)
  let idDraft = $derived(m?.id ?? '');
  const idProblem = $derived(m ? editor.idProblem(m.id, idDraft.trim()) : null);
  const refCount = $derived(m ? editor.references(m.id).length : 0);
  const idHint = $derived(
    refCount
      ? `Renaming also updates ${refCount} reference${refCount === 1 ? '' : 's'} in other modes and the default.`
      : 'The id is used in next, auto_advance_to and the default mode.'
  );

  let confirmDelete = $derived(m ? false : false);

  const permErrors = $derived(
    Object.fromEntries(
      issuesAt(editor.issues, `${p}.permissions`)
        .filter((i) => i.severity === 'error')
        .map((i) => [i.path.slice(`${p}.permissions.`.length), i.message])
    )
  );

  function taskPreview(g: string): string | null {
    const r = globMatches(g, taskNames);
    if (!r) return null;
    if (!taskNames.length) return 'no tasks discovered';
    return r.length
      ? `matches ${r.length}: ${r.slice(0, 4).join(', ')}${r.length > 4 ? ', ...' : ''}`
      : 'matches no discovered task';
  }

  function mcpPreview(g: string): string | null {
    const re = globToRegExp(g);
    if (!re) return null;
    const hit = servers.filter((s) => re.test(`mcp__${s}__tool`));
    if (g === 'mcp__*') return 'all MCP tools';
    return hit.length
      ? `server${hit.length === 1 ? '' : 's'}: ${hit.join(', ')}`
      : 'no configured server matches';
  }

  const extraTools = $derived((m?.tools ?? []).filter((t) => !builtinNames.includes(t)));
  function setExtraTools(v: string[]) {
    if (!m || m.tools === null) return;
    m.tools = [...m.tools.filter((t) => builtinNames.includes(t)), ...v];
  }

  function allTools(on: boolean) {
    if (!m) return;
    m.tools = on ? null : [...builtinNames];
  }

  const models = $derived.by(() => {
    const profiles = cat?.profiles ?? [];
    if (m?.profile) return profiles.find((x) => x.name === m.profile)?.models ?? [];
    return [...new Set(profiles.flatMap((x) => x.models))];
  });
  let customModel = $state(false);
  const modelValue = $derived(customModel ? '__custom__' : (m?.model ?? ''));
  $effect(() => {
    void m?.id;
    customModel = !!m?.model && !models.includes(m.model);
  });

  const profileOptions = $derived([
    { value: '', label: 'Default (session profile)' },
    ...(cat?.profiles ?? []).map((pr) => ({ value: pr.name, label: pr.name })),
    ...(m?.profile && !(cat?.profiles ?? []).some((x) => x.name === m.profile)
      ? [{ value: m.profile, label: `${m.profile} (not configured)` }]
      : [])
  ]);
  const modelOptions = $derived([
    { value: '', label: "Default (profile's model)" },
    ...models.map((mo) => ({ value: mo, label: mo })),
    { value: '__custom__', label: 'Other...' }
  ]);
</script>

{#if m}
  <div class="form" aria-label={`Mode ${m.id}`}>
    <section aria-labelledby="sec-general">
      <h3 id="sec-general">General</h3>
      <div class="two">
        <Input
          label="Display name"
          value={m.name ?? ''}
          placeholder={displayName({ id: m.id, name: null })}
          oninput={(e) => (m.name = e.currentTarget.value || null)}
        />
        <div class="idrow">
          <div class="grow">
            <Input
              label="Id"
              value={idDraft}
              oninput={(e) => (idDraft = e.currentTarget.value)}
              spellcheck="false"
              error={idProblem ?? undefined}
              hint={idHint}
            />
          </div>
          <div class="rename">
            <Button
              variant="outlined"
              disabled={idDraft.trim() === m.id || !!idProblem}
              onclick={() => editor.rename(m.id, idDraft.trim())}>Rename</Button
            >
          </div>
        </div>
      </div>
      <IssueList issues={issuesAt(editor.issues, `${p}.id`)} />
      <Input
        label="Description"
        value={m.description}
        oninput={(e) => (m.description = e.currentTarget.value)}
        placeholder="Shown in the mode switcher"
      />
    </section>

    <section aria-labelledby="sec-prompt">
      <h3 id="sec-prompt">Prompt</h3>
      <p class="muted small">Appended to the system prompt while this mode is active.</p>
      <Textarea mono rows={10} aria-label="Prompt" bind:value={m.prompt} spellcheck={true} />
      <p class="muted small" data-testid="prompt-count">
        {m.prompt.length} character{m.prompt.length === 1 ? '' : 's'} &middot;
        {m.prompt ? m.prompt.split('\n').length : 0} line{m.prompt.split('\n').length === 1
          ? ''
          : 's'}
      </p>
    </section>

    <section aria-labelledby="sec-tools">
      <h3 id="sec-tools">Tools</h3>
      <Checkbox
        label="All tools"
        checked={m.tools === null}
        oncheckedchange={(on) => allTools(on)}
      />
      {#if m.tools !== null}
        <div class="quick">
          <Button
            size="sm"
            variant="outlined"
            intent="neutral"
            onclick={() =>
              (m.tools = [...builtin.filter((t) => t.readOnly).map((t) => t.name), ...extraTools])}
            >Read-only only</Button
          >
          <Button
            size="sm"
            variant="outlined"
            intent="neutral"
            onclick={() => (m.tools = [...builtinNames, ...extraTools])}>Select all</Button
          >
          <Button
            size="sm"
            variant="outlined"
            intent="neutral"
            onclick={() => (m.tools = [...extraTools])}>Select none</Button
          >
        </div>
        <ul class="tools" aria-label="Built-in tools">
          {#each builtin as t (t.name)}
            <li>
              <Checkbox
                checked={m.tools.includes(t.name)}
                oncheckedchange={(on) => (m.tools = toggle(m.tools ?? [], t.name, on))}
              >
                <span class="inl">
                  <code>{t.name}</code>
                  {#if t.readOnly}<Badge intent="success" dot={false}>read-only</Badge>
                  {:else}<Badge dot={false}>{t.kind}</Badge>{/if}
                </span>
              </Checkbox>
            </li>
          {/each}
        </ul>
        <p class="muted small">Other tool names (for example <code>mise__test</code>):</p>
        <ChipInput
          bind:values={() => extraTools, setExtraTools}
          label="Other tools"
          placeholder="Tool name"
        />
      {:else}
        <p class="muted small">Every built-in tool is available in this mode.</p>
      {/if}
      <IssueList issues={issuesAt(editor.issues, `${p}.tools`)} />
    </section>

    <section aria-labelledby="sec-mise">
      <h3 id="sec-mise">Mise tasks</h3>
      <p class="muted small">
        Task names or globs (<code>test*</code>) the agent can run as tools in this mode.
        {taskNames.length} task{taskNames.length === 1 ? '' : 's'} discovered.
      </p>
      <ChipInput
        bind:values={m.miseTasks}
        label="Mise tasks"
        suggestions={taskNames}
        placeholder="Task or glob, e.g. test*"
        problem={(v) => globProblem(v) && `Invalid glob: ${globProblem(v)}`}
        preview={taskPreview}
      />
      <IssueList issues={issuesAt(editor.issues, `${p}.miseTasks`)} />
    </section>

    <section aria-labelledby="sec-mcp">
      <h3 id="sec-mcp">MCP tools</h3>
      <p class="muted small">
        Globs over full MCP tool names (<code>mcp__&lt;server&gt;__&lt;tool&gt;</code>). Empty: no
        MCP tools in this mode.
      </p>
      <ChipInput
        bind:values={m.mcpTools}
        label="MCP tools"
        suggestions={['mcp__*', ...servers.map((s) => `mcp__${s}__*`)]}
        placeholder="Glob, e.g. mcp__github__*"
        problem={(v) => globProblem(v) && `Invalid glob: ${globProblem(v)}`}
        preview={mcpPreview}
      />
      <IssueList issues={issuesAt(editor.issues, `${p}.mcpTools`)} />
    </section>

    <section aria-labelledby="sec-next">
      <h3 id="sec-next">Transitions</h3>
      {#if others.length}
        <div class="two">
          <!-- TODO(truss-ui#149): no Fieldset/CheckboxGroup component (RadioGroup is the only grouped control). -->
          <fieldset>
            <legend>Can switch to</legend>
            <ul class="plain">
              {#each others as o (o.id)}
                <li>
                  <Checkbox
                    checked={m.next.includes(o.id)}
                    oncheckedchange={(on) => (m.next = toggle(m.next, o.id, on))}
                    aria-label={`next: ${o.id}`}
                  >
                    {displayName(o)} <code>{o.id}</code>
                  </Checkbox>
                </li>
              {/each}
            </ul>
            <p class="muted small">None selected: SwitchMode may target any other mode.</p>
          </fieldset>
          <fieldset>
            <legend>Auto-advance to</legend>
            <ul class="plain">
              {#each others as o (o.id)}
                <li>
                  <Checkbox
                    checked={m.autoAdvanceTo.includes(o.id)}
                    oncheckedchange={(on) => (m.autoAdvanceTo = toggle(m.autoAdvanceTo, o.id, on))}
                    aria-label={`auto-advance: ${o.id}`}
                  >
                    {displayName(o)} <code>{o.id}</code>
                  </Checkbox>
                </li>
              {/each}
            </ul>
            <p class="muted small">These switches happen without asking you first.</p>
          </fieldset>
        </div>
      {:else}
        <p class="muted small">Add another mode to define transitions.</p>
      {/if}
      <IssueList issues={issuesAt(editor.issues, `${p}.next`)} />
      <IssueList issues={issuesAt(editor.issues, `${p}.autoAdvanceTo`)} />
    </section>

    <section aria-labelledby="sec-model">
      <h3 id="sec-model">Profile and model</h3>
      <div class="two">
        <Select
          label="Profile"
          searchable={false}
          options={profileOptions}
          value={m.profile ?? ''}
          onchange={(v) => {
            m.profile = v || null;
            m.model = null;
          }}
        />
        <Select
          label="Model"
          searchable={false}
          options={modelOptions}
          value={modelValue}
          onchange={(v) => {
            customModel = v === '__custom__';
            if (!customModel) m.model = v || null;
          }}
        />
      </div>
      {#if customModel}
        <Input
          label="Model id"
          value={m.model ?? ''}
          oninput={(e) => (m.model = e.currentTarget.value.trim() || null)}
          spellcheck="false"
          placeholder="e.g. claude-haiku-5-5"
        />
      {/if}
      <IssueList issues={issuesAt(editor.issues, `${p}.profile`)} />
    </section>

    <section aria-labelledby="sec-perms">
      <h3 id="sec-perms">Permissions</h3>
      <p class="muted small">
        Per tool (or glob) override of the default: allow runs without asking, ask prompts you, deny
        refuses. Exact names beat globs.
      </p>
      <PermissionsTable bind:permissions={m.permissions} catalog={cat} errors={permErrors} />
    </section>

    <section aria-labelledby="sec-hooks">
      <h3 id="sec-hooks">Hooks</h3>
      <HooksEditor bind:hooks={m.hooks} catalog={cat} issues={editor.issues} base={`${p}.hooks`} />
    </section>

    <section aria-labelledby="sec-danger" class="danger-zone">
      <h3 id="sec-danger">Remove</h3>
      {#if confirmDelete}
        <Callout intent="danger">
          Delete mode <code>{m.id}</code>?
          {#if refCount}
            It is referenced in {refCount} place{refCount === 1 ? '' : 's'}
            ({editor
              .references(m.id)
              .map((r) => (r.from ? `${r.from}.${r.field}` : 'default mode'))
              .join(', ')}); those references are removed too.
          {/if}
        </Callout>
        <div class="row">
          <Button
            intent="danger"
            onclick={() => {
              editor.remove(m.id);
              confirmDelete = false;
            }}>Delete mode</Button
          >
          <Button variant="outlined" intent="neutral" onclick={() => (confirmDelete = false)}
            >Cancel</Button
          >
        </div>
      {:else}
        <div class="row">
          <Button variant="outlined" intent="danger" onclick={() => (confirmDelete = true)}
            >Delete mode...</Button
          >
        </div>
      {/if}
    </section>
  </div>
{/if}

<style>
  .form {
    display: grid;
    gap: var(--tr-space-6);
  }
  section h3 {
    margin: 0 0 var(--tr-space-1);
    font-size: var(--tr-text-md);
  }
  section {
    display: grid;
    gap: var(--tr-space-2);
    align-content: start;
  }
  .two {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: var(--tr-space-2) var(--tr-space-4);
    align-items: start;
  }
  .idrow {
    display: flex;
    gap: var(--tr-space-2);
    align-items: flex-start;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  /* The Rename button lines up with the input, not the label above it. */
  .rename {
    margin-top: calc(var(--tr-text-sm) * 1.5 + 4px);
  }
  .small {
    font-size: var(--tr-text-sm);
    margin: 0;
  }
  .quick,
  .row {
    display: flex;
    gap: var(--tr-space-2);
    flex-wrap: wrap;
  }
  .inl {
    display: inline-flex;
    gap: var(--tr-space-2);
    align-items: center;
  }
  .tools,
  .plain {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .tools {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: var(--tr-space-1) var(--tr-space-3);
  }
  fieldset {
    border: 1px solid var(--tr-outline-variant);
    border-radius: var(--tr-radius-md);
    margin: 0;
    padding: var(--tr-space-1) var(--tr-space-3) var(--tr-space-2);
  }
  legend {
    font-size: var(--tr-text-sm);
    color: var(--tr-on-surface-variant);
    padding: 0 var(--tr-space-1);
  }
  .danger-zone {
    border-top: 1px solid var(--tr-outline-variant);
    padding-top: var(--tr-space-2);
  }
</style>
