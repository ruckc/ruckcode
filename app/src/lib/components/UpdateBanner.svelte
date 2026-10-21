<script lang="ts">
  import type { UpdateController } from '../state/updates.svelte';
  import Markdown from './Markdown.svelte';

  let {
    updates,
    openSessions = 0,
    platform = 'linux'
  }: {
    updates: UpdateController;
    /** Sessions open in this window (they end on restart, and can be resumed). */
    openSessions?: number;
    platform?: string;
  } = $props();

  let notes = $state(false);
  let confirmForce = $state(false);

  const pct = $derived(updates.progress === null ? null : Math.round(updates.progress * 100));
  const busy = $derived(updates.busy);
</script>

{#if updates.visible}
  {#if updates.phase === 'downloading'}
    <!-- Subtle: one muted line, no buttons. -->
    <div class="line" role="status" data-testid="update-downloading">
      <span>Downloading update v{updates.version}{pct === null ? '…' : `… ${pct}%`}</span>
      <progress max="100" value={pct ?? undefined} aria-label="Update download progress"></progress>
    </div>
  {:else}
    <div class="banner" role="status" data-testid="update-banner" data-phase={updates.phase}>
      <div class="msg">
        {#if updates.phase === 'ready' || updates.phase === 'installing'}
          <strong>Update v{updates.version} ready.</strong>
          {#if busy}
            <span class="warn">
              An agent is working, so restarting now would interrupt it. Restart once it is done.
            </span>
          {:else if openSessions > 0}
            <span class="muted">
              {openSessions === 1 ? 'The open session ends' : `${openSessions} open sessions end`}
              on restart (resume from the sidebar).
            </span>
          {/if}
          {#if platform === 'windows'}
            <span class="muted">
              After restarting, a WSL project offers to update rc to match.
            </span>
          {/if}
        {:else if updates.phase === 'available'}
          <strong>Update v{updates.version} is available.</strong>
          <span class="muted">Automatic downloads are off.</span>
        {:else if updates.phase === 'manual'}
          <strong>Update v{updates.version} is available.</strong>
          <span class="muted">{updates.reason}</span>
          <!-- eslint-disable-next-line svelte/no-navigation-without-resolve -->
          <a href={updates.downloadPage} target="_blank" rel="noopener noreferrer"
            >{updates.downloadPage}</a
          >
        {:else}
          <strong class="err">Update v{updates.version} failed.</strong>
          <span class="err">{updates.error}</span>
        {/if}
      </div>
      <div class="actions">
        {#if updates.notes}
          <button
            type="button"
            class="ghost"
            aria-expanded={notes}
            onclick={() => (notes = !notes)}
          >
            Release notes
          </button>
        {/if}
        {#if updates.phase === 'ready' || updates.phase === 'installing'}
          {#if busy && !confirmForce}
            <button type="button" onclick={() => (confirmForce = true)}>Restart anyway…</button>
          {:else if busy && confirmForce}
            <button
              type="button"
              class="danger"
              onclick={() => {
                confirmForce = false;
                void updates.restartNow(true);
              }}>Interrupt the agent and restart</button
            >
            <button type="button" onclick={() => (confirmForce = false)}>Cancel</button>
          {:else}
            <button
              type="button"
              class="primary"
              disabled={updates.phase === 'installing'}
              onclick={() => void updates.restartNow()}
              >{updates.phase === 'installing' ? 'Restarting…' : 'Restart now'}</button
            >
          {/if}
          <button type="button" onclick={() => updates.later()}>Later</button>
        {:else if updates.phase === 'available'}
          <button type="button" class="primary" onclick={() => void updates.download()}
            >Download</button
          >
          <button type="button" onclick={() => updates.later()}>Later</button>
        {:else if updates.phase === 'error'}
          <button type="button" class="primary" onclick={() => void updates.download()}
            >Retry</button
          >
          <button type="button" onclick={() => updates.later()}>Dismiss</button>
        {:else}
          <button type="button" onclick={() => updates.later()}>Dismiss</button>
        {/if}
      </div>
    </div>
    {#if notes && updates.notes}
      <div class="notes" aria-label="Release notes"><Markdown text={updates.notes} /></div>
    {/if}
  {/if}
{/if}

<style>
  .line {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 2px 12px;
    font-size: 12px;
    color: var(--muted);
    border-bottom: 1px solid var(--border);
  }
  progress {
    width: 120px;
    height: 6px;
  }
  .banner {
    padding: 6px 12px;
    background: var(--code-bg);
    border-bottom: 1px solid var(--border);
    display: flex;
    gap: 12px;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
  }
  .msg {
    min-width: 0;
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: baseline;
  }
  .actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .warn {
    color: var(--warn);
  }
  .err {
    color: var(--err);
  }
  .notes {
    padding: 4px 16px 8px;
    border-bottom: 1px solid var(--border);
    background: var(--code-bg);
    max-height: 30vh;
    overflow: auto;
  }
  .danger {
    color: var(--err);
  }
</style>
