<script lang="ts">
  import { focusTrap } from '../a11y';
  import type { SetupApi, SetupState, SetupTarget } from '../host/setup';
  import type { SettingsTab } from '../state/wizard.svelte';
  import KeysTab from './KeysTab.svelte';
  import type { UpdateController } from '../state/updates.svelte';
  import ProjectTab from './ProjectTab.svelte';
  import UpdatesTab from './UpdatesTab.svelte';
  import ProvidersTab from './ProvidersTab.svelte';
  import ModesView from './ModesView.svelte';
  import UnsavedDialog from './UnsavedDialog.svelte';
  import type { ModesApi } from '../host/modes';

  let {
    api,
    target,
    tab = $bindable('providers'),
    updates = null,
    modesApi = null,
    canReconnect = false,
    onreconnect,
    onclose,
    onaddprovider
  }: {
    api: SetupApi;
    target: SetupTarget;
    tab?: SettingsTab;
    /** The app's update controller; adds the Updates tab. */
    updates?: UpdateController | null;
    /** Adds the Modes tab (the modes.toml editor). */
    modesApi?: ModesApi | null;
    canReconnect?: boolean;
    /** Restart the agent so saved modes take effect. */
    onreconnect?: () => void;
    onclose: () => void;
    /** Open the setup wizard (a separate dialog) to add a provider. */
    onaddprovider: () => void;
  } = $props();

  const TABS: { id: SettingsTab; label: string }[] = $derived([
    { id: 'providers', label: 'Providers' },
    { id: 'keys', label: 'API keys' },
    ...(modesApi ? [{ id: 'modes' as const, label: 'Modes' }] : []),
    { id: 'project', label: 'Project' },
    ...(updates ? [{ id: 'updates' as const, label: 'Updates' }] : [])
  ]);

  let info = $state<SetupState | null>(null);
  let loadError = $state<string | null>(null);

  async function load() {
    loadError = null;
    try {
      info = await api.state(target);
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
    }
  }
  void load();

  function changed(s: SetupState) {
    info = s;
  }

  /** The Modes tab has unsaved edits. */
  let modesDirty = $state(false);
  /** An action waiting on "keep editing / discard" for unsaved modes. */
  let pending = $state<(() => void) | null>(null);

  function guard(action: () => void) {
    if (tab === 'modes' && modesDirty) pending = action;
    else action();
  }

  function discard() {
    const a = pending;
    pending = null;
    modesDirty = false;
    a?.();
  }

  function close() {
    guard(onclose);
  }

  function select(t: SettingsTab) {
    if (t !== tab) guard(() => (tab = t));
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && !e.defaultPrevented) {
      e.preventDefault();
      close();
    }
  }

  function tabKey(e: KeyboardEvent) {
    const i = TABS.findIndex((t) => t.id === tab);
    const d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    if (!d) return;
    e.preventDefault();
    select(TABS[(i + d + TABS.length) % TABS.length].id);
    queueMicrotask(() => document.getElementById(`tab-${tab}`)?.focus());
  }
</script>

<svelte:window
  {onkeydown}
  onbeforeunload={(e) => {
    if (modesDirty) e.preventDefault();
  }}
/>

<div class="backdrop" role="presentation">
  <div
    class="dialog settings"
    role="dialog"
    aria-modal="true"
    aria-labelledby="settings-title"
    use:focusTrap
  >
    <header>
      <h2 id="settings-title">Settings</h2>
      <span class="muted proj">
        {target.project}{target.distro ? ` (WSL: ${target.distro})` : ''}
      </span>
      <button type="button" class="ghost" aria-label="Close settings" onclick={close}
        >&times;</button
      >
    </header>

    <div class="tabs" role="tablist" aria-label="Settings" tabindex="-1" onkeydown={tabKey}>
      {#each TABS as t (t.id)}
        <button
          type="button"
          role="tab"
          id={`tab-${t.id}`}
          aria-selected={tab === t.id}
          aria-controls={`panel-${t.id}`}
          tabindex={tab === t.id ? 0 : -1}
          onclick={() => select(t.id)}>{t.label}</button
        >
      {/each}
    </div>

    <div class="panel" role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
      {#if tab === 'updates' && updates}
        <UpdatesTab {updates} />
      {:else if loadError}
        <p class="err" role="alert">{loadError}</p>
        <button type="button" onclick={() => load()}>Retry</button>
      {:else if !info}
        <p class="muted" role="status">Loading...</p>
      {:else if tab === 'providers'}
        <ProvidersTab {info} onadd={onaddprovider} />
      {:else if tab === 'modes' && modesApi}
        <ModesView
          api={modesApi}
          {target}
          ondirty={(d) => (modesDirty = d)}
          {canReconnect}
          {onreconnect}
        />
      {:else if tab === 'keys'}
        <KeysTab {api} {target} {info} onchange={changed} />
      {:else}
        <ProjectTab {api} {target} {info} onchange={changed} />
      {/if}
    </div>
  </div>
</div>

{#if pending}
  <UnsavedDialog onkeep={() => (pending = null)} ondiscard={discard} />
{/if}

<style>
  .settings {
    width: min(1000px, 96vw);
    height: min(760px, 92vh);
    max-height: 92vh;
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    gap: 10px;
    align-items: baseline;
  }
  header h2 {
    margin: 0;
  }
  .proj {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }
  .tabs {
    display: flex;
    gap: 4px;
    border-bottom: 1px solid var(--border);
    margin: 8px 0;
    flex-wrap: wrap;
  }
  .tabs button {
    border: 1px solid transparent;
    border-bottom: 2px solid transparent;
    border-radius: 6px 6px 0 0;
    background: none;
  }
  .tabs button[aria-selected='true'] {
    border-bottom-color: var(--accent);
    font-weight: 600;
  }
  .panel {
    overflow: auto;
    min-height: 200px;
    padding: 2px;
  }
  .err {
    color: var(--err);
  }
</style>
