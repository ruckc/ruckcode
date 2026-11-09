<script lang="ts">
  import { DiffView } from '@ruckc/truss-code';
  import { Tag } from '@ruckc/truss-ui';
  import type { DiffContent } from '../state/reducer';

  let {
    diff,
    patch,
    compact = false
  }: {
    /** Old and new text (live tool calls)... */
    diff?: DiffContent;
    /** ...or unified-diff text (stored transcripts). */
    patch?: { path: string; text: string };
    /** A short, scrolling preview (permission prompts). */
    compact?: boolean;
  } = $props();

  const path = $derived(diff?.path ?? patch?.path ?? '');
  const isNew = $derived(diff ? diff.oldText === null : false);
</script>

<div class="diff">
  {#if isNew}<Tag size="sm" intent="success">new file</Tag>{/if}
  {#if patch}
    <DiffView
      patch={patch.text}
      {path}
      label="Diff"
      highlight={false}
      persist="ruckcode.diff.layout"
      height={compact ? 220 : undefined}
    />
  {:else if diff}
    <DiffView
      old={diff.oldText ?? ''}
      new={diff.newText}
      {path}
      label="Diff"
      highlight={false}
      persist="ruckcode.diff.layout"
      height={compact ? 220 : undefined}
    />
  {/if}
</div>

<style>
  .diff {
    display: grid;
    gap: var(--tr-space-1);
    justify-items: start;
  }
  .diff > :global(.dv) {
    justify-self: stretch;
    min-width: 0;
  }
</style>
