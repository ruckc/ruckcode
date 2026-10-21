<script lang="ts">
  /** Renders unified-diff text from the setup preview (added/removed lines coloured). */
  let {
    text,
    label = 'Changes to be written',
    maxHeight = '340px'
  }: { text: string; label?: string; maxHeight?: string } = $props();
  const lines = $derived(text.replace(/\n$/, '').split('\n'));
  function kind(l: string): string {
    if (l.startsWith('+++') || l.startsWith('---')) return 'file';
    if (l.startsWith('+')) return 'add';
    if (l.startsWith('-')) return 'del';
    if (l.startsWith('@@')) return 'hunk';
    if (l.startsWith('# ')) return 'head';
    return '';
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<pre
  class="diff"
  aria-label={label}
  tabindex="0"
  style:max-height={maxHeight}>{#each lines as l, i (i)}<span class={kind(l)}>{l || ' '}</span>
  {/each}</pre>

<style>
  .diff {
    margin: 0;
    padding: 8px 0;
    background: var(--code-bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: auto;
    max-height: 340px;
    /* The markup's own line breaks must not add blank lines; each span is a pre-wrapped block. */
    white-space: normal;
  }
  span {
    display: block;
    padding: 0 10px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .add {
    background: var(--add-bg);
  }
  .del {
    background: var(--del-bg);
  }
  .hunk {
    background: var(--hunk-bg);
    color: var(--muted);
  }
  .file {
    color: var(--muted);
  }
  .head {
    font-weight: 600;
    margin-top: 6px;
  }
</style>
