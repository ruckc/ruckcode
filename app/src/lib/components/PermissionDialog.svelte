<script lang="ts">
  import { Badge, Button, Kbd } from '@ruckc/truss-ui';
  import { summarizeInput } from '../format';
  import type { DiffContent } from '../state/reducer';
  import type { PendingPermission } from '../state/chat.svelte';
  import DiffPanel from './DiffPanel.svelte';
  import KindIcon from './KindIcon.svelte';
  import Modal from './Modal.svelte';

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

  const cancel = () => pending.resolve({ outcome: { outcome: 'cancelled' } });

  function choose(optionId: string) {
    pending.resolve({ outcome: { outcome: 'selected', optionId } });
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      cancel();
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

<Modal
  alert
  size="lg"
  backdropClose={false}
  title={isSwitch ? 'Mode switch proposed' : 'Permission requested'}
  onclose={cancel}
  aria-describedby="perm-desc"
>
  {#if total > 1}<Badge intent="neutral">1 of {total} pending</Badge>{/if}
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
  {#each diffs as d, i (i)}<DiffPanel diff={d} compact />{/each}

  {#snippet footer()}
    {#each options as o, i (o.optionId)}
      {@const key = keyFor(o.kind)}
      <Button
        intent={o.kind.startsWith('reject') ? 'danger' : 'primary'}
        variant={o.kind === 'allow_once' ? 'filled' : 'outlined'}
        autofocus={i === 0}
        data-kind={o.kind}
        onclick={() => choose(o.optionId)}
      >
        {isSwitch && o.kind === 'allow_once'
          ? `Switch to ${target ?? 'mode'}`
          : isSwitch && o.kind.startsWith('reject')
            ? 'Stay in current mode'
            : o.name}
        {#if key}<Kbd keys={key} />{/if}
      </Button>
    {/each}
    <Button variant="minimal" intent="neutral" onclick={cancel}>
      Cancel turn <Kbd keys="Esc" />
    </Button>
  {/snippet}
</Modal>

<style>
  .what {
    display: flex;
    gap: var(--tr-space-2);
    align-items: center;
    flex-wrap: wrap;
  }
  .kind {
    font-size: var(--tr-text-xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .reason {
    margin: 0 0 var(--tr-space-2);
  }
  .input {
    margin: 0 0 var(--tr-space-3);
    padding: var(--tr-space-2) var(--tr-space-3);
    list-style: none;
    background: var(--tr-surface-container);
    border-radius: var(--tr-radius-md);
    overflow-wrap: anywhere;
  }
</style>
