<script lang="ts">
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
</script>

{#if m}
  <div class="form" aria-label={`Mode ${m.id}`}>
    <section aria-labelledby="sec-general">
      <h3 id="sec-general">General</h3>
      <div class="two">
        <label
          >Display name
          <input
            type="text"
            value={m.name ?? ''}
            placeholder={displayName({ id: m.id, name: null })}
            oninput={(e) => (m.name = e.currentTarget.value || null)}
          />
        </label>
        <div class="idrow">
          <label
            >Id
            <input
              type="text"
              bind:value={idDraft}
              spellcheck="false"
              aria-invalid={!!idProblem}
              aria-describedby="id-help"
            />
          </label>
          <button
            type="button"
            disabled={idDraft.trim() === m.id || !!idProblem}
            onclick={() => editor.rename(m.id, idDraft.trim())}>Rename</button
          >
        </div>
      </div>
      <p id="id-help" class="muted small">
        {#if idProblem}<span class="err">{idProblem}</span>
        {:else if refCount}Renaming also updates {refCount} reference{refCount === 1 ? '' : 's'} in other
          modes and the default.
        {:else}The id is used in <code>next</code>, <code>auto_advance_to</code> and the default mode.{/if}
      </p>
      <IssueList issues={issuesAt(editor.issues, `${p}.id`)} />
      <label
        >Description
        <input type="text" bind:value={m.description} placeholder="Shown in the mode switcher" />
      </label>
    </section>

    <section aria-labelledby="sec-prompt">
      <h3 id="sec-prompt">Prompt</h3>
      <p class="muted small">Appended to the system prompt while this mode is active.</p>
      <textarea class="prompt" rows="10" aria-label="Prompt" bind:value={m.prompt} spellcheck="true"
      ></textarea>
      <p class="muted small" data-testid="prompt-count">
        {m.prompt.length} character{m.prompt.length === 1 ? '' : 's'} &middot;
        {m.prompt ? m.prompt.split('\n').length : 0} line{m.prompt.split('\n').length === 1
          ? ''
          : 's'}
      </p>
    </section>

    <section aria-labelledby="sec-tools">
      <h3 id="sec-tools">Tools</h3>
      <label class="check"
        ><input
          type="checkbox"
          checked={m.tools === null}
          onchange={(e) => allTools(e.currentTarget.checked)}
        /> All tools</label
      >
      {#if m.tools !== null}
        <div class="quick">
          <button
            type="button"
            class="small"
            onclick={() =>
              (m.tools = [...builtin.filter((t) => t.readOnly).map((t) => t.name), ...extraTools])}
            >Read-only only</button
          >
          <button
            type="button"
            class="small"
            onclick={() => (m.tools = [...builtinNames, ...extraTools])}>Select all</button
          >
          <button type="button" class="small" onclick={() => (m.tools = [...extraTools])}
            >Select none</button
          >
        </div>
        <ul class="tools" aria-label="Built-in tools">
          {#each builtin as t (t.name)}
            <li>
              <label class="check"
                ><input
                  type="checkbox"
                  checked={m.tools.includes(t.name)}
                  onchange={(e) =>
                    (m.tools = toggle(m.tools ?? [], t.name, e.currentTarget.checked))}
                />
                <code>{t.name}</code>
                {#if t.readOnly}<span class="badge ro">read-only</span>
                {:else}<span class="badge">{t.kind}</span>{/if}
              </label>
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
          <fieldset>
            <legend>Can switch to</legend>
            <ul class="plain">
              {#each others as o (o.id)}
                <li>
                  <label class="check"
                    ><input
                      type="checkbox"
                      checked={m.next.includes(o.id)}
                      onchange={(e) => (m.next = toggle(m.next, o.id, e.currentTarget.checked))}
                      aria-label={`next: ${o.id}`}
                    />
                    {displayName(o)} <code>{o.id}</code></label
                  >
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
                  <label class="check"
                    ><input
                      type="checkbox"
                      checked={m.autoAdvanceTo.includes(o.id)}
                      onchange={(e) =>
                        (m.autoAdvanceTo = toggle(m.autoAdvanceTo, o.id, e.currentTarget.checked))}
                      aria-label={`auto-advance: ${o.id}`}
                    />
                    {displayName(o)} <code>{o.id}</code></label
                  >
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
        <label
          >Profile
          <select
            value={m.profile ?? ''}
            onchange={(e) => {
              m.profile = e.currentTarget.value || null;
              m.model = null;
            }}
          >
            <option value="">Default (session profile)</option>
            {#each cat?.profiles ?? [] as pr (pr.name)}<option value={pr.name}>{pr.name}</option
              >{/each}
            {#if m.profile && !(cat?.profiles ?? []).some((x) => x.name === m.profile)}
              <option value={m.profile}>{m.profile} (not configured)</option>
            {/if}
          </select>
        </label>
        <label
          >Model
          <select
            value={modelValue}
            onchange={(e) => {
              const v = e.currentTarget.value;
              customModel = v === '__custom__';
              if (!customModel) m.model = v || null;
            }}
          >
            <option value="">Default (profile's model)</option>
            {#each models as mo (mo)}<option value={mo}>{mo}</option>{/each}
            <option value="__custom__">Other...</option>
          </select>
        </label>
      </div>
      {#if customModel}
        <label
          >Model id
          <input
            type="text"
            value={m.model ?? ''}
            oninput={(e) => (m.model = e.currentTarget.value.trim() || null)}
            spellcheck="false"
            placeholder="e.g. claude-haiku-5-5"
          />
        </label>
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
        <p role="alert">
          Delete mode <code>{m.id}</code>?
          {#if refCount}
            It is referenced in {refCount} place{refCount === 1 ? '' : 's'}
            ({editor
              .references(m.id)
              .map((r) => (r.from ? `${r.from}.${r.field}` : 'default mode'))
              .join(', ')}); those references are removed too.
          {/if}
        </p>
        <div class="actions">
          <button
            type="button"
            class="danger"
            onclick={() => {
              editor.remove(m.id);
              confirmDelete = false;
            }}>Delete mode</button
          >
          <button type="button" onclick={() => (confirmDelete = false)}>Cancel</button>
        </div>
      {:else}
        <button type="button" class="danger" onclick={() => (confirmDelete = true)}
          >Delete mode...</button
        >
      {/if}
    </section>
  </div>
{/if}

<style>
  .form {
    display: grid;
    gap: 18px;
  }
  section h3 {
    margin: 0 0 4px;
    font-size: 14px;
  }
  section {
    display: grid;
    gap: 4px;
    align-content: start;
  }
  label {
    display: grid;
    gap: 2px;
    font-size: 12px;
    color: var(--muted);
  }
  label input[type='text'],
  label select {
    color: var(--text);
    font-size: 14px;
  }
  label.check {
    display: flex;
    gap: 6px;
    align-items: baseline;
    color: var(--text);
    font-size: 14px;
  }
  .two {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 8px 14px;
    align-items: start;
  }
  .idrow {
    display: flex;
    gap: 6px;
    align-items: end;
  }
  .idrow label {
    flex: 1;
  }
  .prompt {
    width: 100%;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12.5px;
    resize: vertical;
    line-height: 1.45;
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
  .err {
    color: var(--err);
  }
  .quick {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .tools,
  .plain {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .tools {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 2px 12px;
  }
  .badge {
    font-size: 11px;
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0 6px;
    color: var(--muted);
  }
  .badge.ro {
    color: var(--ok);
    border-color: var(--ok);
  }
  fieldset {
    border: 1px solid var(--border);
    border-radius: 6px;
    margin: 0;
    padding: 4px 10px 8px;
  }
  legend {
    font-size: 12px;
    color: var(--muted);
    padding: 0 4px;
  }
  input[aria-invalid='true'] {
    border-color: var(--err);
  }
  .danger-zone {
    border-top: 1px solid var(--border);
    padding-top: 8px;
  }
</style>
