<script lang="ts">
  import Icon from './Icon.svelte';

  export interface MenuItem {
    label: string;
    onclick: () => void;
    disabled?: boolean;
    danger?: boolean;
  }

  let { label, items }: { label: string; items: (MenuItem | 'sep')[] } = $props();
  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();

  function pick(item: MenuItem) {
    open = false;
    item.onclick();
  }

  function onwindow(e: MouseEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }

  function onkeydown(e: KeyboardEvent) {
    if (open && e.key === 'Escape') {
      e.preventDefault();
      open = false;
    }
  }
</script>

<svelte:window onclick={onwindow} {onkeydown} />

<div class="menu" bind:this={root}>
  <button
    type="button"
    class="icon ghost"
    aria-label={label}
    title={label}
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={() => (open = !open)}><Icon name="more" /></button
  >
  {#if open}
    <div class="pop" role="menu" aria-label={label}>
      {#each items as item, i (i)}
        {#if item === 'sep'}
          <hr />
        {:else}
          <button
            type="button"
            role="menuitem"
            class:danger={item.danger}
            disabled={item.disabled}
            onclick={() => pick(item)}>{item.label}</button
          >
        {/if}
      {/each}
    </div>
  {/if}
</div>

<style>
  .menu {
    position: relative;
  }
  .pop {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    z-index: 20;
    min-width: 180px;
    display: grid;
    padding: 4px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 0.18);
  }
  .pop button {
    border: 0;
    background: none;
    text-align: left;
    padding: 0 10px;
  }
  .pop button:hover:not(:disabled) {
    background: var(--code-bg);
  }
  hr {
    border: 0;
    border-top: 1px solid var(--border);
    margin: 4px 0;
  }
</style>
