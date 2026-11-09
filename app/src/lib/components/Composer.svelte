<script lang="ts">
  import { ChatComposer } from '@ruckc/truss-chat';
  import { Button, Kbd } from '@ruckc/truss-ui';
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
  let root: HTMLElement | undefined = $state();
  /** Position in `history` while recalling (history.length = not recalling). */
  let cursor = $state(-1);
  let draft = '';

  const field = () => root?.querySelector('textarea') ?? null;

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
      field()?.focus();
      field()?.setSelectionRange(text.length, text.length);
    });
  }

  export function focus() {
    field()?.focus();
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
    queueMicrotask(() => field()?.setSelectionRange(text.length, text.length));
    return true;
  }

  /**
   * TODO(truss-ui#136): ChatComposer needs focus() and a key hook; until then, a capture-phase handler.
   * truss-chat's composer owns Enter (send) and Shift+Enter; this capture-phase handler adds what
   * ruckcode needs on top: slash-command completion, Esc to stop and Up/Down prompt history.
   */
  function onkeydowncapture(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (matches.length) {
      const c = matches[pick];
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        e.stopPropagation();
        const d = e.key === 'ArrowDown' ? 1 : -1;
        pick = (pick + d + matches.length) % matches.length;
        return;
      }
      if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey && text !== `/${c.name}`)) {
        e.preventDefault();
        e.stopPropagation();
        complete(c);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        text = '';
        return;
      }
    }
    if (e.key === 'Escape' && running) {
      e.preventDefault();
      oncancel();
    } else if (e.key === 'ArrowUp' && !e.shiftKey && (cursor !== -1 || !text.includes('\n'))) {
      // Only recall when the box is empty/recalled, or the caret is on the first line.
      const el = e.target as HTMLTextAreaElement;
      if (cursor !== -1 || el.selectionStart === 0) {
        if (recall(-1)) e.preventDefault();
      }
    } else if (e.key === 'ArrowDown' && !e.shiftKey && cursor !== -1) {
      if (recall(1)) e.preventDefault();
    }
  }

  function send(t: string) {
    cursor = -1;
    onsend(t);
  }
</script>

<div class="composer" bind:this={root}>
  {#if queue.length}
    <ol class="queue" aria-label="Queued prompts">
      {#each queue as q, i (i)}
        <li>
          <span class="muted">queued</span>
          <span class="qtext">{q}</span>
          <Button
            variant="minimal"
            intent="neutral"
            size="sm"
            icon="cross"
            aria-label={`Remove queued prompt ${i + 1}`}
            onclick={() => onunqueue(i)}
          />
        </li>
      {/each}
    </ol>
  {/if}
  {#if matches.length}
    <ul class="cmds" role="listbox" aria-label="Commands">
      {#each matches as c, i (c.name)}
        <li role="option" aria-selected={i === pick} data-picked={i === pick}>
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
  <ChatComposer
    bind:value={text}
    {disabled}
    {placeholder}
    label="Prompt"
    sendLabel={running ? 'Queue' : 'Send'}
    maxRows={8}
    onsend={send}
    {onkeydowncapture}
    oninput={() => {
      if (cursor !== -1) cursor = -1;
    }}
  >
    {#snippet actions()}
      {#if running}
        <Button
          intent="danger"
          variant="outlined"
          aria-label="Cancel the running turn"
          onclick={oncancel}
        >
          Stop <Kbd keys="Esc" />
        </Button>
      {/if}
    {/snippet}
  </ChatComposer>
  <p class="hint muted">
    Enter to send, Shift+Enter for a new line, Up/Down for history, / for commands.
  </p>
</div>

<style>
  .composer {
    padding: var(--tr-space-2) var(--tr-space-3) var(--tr-space-1);
    border-top: 1px solid var(--tr-outline-variant);
    background: var(--tr-surface-container-low);
  }
  .cmds {
    list-style: none;
    margin: 0 0 var(--tr-space-2);
    padding: var(--tr-space-1);
    border: 1px solid var(--tr-outline-variant);
    border-radius: var(--tr-radius-md);
    background: var(--tr-surface);
  }
  .cmds button {
    display: flex;
    gap: var(--tr-space-2);
    align-items: baseline;
    width: 100%;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    padding: var(--tr-space-1) var(--tr-space-2);
    border-radius: var(--tr-radius-sm);
    cursor: pointer;
  }
  .cmds li[data-picked='true'] button {
    background: var(--tr-surface-container-high);
  }
  .desc {
    margin-left: auto;
  }
  .queue {
    margin: 0 0 var(--tr-space-2);
    padding: 0;
    list-style: none;
    display: grid;
    gap: 2px;
    font-size: var(--tr-text-sm);
  }
  .queue li {
    display: flex;
    gap: var(--tr-space-2);
    align-items: center;
  }
  .qtext {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hint {
    margin: var(--tr-space-1) 0 0;
    font-size: var(--tr-text-xs);
  }
</style>
