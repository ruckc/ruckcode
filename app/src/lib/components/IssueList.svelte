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
    margin: 2px 0 0;
    padding: 0;
    font-size: 12px;
  }
  .error {
    color: var(--err);
  }
  .warning {
    color: var(--warn);
  }
  .info {
    color: var(--muted);
  }
</style>
