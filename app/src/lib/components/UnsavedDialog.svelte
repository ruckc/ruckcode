<script lang="ts">
  import { focusTrap } from '../a11y';

  let { onkeep, ondiscard }: { onkeep: () => void; ondiscard: () => void } = $props();
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape' && !e.defaultPrevented) {
      e.preventDefault();
      onkeep();
    }
  }}
/>

<div class="backdrop" role="presentation">
  <div
    class="dialog"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="unsaved-title"
    aria-describedby="unsaved-text"
    use:focusTrap
  >
    <h2 id="unsaved-title">Unsaved mode changes</h2>
    <p id="unsaved-text">
      You have edited the modes but not saved them. Leaving now discards those changes.
    </p>
    <div class="actions">
      <button type="button" class="primary" data-autofocus onclick={onkeep}>Keep editing</button>
      <button type="button" class="danger" onclick={ondiscard}>Discard changes</button>
    </div>
  </div>
</div>
