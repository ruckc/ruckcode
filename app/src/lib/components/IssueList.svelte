<script lang="ts">
  import type { Issue } from '../host/modes';
  let { issues }: { issues: Issue[] } = $props();
</script>

{#if issues.length}
  <ul class="issues">
    {#each issues as i, n (n)}
      <li class={i.severity}>{i.message}</li>
    {/each}
  </ul>
{/if}

<style>
  .issues {
    list-style: none;
    margin: var(--tr-space-1) 0 0;
    padding: 0;
    font-size: var(--tr-text-sm);
  }
  .error {
    color: var(--tr-danger);
  }
  .warning {
    color: var(--tr-warning);
  }
  .info {
    color: var(--tr-on-surface-variant);
  }
</style>
