<script lang="ts">
  import { focusTrap } from '../a11y';
  import { summarizeInput } from '../format';
  import type { DiffContent } from '../state/reducer';
  import type { PendingPermission } from '../state/chat.svelte';
  import DiffView from './DiffView.svelte';
  import KindIcon from './KindIcon.svelte';

  let { pending, total = 1 }: { pending: PendingPermission; total?: number } = $props();

  const call = $derived(pending.request.toolCall);
  const options = $derived(pending.request.options);
  const isSwitch = $derived(call.kind === 'switch_mode');
  const raw = $derived((call.rawInput ?? null) as Record<string, unknown> | null);
  const target = $derived(
    isSwitch && raw && typeof raw.mode_id === 'string' ? (raw.mode_id as string) : null
  );
  const reason = $derived(raw && typeof raw.reason === 'string' ? (raw.reason as string) : null);
  const summary = $derived(isSwitch ? [] : summarizeInput(call.rawInput));
  const diffs = $derived(
    (call.content ?? [])
      .filter((c) => c.type === 'diff')
      .map((c): DiffContent => ({
        path: c.path,
        oldText: c.oldText ?? null,
        newText: c.newText
      }))
  );

  const KEYS: Record<string, string> = {
    allow_once: 'y',
    allow_always: 'a',
    reject_once: 'n',
    reject_always: 'x'
  };
  const keyFor = (kind: string) => KEYS[kind] ?? '';

  function choose(optionId: string) {
    pending.resolve({ outcome: { outcome: 'selected', optionId } });
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      pending.resolve({ outcome: { outcome: 'cancelled' } });
      return;
    }
    const o = options.find((x) => keyFor(x.kind) === e.key.toLowerCase());
    if (o) {
      e.preventDefault();
      choose(o.optionId);
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="backdrop" role="presentation">
  <div
    class="dialog wide"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="perm-title"
    aria-describedby="perm-desc"
    use:focusTrap
  >
    <h2 id="perm-title">
      {#if isSwitch}Mode switch proposed{:else}Permission requested{/if}
      {#if total > 1}<span class="muted count">1 of {total} pending</span>{/if}
    </h2>
    <p id="perm-desc" class="what">
      {#if isSwitch}
        The agent wants to switch to mode <strong>{target ?? '(unknown)'}</strong>. Allowing changes
        which tools and rules apply to the rest of this session.
      {:else}
        <KindIcon kind={call.kind ?? 'other'} />
        <span>The agent wants to run <strong>{call.title ?? 'a tool call'}</strong></span>
        <span class="muted kind">{(call.kind ?? 'other').replace('_', ' ')}</span>
      {/if}
    </p>
    {#if isSwitch && reason}<p class="reason">Reason: {reason}</p>{/if}
    {#if summary.length}
      <ul class="input" aria-label="Tool input">
        {#each summary as line, i (i)}<li><code>{line}</code></li>{/each}
      </ul>
    {/if}
    {#each diffs as d, i (i)}<DiffView diff={d} compact />{/each}
    <div class="actions">
      {#each options as o, i (o.optionId)}
        {@const key = keyFor(o.kind)}
        <button
          class:danger={o.kind.startsWith('reject')}
          class:primary={o.kind === 'allow_once'}
          data-autofocus={i === 0 ? '' : undefined}
          data-kind={o.kind}
          onclick={() => choose(o.optionId)}
        >
          {isSwitch && o.kind === 'allow_once'
            ? `Switch to ${target ?? 'mode'}`
            : isSwitch && o.kind.startsWith('reject')
              ? 'Stay in current mode'
              : o.name}
          {#if key}<kbd>{key}</kbd>{/if}
        </button>
      {/each}
      <button class="ghost" onclick={() => pending.resolve({ outcome: { outcome: 'cancelled' } })}>
        Cancel turn <kbd>Esc</kbd>
      </button>
    </div>
  </div>
</div>

<style>
  .what {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
    margin: 0 0 8px;
  }
  .count {
    font-size: 12px;
    font-weight: 400;
    margin-left: 8px;
  }
  .reason {
    margin: 0 0 8px;
    color: var(--muted);
  }
  .input {
    margin: 0 0 8px;
    padding: 6px 10px;
    list-style: none;
    background: var(--code-bg);
    border-radius: 6px;
  }
  .input li {
    overflow-wrap: anywhere;
  }
  kbd {
    font: inherit;
    font-size: 11px;
    opacity: 0.8;
    border: 1px solid currentColor;
    border-radius: 3px;
    padding: 0 4px;
    margin-left: 6px;
  }
</style>
