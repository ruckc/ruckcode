<script lang="ts">
  import type { ChatModel } from '../state/reducer';
  import Icon from './Icon.svelte';
  import Menu, { type MenuItem } from './Menu.svelte';

  let {
    model,
    title,
    project = '',
    disabled,
    connected = false,
    view = 'chat',
    onback,
    onsessions,
    onmode,
    onconfig,
    menu
  }: {
    model: ChatModel | null;
    title: string;
    /** The connected project, shown next to the session title. */
    project?: string;
    disabled: boolean;
    connected?: boolean;
    view?: 'chat' | 'transcripts';
    /** Leave a secondary view (Transcripts) for the chat. */
    onback?: () => void;
    /** Open the project & session picker. */
    onsessions: () => void;
    onmode: (id: string) => void;
    onconfig: (id: string, value: string) => void;
    /** The overflow menu: rarely used actions. */
    menu: (MenuItem | 'sep')[];
  } = $props();
  const modelOpt = $derived(
    model?.configOptions.find((o) => o.id === 'model' || o.category === 'model')
  );
  const pct = $derived(
    model?.usage && model.usage.size > 0
      ? Math.min(100, Math.round((model.usage.used / model.usage.size) * 100))
      : 0
  );
</script>

<header class="top">
  {#if view !== 'chat'}
    <button
      type="button"
      class="icon ghost"
      aria-label="Back to chat"
      title="Back to chat"
      onclick={onback}><Icon name="back" /></button
    >
    <strong class="title">Transcripts</strong>
  {:else}
    <button
      type="button"
      class="sessions ghost"
      aria-label="Projects and sessions"
      title="Open a project or session"
      onclick={onsessions}
    >
      <Icon name="folder" />
      <strong class="title" {title}>{title}</strong>
      {#if project}<span class="muted proj">{project}</span>{/if}
    </button>
  {/if}
  <span class="spacer"></span>
  {#if model && connected && view === 'chat'}
    {#if model.modes.length}
      <select
        aria-label="Mode"
        title="Mode"
        value={model.currentModeId ?? ''}
        {disabled}
        onchange={(e) => onmode(e.currentTarget.value)}
      >
        {#each model.modes as m (m.id)}
          <option value={m.id} title={m.description}>{m.name}</option>
        {/each}
      </select>
    {/if}
    {#if modelOpt}
      <select
        aria-label="Model"
        title={modelOpt.name}
        value={modelOpt.currentValue}
        {disabled}
        onchange={(e) => onconfig(modelOpt.id, e.currentTarget.value)}
      >
        {#each modelOpt.options as o (o.value)}
          <option value={o.value}>{o.name}</option>
        {/each}
      </select>
    {/if}
    {#if model.usage}
      <div
        class="usage"
        role="meter"
        aria-label="Context window usage"
        aria-valuemin={0}
        aria-valuemax={model.usage.size}
        aria-valuenow={model.usage.used}
        aria-valuetext={`${model.usage.used} of ${model.usage.size} tokens`}
        title={`Context: ${model.usage.used.toLocaleString()} / ${model.usage.size.toLocaleString()} tokens`}
      >
        <div class="bar">
          <div class="fill" class:hot={pct >= 80} style:width={`${pct}%`}></div>
        </div>
      </div>
    {/if}
  {/if}
  <Menu label="More" items={menu} />
</header>

<style>
  .top {
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 30ch;
  }
  .sessions {
    display: flex;
    gap: 8px;
    align-items: center;
    min-width: 0;
    max-width: 60%;
  }
  .sessions :global(svg) {
    width: 16px;
    height: 16px;
    flex: none;
  }
  .proj {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .usage {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 12px;
  }
  .bar {
    width: 70px;
    height: 6px;
    border-radius: 3px;
    background: var(--border);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent);
  }
  .fill.hot {
    background: var(--err);
  }
</style>
