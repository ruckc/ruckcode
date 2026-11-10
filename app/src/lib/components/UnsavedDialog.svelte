<script lang="ts">
  import { Button } from '@ruckc/truss-ui';
  import Modal from './Modal.svelte';

  let { onkeep, ondiscard }: { onkeep: () => void; ondiscard: () => void } = $props();
</script>

<Modal
  alert
  size="sm"
  backdropClose={false}
  title="Unsaved mode changes"
  onclose={onkeep}
  aria-describedby="unsaved-text"
>
  <p id="unsaved-text">
    You have edited the modes but not saved them. Leaving now discards those changes.
  </p>

  {#snippet footer()}
    <Button autofocus onclick={onkeep}>Keep editing</Button>
    <Button intent="danger" variant="outlined" onclick={ondiscard}>Discard changes</Button>
  {/snippet}
</Modal>
