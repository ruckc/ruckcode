<script lang="ts">
  import { buildDiff, sideBySide } from '../diff';
  import type { DiffContent } from '../state/reducer';

  let {
    diff,
    layout = 'unified',
    compact = false
  }: { diff: DiffContent; layout?: 'unified' | 'split'; compact?: boolean } = $props();

  const model = $derived(buildDiff(diff.oldText, diff.newText, diff.path));
  const sign = { add: '+', del: '-', ctx: ' ' } as const;
</script>

<div class="diff" class:compact aria-label={`Diff of ${diff.path}`} role="group">
  <div class="meta">
    <code>{diff.path}</code>
    {#if diff.oldText === null}<span class="tag">new file</span>{/if}
    <span class="add">+{model.added}</span>
    <span class="del">-{model.removed}</span>
  </div>
  {#if model.hunks.length === 0}
    <div class="empty muted">No changes</div>
  {/if}
  {#each model.hunks as h, hi (hi)}
    <div class="hunk-head">{h.header}</div>
    {#if layout === 'unified'}
      <table class="rows">
        <tbody>
          {#each h.rows as r, ri (ri)}
            <tr class={r.type}>
              <td class="no">{r.oldNo ?? ''}</td>
              <td class="no">{r.newNo ?? ''}</td>
              <td class="sign" aria-hidden="true">{sign[r.type]}</td>
              <td class="code">{r.text}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <table class="rows split">
        <tbody>
          {#each sideBySide(h.rows) as pair, pi (pi)}
            <tr>
              <td class="no">{pair.left?.oldNo ?? ''}</td>
              <td class="code {pair.left?.type ?? 'blank'}">{pair.left?.text ?? ''}</td>
              <td class="no">{pair.right?.newNo ?? ''}</td>
              <td class="code {pair.right?.type ?? 'blank'}">{pair.right?.text ?? ''}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  {/each}
</div>

<style>
  .diff {
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: auto;
    background: var(--code-bg);
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12.5px;
  }
  .diff.compact {
    max-height: 220px;
  }
  .meta {
    display: flex;
    gap: 8px;
    align-items: baseline;
    padding: 3px 8px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .tag {
    font-size: 11px;
    color: var(--muted);
  }
  .add {
    color: var(--ok);
  }
  .del {
    color: var(--err);
  }
  .hunk-head {
    padding: 1px 8px;
    color: var(--muted);
    background: var(--hunk-bg);
  }
  .empty {
    padding: 6px 8px;
  }
  table {
    border-collapse: collapse;
    width: 100%;
  }
  .split {
    table-layout: fixed;
  }
  .no {
    width: 3.2em;
    text-align: right;
    padding: 0 6px;
    color: var(--muted);
    user-select: none;
    vertical-align: top;
  }
  .sign {
    width: 1.2em;
    text-align: center;
    user-select: none;
  }
  .code {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    padding: 0 6px;
  }
  tr.add,
  td.add {
    background: var(--add-bg);
    color: inherit;
  }
  tr.del,
  td.del {
    background: var(--del-bg);
  }
  td.blank {
    background: var(--hunk-bg);
  }
  tr.add .sign {
    color: var(--ok);
  }
  tr.del .sign {
    color: var(--err);
  }
</style>
