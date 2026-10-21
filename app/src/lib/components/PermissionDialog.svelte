<script lang="ts">
  import type { PendingPermission } from '../state/chat.svelte';

  let { pending }: { pending: PendingPermission } = $props();

  const toolTitle = $derived(pending.request.toolCall.title ?? 'tool call');
  const options = $derived(pending.request.options);
</script>

<div class="backdrop" role="presentation">
  <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="perm-title">
    <h2 id="perm-title">Permission requested</h2>
    <p><code>{toolTitle}</code></p>
    <div class="actions">
      {#each options as o (o.optionId)}
        <button
          class:danger={o.kind.startsWith('reject')}
          class:primary={o.kind === 'allow_once'}
          onclick={() =>
            pending.resolve({ outcome: { outcome: 'selected', optionId: o.optionId } })}
        >
          {o.name}
        </button>
      {/each}
      <button onclick={() => pending.resolve({ outcome: { outcome: 'cancelled' } })}>Cancel</button>
    </div>
  </div>
</div>
