<script lang="ts">
  import type { UpdateController } from '../state/updates.svelte';

  let { updates }: { updates: UpdateController } = $props();

  const status = $derived.by(() => {
    switch (updates.phase) {
      case 'checking':
        return 'Checking for updates…';
      case 'downloading':
        return `Downloading v${updates.version}…`;
      case 'ready':
      case 'installing':
        return `v${updates.version} is downloaded and waits for a restart.`;
      case 'available':
        return `v${updates.version} is available.`;
      case 'manual':
        return `v${updates.version} is available. ${updates.reason}`;
      case 'uptodate':
        return `ruckcode ${updates.current} is up to date.`;
      case 'unconfigured':
        return `Updates are not configured. ${updates.reason}`;
      case 'error':
        return `The last attempt failed: ${updates.error}`;
      default:
        return updates.error ? `The last check failed: ${updates.error}` : 'Not checked yet.';
    }
  });
</script>

<section aria-label="Updates">
  <label class="row">
    <input
      type="checkbox"
      checked={updates.auto}
      onchange={(e) => updates.setAuto(e.currentTarget.checked)}
    />
    <span>Automatically download updates</span>
  </label>
  <p class="muted">
    ruckcode checks for a new version at startup and every six hours. Downloaded updates are
    installed only when you choose Restart. With this off, you are asked before anything is
    downloaded.
  </p>
  <p role="status" data-testid="update-status">{status}</p>
  {#if updates.lastChecked}
    <p class="muted">Last checked {new Date(updates.lastChecked).toLocaleString()}.</p>
  {/if}
  <button
    type="button"
    disabled={updates.phase === 'checking' ||
      updates.phase === 'downloading' ||
      updates.phase === 'installing'}
    onclick={() => void updates.check(true)}>Check for updates</button
  >
</section>

<style>
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
  }
</style>
