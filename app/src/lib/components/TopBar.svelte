<script lang="ts">
  import type { ChatModel } from '../state/reducer';

  let {
    model,
    title,
    disabled,
    planOpen = $bindable(),
    sidebarOpen = $bindable(),
    view = 'chat',
    onview,
    onmode,
    onconfig,
    onsettings
  }: {
    model: ChatModel | null;
    title: string;
    disabled: boolean;
    planOpen: boolean;
    sidebarOpen: boolean;
    view?: 'chat' | 'transcripts' | 'modes';
    onview?: (v: 'chat' | 'transcripts' | 'modes') => void;
    onmode: (id: string) => void;
    onconfig: (id: string, value: string) => void;
    onsettings: () => void;
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
  <button
    type="button"
    class="small toggle"
    aria-pressed={sidebarOpen}
    aria-label="Toggle sidebar"
    onclick={() => (sidebarOpen = !sidebarOpen)}>&#9776;</button
  >
  <strong class="title" {title}
    >{view === 'transcripts' ? 'Transcripts' : view === 'modes' ? 'Modes' : title}</strong
  >
  {#if onview}
    <div class="views" role="group" aria-label="View">
      <button
        type="button"
        class="small toggle"
        aria-pressed={view === 'chat'}
        onclick={() => onview('chat')}>Chat</button
      >
      <button
        type="button"
        class="small toggle"
        aria-pressed={view === 'transcripts'}
        onclick={() => onview('transcripts')}>Transcripts</button
      >
      <button
        type="button"
        class="small toggle"
        aria-pressed={view === 'modes'}
        onclick={() => onview('modes')}>Modes</button
      >
    </div>
  {/if}
  <span class="spacer"></span>
  {#if model && view === 'chat'}
    <label class="field">
      <span class="muted">Mode</span>
      {#if model.modes.length}
        <select
          aria-label="Mode"
          value={model.currentModeId ?? ''}
          {disabled}
          onchange={(e) => onmode(e.currentTarget.value)}
        >
          {#each model.modes as m (m.id)}
            <option value={m.id} title={m.description}>{m.name}</option>
          {/each}
        </select>
      {:else}
        <code data-testid="mode">{model.currentModeId ?? '-'}</code>
      {/if}
    </label>
    {#if modelOpt}
      <label class="field">
        <span class="muted">{modelOpt.name}</span>
        <select
          aria-label="Model"
          value={modelOpt.currentValue}
          {disabled}
          onchange={(e) => onconfig(modelOpt.id, e.currentTarget.value)}
        >
          {#each modelOpt.options as o (o.value)}
            <option value={o.value}>{o.name}</option>
          {/each}
        </select>
      </label>
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
        title="Context tokens used / window"
      >
        <div class="bar">
          <div class="fill" class:hot={pct >= 80} style:width={`${pct}%`}></div>
        </div>
        <span class="muted"
          >{model.usage.used.toLocaleString()} / {model.usage.size.toLocaleString()}</span
        >
      </div>
    {/if}
  {/if}
  <button type="button" class="small toggle" onclick={onsettings}>Settings</button>
  <button
    type="button"
    class="small toggle"
    aria-pressed={planOpen}
    onclick={() => (planOpen = !planOpen)}>Plan</button
  >
</header>

<style>
  .top {
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
    flex-wrap: wrap;
  }
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 30ch;
  }
  .views {
    display: flex;
    gap: 4px;
  }
  .spacer {
    flex: 1;
  }
  .field {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .toggle[aria-pressed='true'] {
    background: var(--code-bg);
    border-color: var(--accent);
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
