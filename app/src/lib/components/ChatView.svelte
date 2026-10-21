<script lang="ts">
  import { tick } from 'svelte';
  import type { ChatItem } from '../state/reducer';
  import Markdown from './Markdown.svelte';
  import ToolCard from './ToolCard.svelte';

  let { items, running, loading }: { items: ChatItem[]; running: boolean; loading: boolean } =
    $props();

  let scroller: HTMLElement | undefined = $state();
  let stick = true;

  function onscroll() {
    if (!scroller) return;
    stick = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
  }

  // Follow the stream, unless the reader scrolled up.
  $effect(() => {
    void items.length;
    const last = items.at(-1);
    if (last && 'text' in last) void last.text;
    if (last && last.kind === 'tool') void last.output;
    if (stick)
      tick().then(() => {
        if (scroller) scroller.scrollTop = scroller.scrollHeight;
      });
  });

  const reasons: Record<string, string> = {
    end_turn: 'Turn complete',
    cancelled: 'Cancelled',
    max_tokens: 'Stopped: token limit',
    max_turn_requests: 'Stopped: request limit',
    refusal: 'Refused',
    error: 'Turn failed'
  };
</script>

<div
  class="chat"
  bind:this={scroller}
  {onscroll}
  role="log"
  aria-live="polite"
  aria-relevant="additions"
  aria-label="Conversation"
  aria-busy={running || loading}
>
  {#if loading}<p class="muted center">Loading history…</p>{/if}
  {#each items as item, i (i)}
    {#if item.kind === 'user'}
      <div class="msg user" data-testid="user-msg">{item.text}</div>
    {:else if item.kind === 'assistant'}
      <div class="msg assistant" data-testid="assistant-msg"><Markdown text={item.text} /></div>
    {:else if item.kind === 'thought'}
      <details class="thought">
        <summary>Reasoning</summary>
        <div class="thought-body">{item.text}</div>
      </details>
    {:else if item.kind === 'tool'}
      <ToolCard tool={item} />
    {:else if item.kind === 'notice'}
      <div
        class="notice"
        data-severity={item.severity}
        role={item.severity === 'error' ? 'alert' : 'status'}
      >
        <span class="sev">{item.severity}</span>
        <span><strong>{item.title}</strong>{item.description ? ` ${item.description}` : ''}</span>
      </div>
    {:else}
      <div class="turn-end" data-reason={item.stopReason} data-testid="turn-end">
        <span>{reasons[item.stopReason] ?? item.stopReason}</span>
        <code>{item.stopReason}</code>
      </div>
    {/if}
  {/each}
  {#if running}
    <div class="working muted" aria-hidden="true"><span class="pulse"></span> working…</div>
  {/if}
</div>

<style>
  .chat {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
  }
  .chat > :global(*) {
    max-width: min(860px, 100%);
  }
  .center {
    align-self: center;
  }
  .msg {
    padding: 6px 12px;
    border-radius: 10px;
  }
  .msg.user {
    background: var(--accent);
    color: var(--accent-text);
    align-self: flex-end;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .msg.assistant {
    background: var(--surface);
    border: 1px solid var(--border);
    width: 100%;
  }
  .thought {
    color: var(--muted);
    width: 100%;
  }
  .thought summary {
    cursor: pointer;
    font-size: 12px;
  }
  .thought-body {
    font-style: italic;
    white-space: pre-wrap;
    padding: 2px 0 2px 12px;
    border-left: 2px solid var(--border);
  }
  .notice {
    display: flex;
    gap: 8px;
    align-items: baseline;
    width: 100%;
    padding: 4px 10px;
    border: 1px dashed var(--border);
    border-radius: 6px;
    background: var(--code-bg);
    font-size: 12.5px;
  }
  .sev {
    text-transform: uppercase;
    font-size: 10.5px;
    letter-spacing: 0.06em;
    color: var(--muted);
  }
  .notice[data-severity='warning'] {
    border-color: var(--warn);
  }
  .notice[data-severity='warning'] .sev {
    color: var(--warn);
  }
  .notice[data-severity='error'] {
    border-color: var(--err);
  }
  .notice[data-severity='error'] .sev {
    color: var(--err);
  }
  .turn-end {
    display: flex;
    gap: 8px;
    align-items: center;
    width: 100%;
    color: var(--muted);
    font-size: 11.5px;
  }
  .turn-end::before,
  .turn-end::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--border);
  }
  .turn-end[data-reason='cancelled'],
  .turn-end[data-reason='max_tokens'],
  .turn-end[data-reason='max_turn_requests'] {
    color: var(--warn);
  }
  .turn-end[data-reason='refusal'],
  .turn-end[data-reason='error'] {
    color: var(--err);
  }
  .pulse {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.25;
    }
  }
</style>
