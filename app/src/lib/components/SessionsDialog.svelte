<script lang="ts">
  import type { Snippet } from 'svelte';
  import Modal from './Modal.svelte';

  let { onclose, children }: { onclose: () => void; children: Snippet } = $props();
</script>

<Modal title="Open" {onclose}>
  <div class="body">{@render children()}</div>
</Modal>

<style>
  .body {
    margin: calc(-1 * var(--tr-space-4));
  }
</style>
