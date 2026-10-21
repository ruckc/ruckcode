<script lang="ts">
  import type { PlanEntry } from '../state/reducer';

  let { plan }: { plan: PlanEntry[] } = $props();
  const done = $derived(plan.filter((p) => p.status === 'completed').length);
  const mark = { completed: '✓', in_progress: '▶', pending: '○' } as Record<string, string>;
</script>

<aside class="plan" aria-label="Plan">
  <h2>
    Plan {#if plan.length}<span class="muted">{done}/{plan.length}</span>{/if}
  </h2>
  {#if plan.length === 0}
    <p class="muted">The agent's plan and todos appear here.</p>
  {:else}
    <ul>
      {#each plan as p, i (i)}
        <li data-status={p.status}>
          <span class="mark" aria-hidden="true">{mark[p.status] ?? '○'}</span>
          <span class="sr-only">{p.status.replace('_', ' ')}: </span>
          <span class="content">{p.content}</span>
        </li>
      {/each}
    </ul>
  {/if}
</aside>

<style>
  .plan {
    padding: 12px;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 14px;
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
    display: grid;
    gap: 6px;
  }
  li {
    display: flex;
    gap: 8px;
    color: var(--muted);
  }
  li[data-status='completed'] .content {
    text-decoration: line-through;
  }
  li[data-status='completed'] .mark {
    color: var(--ok);
  }
  li[data-status='in_progress'] {
    color: var(--text);
    font-weight: 600;
  }
  li[data-status='in_progress'] .mark {
    color: var(--warn);
  }
  .mark {
    width: 1.2em;
    flex: none;
  }
</style>
