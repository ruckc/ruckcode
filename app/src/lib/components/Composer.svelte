<script lang="ts">
  import type { CommandInfo } from '../state/reducer';

  let {
    disabled,
    running,
    queue,
    history,
    placeholder,
    commands = [],
    onsend,
    oncancel,
    onunqueue
  }: {
    disabled: boolean;
    running: boolean;
    queue: string[];
    history: string[];
    placeholder: string;
    /** Slash commands offered while typing `/...`. */
    commands?: CommandInfo[];
    onsend: (text: string) => void;
    oncancel: () => void;
    onunqueue: (index: number) => void;
  } = $props();

  let text = $state('');
  let input: HTMLTextAreaElement | undefined = $state();
  /** Position in `history` while recalling (history.length = not recalling). */
  let cursor = $state(-1);
  let draft = '';

  /** Commands matching a `/name` being typed (before any argument). */
  const matches = $derived.by(() => {
    const m = /^\/(\S*)$/.exec(text);
    if (!m || cursor !== -1) return [];
    return commands.filter((c) => c.name.startsWith(m[1]));
  });
  let pick = $state(0);
  $effect(() => {
    void matches;
    pick = 0;
  });

  function complete(c: CommandInfo) {
    text = `/${c.name}${c.hint ? ' ' : ''}`;
    queueMicrotask(() => {
      input?.focus();
      input?.setSelectionRange(text.length, text.length);
    });
  }

  export function focus() {
    input?.focus();
  }

  function submit() {
    const t = text.trim();
    if (!t || disabled) return;
    onsend(text);
    text = '';
    cursor = -1;
  }

  function recall(delta: -1 | 1) {
    if (!history.length) return false;
    if (cursor === -1) {
      if (delta === 1) return false;
      draft = text;
      cursor = history.length;
    }
    const n = cursor + delta;
    if (n < 0) return true;
    if (n >= history.length) {
      cursor = -1;
      text = draft;
    } else {
      cursor = n;
      text = history[n];
    }
    queueMicrotask(() => input?.setSelectionRange(text.length, text.length));
    return true;
  }

  function onkeydown(e: KeyboardEvent) {
    if (matches.length) {
      const c = matches[pick];
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const d = e.key === 'ArrowDown' ? 1 : -1;
        pick = (pick + d + matches.length) % matches.length;
        return;
      }
      if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey && text !== `/${c.name}`)) {
        e.preventDefault();
        complete(c);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        text = '';
        return;
      }
    }
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    } else if (e.key === 'Escape' && running) {
      e.preventDefault();
      oncancel();
    } else if (e.key === 'ArrowUp' && !e.shiftKey && (cursor !== -1 || !text.includes('\n'))) {
      // Only recall when the box is empty/recalled, or the caret is on the first line.
      const el = e.currentTarget as HTMLTextAreaElement;
      if (cursor !== -1 || el.selectionStart === 0) {
        if (recall(-1)) e.preventDefault();
      }
    } else if (e.key === 'ArrowDown' && !e.shiftKey && cursor !== -1) {
      if (recall(1)) e.preventDefault();
    }
  }
</script>

<form
  class="composer"
  onsubmit={(e) => {
    e.preventDefault();
    submit();
  }}
>
  {#if queue.length}
    <ol class="queue" aria-label="Queued prompts">
      {#each queue as q, i (i)}
        <li>
          <span class="muted">queued</span>
          <span class="qtext">{q}</span>
          <button
            type="button"
            class="x"
            aria-label={`Remove queued prompt ${i + 1}`}
            onclick={() => onunqueue(i)}>&times;</button
          >
        </li>
      {/each}
    </ol>
  {/if}
  {#if matches.length}
    <ul class="cmds" role="listbox" aria-label="Commands">
      {#each matches as c, i (c.name)}
        <li role="option" aria-selected={i === pick}>
          <button
            type="button"
            tabindex="-1"
            onmousedown={(e) => e.preventDefault()}
            onclick={() => complete(c)}
          >
            <code>/{c.name}</code>
            {#if c.hint}<span class="muted">[{c.hint}]</span>{/if}
            <span class="muted desc">{c.description}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
  <div class="row">
    <textarea
      bind:this={input}
      rows="2"
      {placeholder}
      aria-label="Prompt"
      bind:value={text}
      {disabled}
      {onkeydown}
      oninput={() => {
        if (cursor !== -1) cursor = -1;
      }}></textarea>
    <div class="btns">
      {#if running}
        <button
          type="button"
          class="danger"
          onclick={oncancel}
          aria-label="Cancel the running turn"
        >
          Stop <kbd>Esc</kbd>
        </button>
      {/if}
      <button class="primary" type="submit" disabled={disabled || !text.trim()}>
        {running ? 'Queue' : 'Send'}
      </button>
    </div>
  </div>
  <p class="hint muted">
    Enter to send, Shift+Enter for a new line, Up/Down for history, / for commands.
  </p>
</form>

<style>
  .cmds {
    list-style: none;
    margin: 0 0 6px;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
  }
  .cmds button {
    display: flex;
    gap: 8px;
    align-items: baseline;
    width: 100%;
    border: 0;
    background: none;
    text-align: left;
    min-height: 0;
    padding: 4px 8px;
  }
  .cmds li[aria-selected='true'] button {
    background: var(--code-bg);
  }
  .desc {
    margin-left: auto;
  }
  .composer {
    padding: 8px 12px 6px;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .row {
    display: flex;
    gap: 8px;
    align-items: stretch;
  }
  textarea {
    flex: 1;
    resize: none;
  }
  .btns {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .queue {
    margin: 0 0 6px;
    padding: 0;
    list-style: none;
    display: grid;
    gap: 2px;
    font-size: 12.5px;
  }
  .queue li {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .qtext {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .x {
    border: 0;
    background: none;
    padding: 0 6px;
    font-size: 16px;
    line-height: 1;
  }
  .hint {
    margin: 4px 0 0;
    font-size: 11px;
  }
  kbd {
    font: inherit;
    font-size: 11px;
    opacity: 0.8;
    margin-left: 4px;
  }
</style>
