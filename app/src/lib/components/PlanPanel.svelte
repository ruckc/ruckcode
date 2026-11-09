<script lang="ts">
  import { Badge, Icon } from '@ruckc/truss-ui';
  import type { IconName } from '@ruckc/truss-ui';
  import type { PlanEntry } from '../state/reducer';

  let { plan, complete = null }: { plan: PlanEntry[]; complete?: number | null } = $props();

  const done = $derived(plan.filter((p) => p.status === 'completed').length);
  const marks: Record<string, IconName> = {
    completed: 'tick',
    in_progress: 'play',
    pending: 'minus'
  };
</script>

<aside class="plan" aria-label="Plan">
  <h2>
    Plan {#if plan.length}<span class="muted">{done}/{plan.length}</span>{/if}
  </h2>
  {#if plan.length === 0 && complete != null}
    <p class="done" role="status">
      <Badge intent="success" icon="tick">Plan complete ({complete}/{complete})</Badge>
    </p>
  {:else if plan.length === 0}
    <p class="muted">The agent's plan and todos appear here.</p>
  {:else}
    <ul>
      {#each plan as p, i (i)}
        <li data-status={p.status}>
          <span class="mark"><Icon name={marks[p.status] ?? 'minus'} /></span>
          <span class="tr-visually-hidden">{p.status.replace('_', ' ')}: </span>
          <span class="content">{p.content}</span>
        </li>
      {/each}
    </ul>
  {/if}
</aside>

<style>
  .plan {
    padding: var(--tr-space-3);
  }
  h2 {
    margin: 0 0 var(--tr-space-2);
    font-size: var(--tr-text-md);
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
    display: grid;
    gap: var(--tr-space-2);
  }
  li {
    display: flex;
    gap: var(--tr-space-2);
    align-items: baseline;
    color: var(--tr-on-surface-variant);
  }
  li[data-status='completed'] .content {
    text-decoration: line-through;
  }
  li[data-status='completed'] .mark {
    color: var(--tr-success);
  }
  li[data-status='in_progress'] {
    color: var(--tr-on-surface);
    font-weight: 600;
  }
  li[data-status='in_progress'] .mark {
    color: var(--tr-warning);
  }
  .done {
    margin: 0;
  }
  .mark {
    flex: none;
    display: inline-flex;
  }
</style>
