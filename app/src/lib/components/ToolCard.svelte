<script lang="ts">
  import { truncateLines } from '../format';
  import type { ToolItem } from '../state/reducer';
  import DiffView from './DiffView.svelte';
  import KindIcon from './KindIcon.svelte';

  let { tool, layout = 'unified' }: { tool: ToolItem; layout?: 'unified' | 'split' } = $props();

  const LIMIT = 12;
  const statusLabel = $derived(tool.status === 'in_progress' ? 'running' : tool.status);
  // Failures, running calls and anything with a diff start open; quiet successes start closed.
  let userOpen = $state<boolean | null>(null);
  const open = $derived(
    userOpen ?? (tool.diffs.length > 0 || tool.status !== 'completed' || false)
  );
  let showInput = $state(false);
  let showAll = $state(false);
  let split = $state(false);
  const effectiveLayout = $derived(split ? 'split' : layout);
  const out = $derived(truncateLines(tool.output, LIMIT));
  const hasBody = $derived(
    tool.output !== '' ||
      tool.diffs.length > 0 ||
      tool.locations.length > 0 ||
      tool.rawInput != null
  );
</script>

<div class="tool" data-status={tool.status} data-kind={tool.toolKind} data-testid="tool-card">
  <button
    class="head"
    type="button"
    aria-expanded={hasBody ? open : undefined}
    disabled={!hasBody}
    onclick={() => (userOpen = !open)}
  >
    <KindIcon kind={tool.toolKind} />
    <span class="title">{tool.title}</span>
    <span class="kindname muted">{tool.toolKind.replace('_', ' ')}</span>
    <span class="status" data-status={tool.status}>
      {#if tool.status === 'in_progress'}<span class="spin" aria-hidden="true"></span>{/if}
      {statusLabel}
    </span>
  </button>
  {#if open && hasBody}
    <div class="body">
      {#if tool.locations.length}
        <ul class="locs" aria-label="Locations">
          {#each tool.locations as l, i (i)}
            <li><code>{l.path}{l.line ? `:${l.line}` : ''}</code></li>
          {/each}
        </ul>
      {/if}
      {#if tool.diffs.length}
        <div class="bar">
          <label class="toggle">
            <input type="checkbox" bind:checked={split} /> side by side
          </label>
        </div>
        {#each tool.diffs as d, i (i)}
          <DiffView diff={d} layout={effectiveLayout} />
        {/each}
      {/if}
      {#if tool.output}
        <pre class="out" aria-label="Tool output">{showAll ? tool.output : out.shown}</pre>
        {#if out.hidden > 0}
          <button class="link" type="button" onclick={() => (showAll = !showAll)}>
            {showAll ? 'Show less' : `Show all (${out.hidden} more lines)`}
          </button>
        {/if}
      {/if}
      {#if tool.rawInput != null}
        <button
          class="link"
          type="button"
          aria-expanded={showInput}
          onclick={() => (showInput = !showInput)}
        >
          {showInput ? 'Hide input' : 'Show input'}
        </button>
        {#if showInput}<pre class="out input" aria-label="Tool input">{JSON.stringify(
              tool.rawInput,
              null,
              2
            )}</pre>{/if}
      {/if}
    </div>
  {/if}
</div>

<style>
  .tool {
    border: 1px solid var(--border);
    border-left-width: 4px;
    border-radius: 6px;
    background: var(--surface);
    width: 100%;
  }
  .tool[data-status='completed'] {
    border-left-color: var(--ok);
  }
  .tool[data-status='failed'] {
    border-left-color: var(--err);
  }
  .tool[data-status='in_progress'],
  .tool[data-status='pending'] {
    border-left-color: var(--warn);
  }
  .head {
    display: flex;
    width: 100%;
    gap: 8px;
    align-items: center;
    border: 0;
    background: none;
    text-align: left;
    padding: 6px 10px;
  }
  .head:disabled {
    opacity: 1;
    cursor: default;
  }
  .title {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .kindname {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .status {
    margin-left: auto;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    display: inline-flex;
    gap: 5px;
    align-items: center;
    color: var(--muted);
    white-space: nowrap;
  }
  .status[data-status='completed'] {
    color: var(--ok);
  }
  .status[data-status='failed'] {
    color: var(--err);
  }
  .status[data-status='in_progress'] {
    color: var(--warn);
  }
  .spin {
    width: 10px;
    height: 10px;
    border: 2px solid currentColor;
    border-right-color: transparent;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .body {
    padding: 0 10px 8px;
    display: grid;
    gap: 6px;
  }
  .locs {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
    color: var(--muted);
  }
  .bar {
    display: flex;
    justify-content: flex-end;
  }
  .toggle {
    font-size: 12px;
    color: var(--muted);
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }
  .out {
    margin: 0;
    padding: 6px 8px;
    background: var(--code-bg);
    border-radius: 4px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 360px;
    overflow: auto;
  }
  .link {
    justify-self: start;
    border: 0;
    background: none;
    color: var(--accent-link);
    padding: 0;
    font-size: 12px;
    text-decoration: underline;
  }
</style>
