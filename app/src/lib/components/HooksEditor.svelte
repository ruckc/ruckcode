<script lang="ts">
  import {
    DEFAULT_MAX_FIX_TURNS,
    HOOK_EVENT_NAMES,
    type Catalog,
    type HookEventName,
    type Issue,
    type ModeDto,
    type OnFailure
  } from '../host/modes';
  import { issuesAt, newHook } from '../modes/doc';

  let {
    hooks = $bindable(),
    catalog,
    issues,
    base,
    readonly = false
  }: {
    hooks: ModeDto['hooks'];
    catalog: Catalog | null;
    issues: Issue[];
    /** Path prefix of the issues for these hooks, e.g. `modes[2].hooks`. */
    base: string;
    /** Display only (the project's global hooks). */
    readonly?: boolean;
  } = $props();

  const allEvents = $derived(
    catalog?.hookEvents ??
      HOOK_EVENT_NAMES.map((name) => ({
        name,
        glob: name === 'post_edit',
        matcher: /tool_use/.test(name),
        description: ''
      }))
  );

  // The read-only view lists only the events that have hooks.
  const events = $derived(readonly ? allEvents.filter((e) => hooks[e.name]?.length) : allEvents);

  function add(ev: HookEventName) {
    hooks[ev] = [...(hooks[ev] ?? []), newHook()];
  }

  function remove(ev: HookEventName, i: number) {
    const rest = (hooks[ev] ?? []).filter((_, j) => j !== i);
    if (rest.length) hooks[ev] = rest;
    else delete hooks[ev];
  }

  function move(ev: HookEventName, i: number, d: number) {
    const list = [...(hooks[ev] ?? [])];
    const j = i + d;
    if (j < 0 || j >= list.length) return;
    [list[i], list[j]] = [list[j], list[i]];
    hooks[ev] = list;
  }

  const at = (ev: HookEventName, i: number, field: string) =>
    issuesAt(issues, `${base}.${ev}[${i}].${field}`);
</script>

