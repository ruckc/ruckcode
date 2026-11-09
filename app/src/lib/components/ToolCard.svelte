<script lang="ts">
  import { ToolCall } from '@ruckc/truss-chat';
  import { Button } from '@ruckc/truss-ui';
  import { truncateLines } from '../format';
  import type { ToolItem } from '../state/reducer';
  import DiffPanel from './DiffPanel.svelte';

  let { tool }: { tool: ToolItem } = $props();
  const LIMIT = 12;

  // Failures, running calls and anything with a diff start open; quiet successes start closed.
  let userOpen = $state<boolean | null>(null);
  const open = $derived(userOpen ?? (tool.diffs.length > 0 || tool.status !== 'completed'));
  let showAll = $state(false);
  const out = $derived(truncateLines(tool.output, LIMIT));
  const hasBody = $derived(
    tool.output !== '' ||
      tool.diffs.length > 0 ||
      tool.locations.length > 0 ||
      tool.rawInput != null
  );
</script>

{#snippet details()}
  <p class="kind muted">Kind: {tool.toolKind.replace('_', ' ')}</p>
  {#if tool.locations.length}
    <ul class="locs" aria-label="Locations">
      {#each tool.locations as l, i (i)}
        <li><code>{l.path}{l.line ? `:${l.line}` : ''}</code></li>
      {/each}
    </ul>
  {/if}
  {#each tool.diffs as d, i (i)}
    <DiffPanel diff={d} />
  {/each}
  {#if out.hidden > 0}
    <Button variant="minimal" size="sm" onclick={() => (showAll = !showAll)}>
      {showAll ? 'Show less' : `Show all (${out.hidden} more lines)`}
    </Button>
  {/if}
{/snippet}

<!-- TODO(truss-ui#137): per-kind icon/meta slot, output line limit with "show all", and an Input toggle on ToolCall. -->
<ToolCall
  name={tool.title}
  status={tool.status}
  input={tool.rawInput ?? undefined}
  output={tool.output ? (showAll ? tool.output : out.shown) : undefined}
  details={hasBody ? details : undefined}
  bind:open={() => open, (v) => (userOpen = v)}
  data-testid="tool-card"
  data-kind={tool.toolKind}
/>

<style>
  .kind {
    margin: 0 0 var(--tr-space-2);
    text-transform: capitalize;
  }
  .locs {
    margin: 0 0 var(--tr-space-2);
    padding: 0;
    list-style: none;
    display: flex;
    gap: var(--tr-space-3);
    flex-wrap: wrap;
    color: var(--tr-on-surface-variant);
  }
</style>
