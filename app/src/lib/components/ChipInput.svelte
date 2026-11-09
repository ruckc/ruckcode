<script lang="ts">
  import { Button, Input, Tag } from '@ruckc/truss-ui';

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

  // TODO(truss-ui#143): TagInput has no suggestions, per-tag notes/preview or non-blocking validation, so the
  // chip list is composed from Tag + Input + Button here.
  let draft = $state('');

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
          <Tag
            size="sm"
            intent={bad ? 'danger' : 'neutral'}
            removeLabel={`Remove ${v}`}
            onremove={() => (values = values.filter((x) => x !== v))}><code>{v}</code></Tag
          >
          {#if bad}<span class="err small">{bad}</span>
          {:else if note}<span class="muted small">{note}</span>{/if}
        </li>
      {/each}
    </ul>
  {/if}
  <div class="row">
    <div class="grow">
      <Input
        size="sm"
        aria-label={`Add to ${label}`}
        {placeholder}
        value={draft}
        oninput={(e) => (draft = e.currentTarget.value)}
        {onkeydown}
        spellcheck="false"
        style="font-family: var(--tr-font-mono)"
      />
    </div>
    <Button size="sm" variant="outlined" disabled={!draft.trim()} onclick={() => add()}>Add</Button>
  </div>
  {#if draftProblem}<p class="err small" role="status">{draftProblem}</p>{/if}
  {#if offered.length}
    <div class="sugg" aria-label={`Suggestions for ${label}`} role="group">
      {#each offered.slice(0, 14) as s (s)}
        <Button size="sm" variant="tonal" intent="neutral" onclick={() => add(s)} title={`Add ${s}`}
          >+ {s}</Button
        >
      {/each}
    </div>
  {/if}
</div>

<style>
  .chips {
    display: grid;
    gap: var(--tr-space-1);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: var(--tr-space-1) var(--tr-space-3);
  }
  .list li {
    display: flex;
    align-items: center;
    gap: var(--tr-space-2);
  }
  .row {
    display: flex;
    gap: var(--tr-space-2);
    align-items: center;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .sugg {
    display: flex;
    flex-wrap: wrap;
    gap: var(--tr-space-1);
  }
  .small {
    font-size: var(--tr-text-sm);
    margin: 0;
  }
  .err {
    color: var(--tr-danger);
    margin: 0;
  }
</style>
