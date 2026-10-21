<script lang="ts">
  /** A list of strings edited as chips, with a text box (Enter adds), suggestions and live checks. */
  let {
    values = $bindable(),
    label,
    suggestions = [],
    placeholder = 'Add...',
    problem,
    preview
  }: {
    values: string[];
    label: string;
    /** Values offered as one-click chips (those already present are hidden). */
    suggestions?: string[];
    placeholder?: string;
    /** A message when the value is not acceptable (shown on the chip, not blocking). */
    problem?: (v: string) => string | null;
    /** Extra text under a chip (e.g. what a glob matches now). */
    preview?: (v: string) => string | null;
  } = $props();

  let draft = $state('');
  const inputId = `chip-${Math.random().toString(36).slice(2, 8)}`;

  const draftProblem = $derived(draft.trim() && problem ? problem(draft.trim()) : null);
  const offered = $derived(
    suggestions.filter((s) => !values.includes(s)).filter((s) => !draft || s.includes(draft.trim()))
  );

  function add(v = draft) {
    const t = v.trim();
    if (!t) return;
    if (!values.includes(t)) values = [...values, t];
    draft = '';
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      add();
    } else if (e.key === 'Backspace' && !draft && values.length) {
      values = values.slice(0, -1);
    }
  }
</script>

<div class="chips" role="group" aria-label={label}>
  {#if values.length}
    <ul class="list">
      {#each values as v (v)}
        {@const bad = problem?.(v)}
        {@const note = preview?.(v)}
        <li>
          <span class="chip" class:bad={!!bad}>
            <code>{v}</code>
            <button
              type="button"
              class="x"
              aria-label={`Remove ${v}`}
              onclick={() => (values = values.filter((x) => x !== v))}>&times;</button
            >
          </span>
          {#if bad}<span class="err small">{bad}</span>
          {:else if note}<span class="muted small">{note}</span>{/if}
        </li>
      {/each}
    </ul>
  {/if}
  <div class="row">
    <input
      id={inputId}
      type="text"
      aria-label={`Add to ${label}`}
      {placeholder}
      bind:value={draft}
      {onkeydown}
      spellcheck="false"
    />
    <button type="button" disabled={!draft.trim()} onclick={() => add()}>Add</button>
  </div>
  {#if draftProblem}<p class="err small" role="status">{draftProblem}</p>{/if}
  {#if offered.length}
    <div class="sugg" aria-label={`Suggestions for ${label}`} role="group">
      {#each offered.slice(0, 14) as s (s)}
        <button type="button" class="s" onclick={() => add(s)} title={`Add ${s}`}>+ {s}</button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .chips {
    display: grid;
    gap: 4px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
  }
  .list li {
    display: flex;
    align-items: baseline;
    gap: 6px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 0 2px 0 8px;
    background: var(--code-bg);
  }
  .chip.bad {
    border-color: var(--err);
  }
  .x {
    border: none;
    background: none;
    padding: 0 6px;
    line-height: 1.3;
  }
  .row {
    display: flex;
    gap: 6px;
  }
  .row input {
    flex: 1;
    min-width: 0;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12.5px;
  }
  .sugg {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .s {
    font-size: 12px;
    padding: 0 8px;
    border-radius: 12px;
    color: var(--muted);
  }
  .small {
    font-size: 12px;
  }
  .err {
    color: var(--err);
    margin: 0;
  }
</style>
