<script lang="ts">
  import { tick } from 'svelte';
  import AskDialog from '../lib/components/AskDialog.svelte';
  import PermissionDialog from '../lib/components/PermissionDialog.svelte';
  import ToolCard from '../lib/components/ToolCard.svelte';
  import { hostMode } from '../lib/host';
  import { pickFolder } from '../lib/host/tauri';
  import { ChatController } from '../lib/state/chat.svelte';

  const chat = new ChatController();
  const mode = hostMode();

  function loadCwd(): string {
    try {
      return localStorage.getItem('ruckcode.cwd') ?? '';
    } catch {
      return '';
    }
  }

  let cwd = $state(loadCwd() || (mode === 'mock' ? '/demo/project' : ''));
  let prompt = $state('');
  let scroller: HTMLElement | undefined = $state();

  const connected = $derived(chat.status === 'ready' || chat.status === 'busy');

  async function browse() {
    const dir = await pickFolder().catch(() => null);
    if (dir) cwd = dir;
  }

  async function connect() {
    try {
      localStorage.setItem('ruckcode.cwd', cwd);
    } catch {
      // storage unavailable; the folder just isn't remembered
    }
    await chat.connect({ cwd });
  }

  async function submit(e: Event) {
    e.preventDefault();
    const text = prompt;
    prompt = '';
    await chat.send(text);
  }

  $effect(() => {
    // Keep the newest output in view while streaming.
    void chat.model.items.length;
    void chat.model.items.at(-1);
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
  });
</script>

<div class="app">
  <header>
    <strong>ruckcode</strong>
    <input
      type="text"
      class="cwd"
      placeholder="Project folder"
      aria-label="Project folder"
      bind:value={cwd}
      disabled={connected}
    />
    {#if mode === 'tauri'}
      <button onclick={browse} disabled={connected}>Browse...</button>
    {/if}
    {#if connected}
      <button onclick={() => chat.disconnect()}>Disconnect</button>
    {:else}
      <button
        class="primary"
        onclick={connect}
        disabled={!cwd.trim() || chat.status === 'connecting' || mode === 'none'}
      >
        {chat.status === 'connecting' ? 'Connecting...' : 'Connect'}
      </button>
    {/if}
    <span class="spacer"></span>
    {#if connected}
      <label class="mode">
        mode
        {#if chat.modes.length}
          <select
            value={chat.model.currentModeId ?? ''}
            onchange={(e) => chat.setMode(e.currentTarget.value)}
            aria-label="Mode"
          >
            {#each chat.modes as m (m.id)}<option value={m.id}>{m.name}</option>{/each}
          </select>
        {:else}
          <code data-testid="mode">{chat.model.currentModeId ?? '-'}</code>
        {/if}
      </label>
      {#if chat.model.usage}
        <span class="muted" title="context tokens used / window">
          {chat.model.usage.used.toLocaleString()} / {chat.model.usage.size.toLocaleString()} tok
        </span>
      {/if}
    {/if}
  </header>

  {#if mode === 'none'}
    <p class="banner">
      No agent host available. Run the desktop app (<code>mise run app:dev</code>), or open this
      page with <code>?mock=1</code> for a scripted demo agent.
    </p>
  {/if}
  {#if chat.error}<p class="banner error" role="alert">{chat.error}</p>{/if}

  <main bind:this={scroller}>
    {#if !connected && chat.model.items.length === 0}
      <p class="muted empty">Choose a project folder and connect to start a session.</p>
    {/if}
    {#each chat.model.items as item, i (i)}
      {#if item.kind === 'user'}
        <div class="msg user">{item.text}</div>
      {:else if item.kind === 'assistant'}
        <div class="msg assistant">{item.text}</div>
      {:else if item.kind === 'thought'}
        <details class="thought">
          <summary>reasoning</summary>
          {item.text}
        </details>
      {:else if item.kind === 'tool'}
        <ToolCard tool={item} />
      {:else}
        <div class="notice" data-severity={item.severity}>
          {item.title}{item.description ? `: ${item.description}` : ''}
        </div>
      {/if}
    {/each}
    {#if chat.model.plan.length}
      <ul class="plan" aria-label="Plan">
        {#each chat.model.plan as p, i (i)}
          <li data-status={p.status}>{p.content}</li>
        {/each}
      </ul>
    {/if}
  </main>

  <form class="composer" onsubmit={submit}>
    <textarea
      rows="2"
      placeholder={connected ? 'Message the agent (try "mock:write a.txt hi")' : 'Not connected'}
      aria-label="Prompt"
      bind:value={prompt}
      disabled={!connected}
      onkeydown={(e) => {
        if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
          e.preventDefault();
          e.currentTarget.form?.requestSubmit();
        }
      }}></textarea>
    {#if chat.status === 'busy'}
      <button type="button" class="danger" onclick={() => chat.cancel()}>Cancel</button>
    {:else}
      <button class="primary" type="submit" disabled={!connected || !prompt.trim()}>Send</button>
    {/if}
  </form>
</div>

{#if chat.permission}<PermissionDialog pending={chat.permission} />{/if}
{#if chat.question}<AskDialog pending={chat.question} />{/if}

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  header {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .cwd {
    flex: 1;
    max-width: 420px;
    min-width: 120px;
  }
  .spacer {
    flex: 1;
  }
  .mode {
    display: flex;
    gap: 6px;
    align-items: center;
    color: var(--muted);
  }
  .banner {
    margin: 0;
    padding: 6px 12px;
    background: var(--code-bg);
    border-bottom: 1px solid var(--border);
  }
  .banner.error {
    color: var(--err);
  }
  main {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
  }
  main > :global(*) {
    max-width: min(820px, 100%);
  }
  .empty {
    align-self: center;
    margin-top: 20vh;
  }
  .msg {
    white-space: pre-wrap;
    padding: 6px 10px;
    border-radius: 8px;
  }
  .msg.user {
    background: var(--accent);
    color: var(--accent-text);
    align-self: flex-end;
  }
  .msg.assistant {
    background: var(--surface);
    border: 1px solid var(--border);
  }
  .thought {
    color: var(--muted);
    font-style: italic;
    white-space: pre-wrap;
  }
  .notice {
    color: var(--muted);
    font-size: 12px;
  }
  .notice[data-severity='warning'] {
    color: var(--warn);
  }
  .notice[data-severity='error'] {
    color: var(--err);
  }
  .plan {
    margin: 0;
    padding-left: 20px;
    color: var(--muted);
  }
  .plan li[data-status='completed'] {
    text-decoration: line-through;
  }
  .plan li[data-status='in_progress'] {
    color: var(--text);
    font-weight: 600;
  }
  .composer {
    display: flex;
    gap: 8px;
    padding: 8px 12px;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .composer textarea {
    flex: 1;
    resize: none;
  }
</style>