{#if !readonly}
  <p class="muted help">
    Hooks run a shell command in the project root. In a <code>post_edit</code> hook,
    <code>{'{files}'}</code> expands to the (quoted) files that were edited and
    <em>glob</em> limits it to matching files. <em>Matcher</em> is an anchored regex on the tool
    name for tool hooks (<code>Bash|Edit</code>). When a hook fails, <em>ignore</em> carries on,
    <em>block</em> stops the edit or tool call, and <em>fix_turn</em> hands the output to the agent for
    up to the given number of fix turns.
  </p>
{/if}

{#each events as info (info.name)}
  {@const list = hooks[info.name] ?? []}
  <details open={list.length > 0} class="event">
    <summary>
      <code>{info.name}</code>
      <span class="count">{list.length}</span>
      <span class="muted small">{info.description}</span>
    </summary>
    {#if list.length}
      <ol class="hooks">
        {#each list as h, i (i)}
          <li class="hook">
            <div class="grid">
              <label class="wide"
                >Command
                <input
                  type="text"
                  class="mono"
                  bind:value={h.run}
                  {readonly}
                  spellcheck="false"
                  aria-label={`Command for ${info.name} hook ${i + 1}`}
                  placeholder={info.glob ? 'mise run fmt {files}' : 'mise run check'}
                />
              </label>
              {#each at(info.name, i, 'run') as m (m.message)}<span class="msg {m.severity}"
                  >{m.message}</span
                >{/each}
              {#if info.glob || h.glob}
                <label
                  >File glob
                  <input
                    type="text"
                    class="mono"
                    value={h.glob ?? ''}
                    oninput={(e) => (h.glob = e.currentTarget.value || null)}
                    {readonly}
                    spellcheck="false"
                    aria-label={`Glob for ${info.name} hook ${i + 1}`}
                    placeholder="*.rs"
                  />
                </label>
                {#each at(info.name, i, 'glob') as m (m.message)}<span class="msg {m.severity}"
                    >{m.message}</span
                  >{/each}
              {/if}
              {#if info.matcher || h.matcher}
                <label
                  >Tool matcher (regex)
                  <input
                    type="text"
                    class="mono"
                    value={h.matcher ?? ''}
                    oninput={(e) => (h.matcher = e.currentTarget.value || null)}
                    {readonly}
                    spellcheck="false"
                    aria-label={`Matcher for ${info.name} hook ${i + 1}`}
                    aria-invalid={at(info.name, i, 'matcher').some((m) => m.severity === 'error')}
                    placeholder="Bash|Edit"
                  />
                </label>
                {#each at(info.name, i, 'matcher') as m (m.message)}<span class="msg {m.severity}"
                    >{m.message}</span
                  >{/each}
              {/if}
              <label
                >On failure
                <select
                  value={h.onFailure}
                  disabled={readonly}
                  aria-label={`On failure for ${info.name} hook ${i + 1}`}
                  onchange={(e) => (h.onFailure = e.currentTarget.value as OnFailure)}
                >
                  <option value="ignore">ignore</option>
                  <option value="block">block</option>
                  <option value="fix_turn">fix_turn</option>
                </select>
              </label>
              {#if h.onFailure === 'fix_turn'}
                <label
                  >Fix turns
                  <input
                    type="number"
                    min="0"
                    max="20"
                    value={h.maxFixTurns}
                    {readonly}
                    aria-label={`Max fix turns for ${info.name} hook ${i + 1}`}
                    oninput={(e) => (h.maxFixTurns = e.currentTarget.valueAsNumber || 0)}
                  />
                </label>
                {#each at(info.name, i, 'maxFixTurns') as m (m.message)}<span
                    class="msg {m.severity}">{m.message}</span
                  >{/each}
              {/if}
              <label
                >Timeout (s)
                <input
                  type="number"
                  min="1"
                  value={h.timeoutSecs}
                  {readonly}
                  aria-label={`Timeout for ${info.name} hook ${i + 1}`}
                  oninput={(e) => (h.timeoutSecs = e.currentTarget.valueAsNumber || 0)}
                />
              </label>
              {#each at(info.name, i, 'timeoutSecs') as m (m.message)}<span class="msg {m.severity}"
                  >{m.message}</span
                >{/each}
            </div>
            {#if !readonly}
              <div class="ctl">
                <button
                  type="button"
                  class="small"
                  disabled={i === 0}
                  aria-label={`Move ${info.name} hook ${i + 1} up`}
                  onclick={() => move(info.name, i, -1)}>&uarr;</button
                >
                <button
                  type="button"
                  class="small"
                  disabled={i === list.length - 1}
                  aria-label={`Move ${info.name} hook ${i + 1} down`}
                  onclick={() => move(info.name, i, 1)}>&darr;</button
                >
                <button
                  type="button"
                  class="small danger"
                  aria-label={`Remove ${info.name} hook ${i + 1}`}
                  onclick={() => remove(info.name, i)}>Remove</button
                >
              </div>
            {/if}
          </li>
        {/each}
      </ol>
    {/if}
    {#if !readonly}
      <button type="button" class="small" onclick={() => add(info.name)}
        >Add {info.name} hook</button
      >
    {/if}
  </details>
{/each}
<span class="sr-only">Default fix turns: {DEFAULT_MAX_FIX_TURNS}</span>

<style>
  .help {
    margin: 0 0 6px;
    font-size: 12.5px;
  }
  .event {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 4px 8px;
    margin-bottom: 6px;
  }
  summary {
    cursor: pointer;
    display: flex;
    gap: 8px;
    align-items: baseline;
  }
  .count {
    font-size: 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0 6px;
  }
  .small {
    font-size: 12px;
  }
  .hooks {
    list-style: none;
    margin: 6px 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }
  .hook {
    border-left: 3px solid var(--border);
    padding-left: 8px;
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }
  .grid {
    flex: 1;
    min-width: 0;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 4px 10px;
  }
  .grid label {
    display: grid;
    gap: 2px;
    font-size: 12px;
    color: var(--muted);
  }
  .grid label.wide,
  .msg {
    grid-column: 1 / -1;
  }
  .grid input,
  .grid select {
    width: 100%;
    color: var(--text);
  }
  .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12.5px;
  }
  .msg {
    font-size: 12px;
  }
  .msg.error {
    color: var(--err);
  }
  .msg.warning {
    color: var(--warn);
  }
  .ctl {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  input[aria-invalid='true'] {
    border-color: var(--err);
  }
</style>
