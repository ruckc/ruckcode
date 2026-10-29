<script lang="ts">
  import { focusTrap } from '../a11y';
  import type { UnfinishedSession } from '../host/transcripts';

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

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && !e.defaultPrevented) {
      e.preventDefault();
      onclose();
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="backdrop" role="presentation">
  <div
    class="dialog wide"
    role="dialog"
    aria-modal="true"
    aria-labelledby="resume-title"
    use:focusTrap
  >
    <h2 id="resume-title">Pick up where you left off?</h2>
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
            <button
              class="primary"
              data-autofocus={i === 0 ? '' : undefined}
              onclick={() => onresume(s.id, false)}
            >
              Resume
            </button>
            <button onclick={() => onresume(s.id, true)}>Compact and resume</button>
            <button class="ghost" onclick={() => ondismiss(s.id)}>Dismiss</button>
          </div>
        </li>
      {/each}
    </ul>
    <div class="actions">
      <button class="ghost" onclick={onclose}>Start fresh</button>
    </div>
  </div>
</div>

<style>
  .list {
    list-style: none;
    margin: 8px 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-height: 50vh;
    overflow-y: auto;
  }
  li {
    border: 1px solid var(--border, currentColor);
    border-radius: 8px;
    padding: 8px 10px;
  }
  .title {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .meta {
    font-size: 12px;
    overflow-wrap: anywhere;
  }
  .row {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-top: 8px;
  }
</style>
