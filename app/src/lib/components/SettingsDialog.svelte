<script lang="ts">
  import { focusTrap } from '../a11y';
  import type { EndpointSuggestion, SetupApi, SetupState, SetupTarget } from '../host/setup';
  import { SetupWizard, type SettingsTab } from '../state/wizard.svelte';
  import KeysTab from './KeysTab.svelte';
  import type { UpdateController } from '../state/updates.svelte';
  import ProjectTab from './ProjectTab.svelte';
  import UpdatesTab from './UpdatesTab.svelte';
  import SetupWizardView from './SetupWizardView.svelte';

  let {
    api,
    target,
    tab = $bindable('providers'),
    firstRun = false,
    updates = null,
    onclose,
    onreconnect
  }: {
    api: SetupApi;
    target: SetupTarget;
    tab?: SettingsTab;
    /** Opened automatically because there is no usable profile. */
    firstRun?: boolean;
    /** The app's update controller; adds the Updates tab. */
    updates?: UpdateController | null;
    onclose: () => void;
    onreconnect: () => void;
  } = $props();

  const TABS: { id: SettingsTab; label: string }[] = $derived([
    { id: 'providers', label: 'Providers & models' },
    { id: 'keys', label: 'API keys' },
    { id: 'project', label: 'Project' },
    ...(updates ? [{ id: 'updates' as const, label: 'Updates' }] : [])
  ]);

  let info = $state<SetupState | null>(null);
  let presets = $state<EndpointSuggestion[]>([]);
  let loadError = $state<string | null>(null);
  let wizard = $state<SetupWizard | null>(null);

  async function load() {
    loadError = null;
    try {
      const [p, s] = await Promise.all([api.presets(), api.state(target)]);
      presets = p;
      info = s;
      wizard = new SetupWizard(api, target, p, s);
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
    }
  }
  void load();

  function changed(s: SetupState) {
    info = s;
    // An untouched wizard follows changes made in other tabs (new keys, new modes.toml).
    if (wizard && wizard.step === 'key' && !wizard.newKeySecret && !wizard.endpointTouched) {
      wizard = new SetupWizard(api, target, presets, s);
    }
  }

  function close() {
    wizard?.clearSecrets();
    onclose();
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
    tab = TABS[(i + d + TABS.length) % TABS.length].id;
    queueMicrotask(() => document.getElementById(`tab-${tab}`)?.focus());
  }
</script>

<svelte:window {onkeydown} />

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

    {#if firstRun && tab === 'providers' && wizard?.step !== 'done'}
      <p class="welcome" role="note">
        Welcome! No model provider is configured yet. Pick one below to get started.
      </p>
    {/if}

    <div class="tabs" role="tablist" aria-label="Settings" tabindex="-1" onkeydown={tabKey}>
      {#each TABS as t (t.id)}
        <button
          type="button"
          role="tab"
          id={`tab-${t.id}`}
          aria-selected={tab === t.id}
          aria-controls={`panel-${t.id}`}
          tabindex={tab === t.id ? 0 : -1}
          onclick={() => (tab = t.id)}>{t.label}</button
        >
      {/each}
    </div>

    <div class="panel" role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
      {#if tab === 'updates' && updates}
        <UpdatesTab {updates} />
      {:else if loadError}
        <p class="err" role="alert">{loadError}</p>
        <button type="button" onclick={() => load()}>Retry</button>
      {:else if !info || !wizard}
        <p class="muted" role="status">Loading...</p>
      {:else if tab === 'providers'}
        {#key wizard}
          <SetupWizardView {wizard} onclose={close} {onreconnect} />
        {/key}
      {:else if tab === 'keys'}
        <KeysTab {api} {target} {info} onchange={changed} />
      {:else}
        <ProjectTab {api} {target} {info} onchange={changed} />
      {/if}
    </div>
  </div>
</div>

<style>
  .settings {
    width: min(760px, 96vw);
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
  .welcome {
    margin: 8px 0 0;
    padding: 6px 10px;
    border-radius: 6px;
    background: var(--code-bg);
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
