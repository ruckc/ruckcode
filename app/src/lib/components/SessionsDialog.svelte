<script lang="ts">
  import type { Snippet } from 'svelte';
  import { focusTrap } from '../a11y';
  import Icon from './Icon.svelte';

  let { onclose, children }: { onclose: () => void; children: Snippet } = $props();

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && !e.defaultPrevented) {
      e.preventDefault();
      onclose();
    }
  }
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="presentation"
  onclick={(e) => e.target === e.currentTarget && onclose()}
>
  <div
    class="dialog sessions"
    role="dialog"
    aria-modal="true"
    aria-labelledby="sessions-title"
    use:focusTrap
  >
    <header>
      <h2 id="sessions-title">Open</h2>
      <button type="button" class="icon ghost" aria-label="Close" onclick={onclose}
        ><Icon name="close" /></button
      >
    </header>
    <div class="body">{@render children()}</div>
  </div>
</div>

<style>
  .sessions {
    width: min(460px, 94vw);
    max-height: 86vh;
    display: flex;
    flex-direction: column;
    padding: 0;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 12px 0 16px;
  }
  h2 {
    margin: 0;
    font-size: 15px;
  }
  .body {
    overflow-y: auto;
    min-height: 0;
  }
</style>
