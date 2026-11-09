<script lang="ts">
  import { Badge, Button, Input, Select } from '@ruckc/truss-ui';
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

  const failureOptions = (['ignore', 'block', 'fix_turn'] as OnFailure[]).map((o) => ({
    value: o,
    label: o
  }));

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
  /** Errors go on the field (red, aria-invalid); warnings and notes are listed under it. */
  const errorAt = (ev: HookEventName, i: number, field: string) =>
    at(ev, i, field)
      .filter((m) => m.severity === 'error')
      .map((m) => m.message)
      .join(' ') || undefined;
  const notesAt = (ev: HookEventName, i: number, field: string) =>
    at(ev, i, field).filter((m) => m.severity !== 'error');
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
  <!-- TODO(truss-ui): consider Accordion; Accordion would fit, but the tests and e2e rely on a native <details>/<summary>. -->
  <details open={list.length > 0} class="event">
    <summary>
      <code>{info.name}</code>
      <Badge dot={false}>{list.length}</Badge>
      <span class="muted small">{info.description}</span>
    </summary>
    {#if list.length}
      <ol class="hooks">
        {#each list as h, i (i)}
          <li class="hook">
            <div class="grid">
              <div class="wide">
                <Input
                  size="sm"
                  label="Command"
                  value={h.run}
                  oninput={(e) => (h.run = e.currentTarget.value)}
                  {readonly}
                  spellcheck="false"
                  aria-label={`Command for ${info.name} hook ${i + 1}`}
                  error={errorAt(info.name, i, 'run')}
                  placeholder={info.glob ? 'mise run fmt {files}' : 'mise run check'}
                  style="font-family: var(--tr-font-mono)"
                />
              </div>
              {#each notesAt(info.name, i, 'run') as m (m.message)}<span class="msg {m.severity}"
                  >{m.message}</span
                >{/each}
              {#if info.glob || h.glob}
                <Input
                  size="sm"
                  label="File glob"
                  value={h.glob ?? ''}
                  oninput={(e) => (h.glob = e.currentTarget.value || null)}
                  {readonly}
                  spellcheck="false"
                  aria-label={`Glob for ${info.name} hook ${i + 1}`}
                  error={errorAt(info.name, i, 'glob')}
                  placeholder="*.rs"
                  style="font-family: var(--tr-font-mono)"
                />
                {#each notesAt(info.name, i, 'glob') as m (m.message)}<span class="msg {m.severity}"
                    >{m.message}</span
                  >{/each}
              {/if}
              {#if info.matcher || h.matcher}
                <Input
                  size="sm"
                  label="Tool matcher (regex)"
                  value={h.matcher ?? ''}
                  oninput={(e) => (h.matcher = e.currentTarget.value || null)}
                  {readonly}
                  spellcheck="false"
                  aria-label={`Matcher for ${info.name} hook ${i + 1}`}
                  error={errorAt(info.name, i, 'matcher')}
                  placeholder="Bash|Edit"
                  style="font-family: var(--tr-font-mono)"
                />
                {#each notesAt(info.name, i, 'matcher') as m (m.message)}<span
                    class="msg {m.severity}">{m.message}</span
                  >{/each}
              {/if}
              <div>
                <!-- The visible text is separate from the accessible name ("On failure for <event> hook N"),
                     which truss Select can only set via ariaLabel when it has no visible `label`. -->
                <span class="tr-label" aria-hidden="true">On failure</span>
                <Select
                  size="sm"
                  searchable={false}
                  disabled={readonly}
                  ariaLabel={`On failure for ${info.name} hook ${i + 1}`}
                  options={failureOptions}
                  value={h.onFailure}
                  onchange={(v) => (h.onFailure = v)}
                />
              </div>
              {#if h.onFailure === 'fix_turn'}
                <Input
                  size="sm"
                  label="Fix turns"
                  type="number"
                  min="0"
                  max="20"
                  value={h.maxFixTurns}
                  {readonly}
                  aria-label={`Max fix turns for ${info.name} hook ${i + 1}`}
                  error={errorAt(info.name, i, 'maxFixTurns')}
                  oninput={(e) => (h.maxFixTurns = e.currentTarget.valueAsNumber || 0)}
                />
                {#each notesAt(info.name, i, 'maxFixTurns') as m (m.message)}<span
                    class="msg {m.severity}">{m.message}</span
                  >{/each}
              {/if}
              <Input
                size="sm"
                label="Timeout (s)"
                type="number"
                min="1"
                value={h.timeoutSecs}
                {readonly}
                aria-label={`Timeout for ${info.name} hook ${i + 1}`}
                error={errorAt(info.name, i, 'timeoutSecs')}
                oninput={(e) => (h.timeoutSecs = e.currentTarget.valueAsNumber || 0)}
              />
              {#each notesAt(info.name, i, 'timeoutSecs') as m (m.message)}<span
                  class="msg {m.severity}">{m.message}</span
                >{/each}
            </div>
            {#if !readonly}
              <div class="ctl">
                <Button
                  size="sm"
                  variant="minimal"
                  intent="neutral"
                  icon="arrow-up"
                  disabled={i === 0}
                  aria-label={`Move ${info.name} hook ${i + 1} up`}
                  onclick={() => move(info.name, i, -1)}
                />
                <Button
                  size="sm"
                  variant="minimal"
                  intent="neutral"
                  icon="arrow-down"
                  disabled={i === list.length - 1}
                  aria-label={`Move ${info.name} hook ${i + 1} down`}
                  onclick={() => move(info.name, i, 1)}
                />
                <Button
                  size="sm"
                  variant="outlined"
                  intent="danger"
                  icon="trash"
                  aria-label={`Remove ${info.name} hook ${i + 1}`}
                  onclick={() => remove(info.name, i)}>Remove</Button
                >
              </div>
            {/if}
          </li>
        {/each}
      </ol>
    {/if}
    {#if !readonly}
      <Button size="sm" variant="outlined" icon="plus" onclick={() => add(info.name)}
        >Add {info.name} hook</Button
      >
    {/if}
  </details>
{/each}
<span class="tr-visually-hidden">Default fix turns: {DEFAULT_MAX_FIX_TURNS}</span>

<style>
  .help {
    margin: 0 0 var(--tr-space-2);
    font-size: var(--tr-text-sm);
  }
  .event {
    border: 1px solid var(--tr-outline-variant);
    border-radius: var(--tr-radius-md);
    padding: var(--tr-space-1) var(--tr-space-2);
    margin-bottom: var(--tr-space-2);
  }
  summary {
    cursor: pointer;
    display: flex;
    gap: var(--tr-space-2);
    align-items: center;
    min-height: var(--tr-control-h-sm);
  }
  .small {
    font-size: var(--tr-text-sm);
  }
  .hooks {
    list-style: none;
    margin: var(--tr-space-2) 0;
    padding: 0;
    display: grid;
    gap: var(--tr-space-2);
  }
  .hook {
    border-left: 3px solid var(--tr-outline-variant);
    padding-left: var(--tr-space-2);
    display: flex;
    gap: var(--tr-space-2);
    align-items: flex-start;
  }
  .grid {
    flex: 1;
    min-width: 0;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: var(--tr-space-1) var(--tr-space-3);
  }
  .wide,
  .msg {
    grid-column: 1 / -1;
  }
  .msg {
    font-size: var(--tr-text-sm);
  }
  .msg.warning {
    color: var(--tr-warning);
  }
  .msg.info {
    color: var(--tr-on-surface-variant);
  }
  .ctl {
    display: flex;
    gap: var(--tr-space-1);
    flex-wrap: wrap;
  }
</style>
