<script lang="ts">
  import { renderMarkdown } from '../markdown';

  let { text }: { text: string } = $props();

  const html = $derived(renderMarkdown(text));
  let root: HTMLElement | undefined = $state();

  // Add a copy button to every code block after each (streamed) re-render.
  $effect(() => {
    void html;
    if (!root) return;
    for (const pre of root.querySelectorAll('pre')) {
      if (pre.querySelector(':scope > button.copy')) continue;
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'copy';
      btn.textContent = 'Copy';
      btn.setAttribute('aria-label', 'Copy code');
      pre.appendChild(btn);
    }
  });

  async function onclick(e: MouseEvent) {
    const btn = (e.target as HTMLElement).closest('button.copy');
    if (!btn) return;
    const code = btn.parentElement?.querySelector('code')?.textContent ?? '';
    try {
      await navigator.clipboard.writeText(code);
      btn.textContent = 'Copied';
    } catch {
      btn.textContent = 'Copy failed';
    }
    setTimeout(() => {
      btn.textContent = 'Copy';
    }, 1500);
  }
</script>

<!-- The HTML is sanitized by DOMPurify in renderMarkdown. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="md" bind:this={root} {onclick}>
  <!-- eslint-disable-next-line svelte/no-at-html-tags -->
  {@html html}
</div>

<style>
  .md {
    overflow-wrap: anywhere;
  }
  .md > :global(:first-child) {
    margin-top: 0;
  }
  .md > :global(:last-child) {
    margin-bottom: 0;
  }
  .md :global(h1),
  .md :global(h2),
  .md :global(h3) {
    font-size: 1.05em;
    margin: 0.8em 0 0.3em;
  }
  .md :global(p),
  .md :global(ul),
  .md :global(ol) {
    margin: 0.4em 0;
  }
  .md :global(a) {
    color: var(--accent-link);
  }
  .md :global(code) {
    background: var(--code-bg);
    padding: 1px 4px;
    border-radius: 4px;
  }
  .md :global(pre) {
    position: relative;
    background: var(--code-bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
    overflow-x: auto;
    margin: 0.5em 0;
  }
  .md :global(pre code) {
    background: none;
    padding: 0;
  }
  .md :global(pre button.copy) {
    position: absolute;
    top: 4px;
    right: 4px;
    font-size: 11px;
    padding: 1px 8px;
    opacity: 0.75;
  }
  .md :global(pre:hover button.copy),
  .md :global(pre button.copy:focus-visible) {
    opacity: 1;
  }
  .md :global(blockquote) {
    margin: 0.4em 0;
    padding-left: 10px;
    border-left: 3px solid var(--border);
    color: var(--muted);
  }
  .md :global(table) {
    border-collapse: collapse;
  }
  .md :global(th),
  .md :global(td) {
    border: 1px solid var(--border);
    padding: 2px 8px;
  }
</style>
