<script lang="ts">
  import { Button } from '@ruckc/truss-ui';
  import type { UnfinishedSession } from '../host/transcripts';
  import Modal from './Modal.svelte';

  let {
    sessions,
    onresume,
    ondismiss,
    onclose
  }: {
    sessions: UnfinishedSession[];
    onresume: (id: string, compact: boolean) => void;
    ondismiss: (id: string) => void;
    onclose: () => void;
  } = $props();

  const why = (s: UnfinishedSession): string => {
    const parts: string[] = [];
    if (s.interrupted) parts.push('interrupted mid-work');
    if (s.openTasks.length)
      parts.push(`${s.openTasks.length} task${s.openTasks.length === 1 ? '' : 's'} left`);
    return parts.join(', ');
  };
</script>

<Modal title="Pick up where you left off?" size="lg" {onclose}>
  <p class="muted">
    These sessions in this project stopped before they were done. Resuming continues the session;
    compacting first keeps only what was in progress and what remains.
  </p>
  <ul class="list">
    {#each sessions as s, i (s.id)}
      <li data-session={s.id}>
        <div class="title">{s.firstPrompt || s.id}</div>
        <div class="muted meta">
          {why(s)} · {s.mode} · {s.updatedAt.slice(0, 16).replace('T', ' ')}
        </div>
        {#if s.inProgress}<div class="muted meta">In progress: {s.inProgress}</div>{/if}
        {#each s.openTasks.filter((t) => t !== s.inProgress).slice(0, 3) as t (t)}
          <div class="muted meta">To do: {t}</div>
        {/each}
        <div class="row">
          <Button autofocus={i === 0} onclick={() => onresume(s.id, false)}>Resume</Button>
          <Button variant="outlined" onclick={() => onresume(s.id, true)}>
            Compact and resume
          </Button>
          <Button variant="minimal" intent="neutral" onclick={() => ondismiss(s.id)}>
            Dismiss
          </Button>
        </div>
      </li>
    {/each}
  </ul>

  {#snippet footer()}
    <Button variant="minimal" intent="neutral" onclick={onclose}>Start fresh</Button>
  {/snippet}
</Modal>

<style>
  .list {
    list-style: none;
    margin: var(--tr-space-3) 0 0;
    padding: 0;
    display: grid;
    gap: var(--tr-space-3);
  }
  .list li {
    border: 1px solid var(--tr-outline-variant);
    border-radius: var(--tr-radius-md);
    padding: var(--tr-space-3);
    background: var(--tr-surface);
  }
  .title {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .meta {
    font-size: var(--tr-text-sm);
  }
  .row {
    display: flex;
    gap: var(--tr-space-2);
    flex-wrap: wrap;
    margin-top: var(--tr-space-2);
  }
</style>
