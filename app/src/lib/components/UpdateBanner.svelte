<script lang="ts">
  import type { UpdateController } from '../state/updates.svelte';
  import { Button, Callout, Link, ProgressBar } from '@ruckc/truss-ui';
  import { Markdown } from '@ruckc/truss-markdown';

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
      <div class="bar">
        <ProgressBar value={pct} size="sm" label="Update download progress" />
      </div>
    </div>
  {:else}
    <Callout
      intent={updates.phase === 'error' ? 'danger' : busy ? 'warning' : 'primary'}
      role="status"
      data-testid="update-banner"
      data-phase={updates.phase}
    >
      {#if updates.phase === 'ready' || updates.phase === 'installing'}
        <strong>Update v{updates.version} ready.</strong>
        {#if busy}
          An agent is working, so restarting now would interrupt it. Restart once it is done.
        {:else if openSessions > 0}
          <span class="muted">
            {openSessions === 1 ? 'The open session ends' : `${openSessions} open sessions end`}
            on restart (resume from the sidebar).
          </span>
        {/if}
        {#if platform === 'windows'}
          <span class="muted"> After restarting, a WSL project offers to update rc to match. </span>
        {/if}
      {:else if updates.phase === 'available'}
        <strong>Update v{updates.version} is available.</strong>
        <span class="muted">Automatic downloads are off.</span>
      {:else if updates.phase === 'manual'}
        <strong>Update v{updates.version} is available.</strong>
        <span class="muted">{updates.reason}</span>
        <Link href={updates.downloadPage} external>{updates.downloadPage}</Link>
      {:else}
        <strong>Update v{updates.version} failed.</strong>
        {updates.error}
      {/if}
      {#snippet actions()}
        {#if updates.notes}
          <Button
            variant="minimal"
            intent="neutral"
            aria-expanded={notes}
            onclick={() => (notes = !notes)}
          >
            Release notes
          </Button>
        {/if}
        {#if updates.phase === 'ready' || updates.phase === 'installing'}
          {#if busy && !confirmForce}
            <Button variant="outlined" onclick={() => (confirmForce = true)}>
              Restart anyway…
            </Button>
          {:else if busy && confirmForce}
            <Button
              intent="danger"
              onclick={() => {
                confirmForce = false;
                void updates.restartNow(true);
              }}>Interrupt the agent and restart</Button
            >
            <Button variant="outlined" onclick={() => (confirmForce = false)}>Cancel</Button>
          {:else}
            <Button
              disabled={updates.phase === 'installing'}
              onclick={() => void updates.restartNow()}
              >{updates.phase === 'installing' ? 'Restarting…' : 'Restart now'}</Button
            >
          {/if}
          <Button variant="outlined" onclick={() => updates.later()}>Later</Button>
        {:else if updates.phase === 'available'}
          <Button onclick={() => void updates.download()}>Download</Button>
          <Button variant="outlined" onclick={() => updates.later()}>Later</Button>
        {:else if updates.phase === 'error'}
          <Button onclick={() => void updates.download()}>Retry</Button>
          <Button variant="outlined" onclick={() => updates.later()}>Dismiss</Button>
        {:else}
          <Button variant="outlined" onclick={() => updates.later()}>Dismiss</Button>
        {/if}
      {/snippet}
    </Callout>
    {#if notes && updates.notes}
      <div class="notes" aria-label="Release notes"><Markdown text={updates.notes} /></div>
    {/if}
  {/if}
{/if}

<style>
  .line {
    display: flex;
    gap: var(--tr-space-3);
    align-items: center;
    padding: 2px var(--tr-space-3);
    font-size: var(--tr-text-sm);
    color: var(--tr-on-surface-variant);
    border-bottom: 1px solid var(--tr-outline-variant);
  }
  .bar {
    width: 120px;
  }
  .bar :global(.meta) {
    display: none;
  }
  .notes {
    padding: var(--tr-space-1) var(--tr-space-4) var(--tr-space-2);
    border-bottom: 1px solid var(--tr-outline-variant);
    background: var(--tr-surface-container);
    max-height: 30vh;
    overflow: auto;
  }
</style>
