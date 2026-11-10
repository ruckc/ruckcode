<script lang="ts">
  import { onDestroy } from 'svelte';
  import { ChatMessage, ChatStatus, ChatThread } from '@ruckc/truss-chat';
  import { Button, Callout } from '@ruckc/truss-ui';
  import type { ChatItem } from '../state/reducer';
  import { rateWaitText, type RateWait } from '../state/ratelimit';
  import ToolCard from './ToolCard.svelte';

  let {
    items,
    running,
    loading,
    rateWait = null,
    oncancel
  }: {
    items: ChatItem[];
    running: boolean;
    loading: boolean;
    rateWait?: RateWait | null;
    oncancel?: () => void;
  } = $props();

  // Live countdown: re-render the banner text a few times a second.
  let now = $state(Date.now());
  const timer = setInterval(() => (now = Date.now()), 250);
  onDestroy(() => clearInterval(timer));

  const reasons: Record<string, string> = {
    end_turn: 'Turn complete',
    cancelled: 'Cancelled',
    max_tokens: 'Stopped: token limit',
    max_turn_requests: 'Stopped: request limit',
    refusal: 'Refused',
    rate_limited: 'Stopped: rate limited (resume after the reset)',
    error: 'Turn failed'
  };

  /** truss-ui intents for the agent's notice severities. */
  const intents = { error: 'danger', warning: 'warning' } as const;
  const intentOf = (severity: string) => intents[severity as keyof typeof intents] ?? 'primary';
</script>

<ChatThread label="Conversation" aria-busy={running || loading} class="thread">
  {#if loading}<p class="muted center">Loading history…</p>{/if}
  {#each items as item, i (i)}
    {#if item.kind === 'user'}
      <ChatMessage role="user" text={item.text} data-testid="user-msg" />
    {:else if item.kind === 'assistant'}
      <ChatMessage
        role="assistant"
        text={item.text}
        streaming={running && i === items.length - 1}
        data-testid="assistant-msg"
      />
    {:else if item.kind === 'thought'}
      <ChatMessage
        role="assistant"
        label="Reasoning"
        reasoning={item.text}
        streaming={running && i === items.length - 1}
        data-testid="thought"
      />
    {:else if item.kind === 'tool'}
      <ToolCard tool={item} />
    {:else if item.kind === 'notice'}
      <Callout
        intent={intentOf(item.severity)}
        title={item.title}
        role={item.severity === 'error' ? 'alert' : 'status'}
        data-severity={item.severity}
      >
        {item.description}
      </Callout>
    {:else}
      <div class="turn-end" data-reason={item.stopReason} data-testid="turn-end">
        <span>{reasons[item.stopReason] ?? item.stopReason}</span>
        <code>{item.stopReason}</code>
      </div>
    {/if}
  {/each}
  {#if rateWait}
    <ChatStatus variant="progress" label={rateWaitText(rateWait, now)} data-testid="rate-wait">
      {#if oncancel}
        <Button size="sm" variant="outlined" aria-label="Cancel the wait" onclick={oncancel}>
          Cancel
        </Button>
      {/if}
    </ChatStatus>
  {:else if running}
    <ChatStatus variant="progress" label="Working…" aria-hidden="true" />
  {/if}
</ChatThread>

<style>
  :global(.thread) {
    flex: 1;
    min-height: 0;
  }
  .center {
    text-align: center;
  }
  .turn-end {
    display: flex;
    gap: var(--tr-space-2);
    align-items: center;
    color: var(--tr-on-surface-variant);
    font-size: var(--tr-text-xs);
  }
  .turn-end::before,
  .turn-end::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--tr-outline-variant);
  }
  .turn-end[data-reason='cancelled'],
  .turn-end[data-reason='max_tokens'],
  .turn-end[data-reason='max_turn_requests'] {
    color: var(--tr-warning);
  }
  .turn-end[data-reason='refusal'],
  .turn-end[data-reason='error'] {
    color: var(--tr-danger);
  }
</style>
