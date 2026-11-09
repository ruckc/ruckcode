<script lang="ts">
  import { Button, MenuButton, ProgressBar, Select } from '@ruckc/truss-ui';
  import type { MenuEntry } from '@ruckc/truss-ui';
  import type { ChatModel } from '../state/reducer';

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
    menu: MenuEntry[];
  } = $props();

  // TODO(truss-ui#148): this is a meter (role=meter, valuetext), not task progress.
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
    <Button
      variant="minimal"
      intent="neutral"
      icon="arrow-left"
      aria-label="Back to chat"
      title="Back to chat"
      onclick={onback}
    />
    <strong class="title">Transcripts</strong>
  {:else}
    <Button
      variant="minimal"
      intent="neutral"
      icon="folder"
      class="sessions"
      aria-label="Projects and sessions"
      title="Open a project or session"
      onclick={onsessions}
    >
      <strong class="title" {title}>{title}</strong>
      {#if project}<span class="muted proj">{project}</span>{/if}
    </Button>
  {/if}
  <span class="spacer"></span>
  {#if model && connected && view === 'chat'}
    {#if model.modes.length}
      <Select
        ariaLabel="Mode"
        size="sm"
        {disabled}
        value={model.currentModeId ?? ''}
        options={model.modes.map((m) => ({
          value: m.id,
          label: m.name,
          description: m.description
        }))}
        onchange={(id) => onmode(id)}
      />
    {/if}
    {#if modelOpt}
      <Select
        ariaLabel="Model"
        size="sm"
        {disabled}
        value={modelOpt.currentValue}
        options={modelOpt.options.map((o) => ({ value: o.value, label: o.name }))}
        onchange={(v) => onconfig(modelOpt.id, v)}
      />
    {/if}
    {#if model.usage}
      <div
        class="usage"
        title={`Context: ${model.usage.used.toLocaleString()} / ${model.usage.size.toLocaleString()} tokens`}
      >
        <ProgressBar
          value={pct}
          size="sm"
          intent={pct >= 80 ? 'danger' : 'primary'}
          label="Context window usage"
        />
      </div>
    {/if}
  {/if}
  <MenuButton label="More" iconOnly variant="minimal" intent="neutral" align="end" items={menu} />
</header>

<style>
  .top {
    display: flex;
    gap: var(--tr-space-3);
    align-items: center;
    padding: var(--tr-space-1) var(--tr-space-3);
    border-bottom: 1px solid var(--tr-outline-variant);
    background: var(--tr-surface-container-low);
  }
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 30ch;
  }
  .top :global(.sessions) {
    min-width: 0;
    max-width: 60%;
  }
  .proj {
    font-size: var(--tr-text-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .usage {
    width: 120px;
  }
  /* The meter's caption is the accessible name; the bar alone is enough visually. */
  .usage :global(.meta) {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
</style>
