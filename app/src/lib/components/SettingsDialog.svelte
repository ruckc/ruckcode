<script lang="ts">
  import { Button, Tabs } from '@ruckc/truss-ui';
  import type { SetupApi, SetupState, SetupTarget } from '../host/setup';
  import type { SettingsTab } from '../state/wizard.svelte';
  import KeysTab from './KeysTab.svelte';
  import type { UpdateController } from '../state/updates.svelte';
  import ProjectTab from './ProjectTab.svelte';
  import UpdatesTab from './UpdatesTab.svelte';
  import ProvidersTab from './ProvidersTab.svelte';
  import ModesView from './ModesView.svelte';
  import Modal from './Modal.svelte';
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
</script>

<svelte:window
  onbeforeunload={(e) => {
    if (modesDirty) e.preventDefault();
  }}
/>

<Modal title="Settings" size="xl" onclose={close}>
  <p class="muted proj">
    {target.project}{target.distro ? ` (WSL: ${target.distro})` : ''}
  </p>
  <Tabs
    label="Settings"
    tabs={TABS}
    bind:value={() => tab, (t) => select(t as SettingsTab)}
    {panel}
  />
</Modal>

{#snippet panel()}
  <div class="panel">
    {#if tab === 'updates' && updates}
      <UpdatesTab {updates} />
    {:else if loadError}
      <p class="err" role="alert">{loadError}</p>
      <Button variant="outlined" onclick={() => load()}>Retry</Button>
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
{/snippet}

{#if pending}
  <UnsavedDialog onkeep={() => (pending = null)} ondiscard={discard} />
{/if}

<style>
  .proj {
    margin: 0 0 var(--tr-space-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--tr-text-sm);
  }
  .panel {
    min-height: 200px;
    padding: var(--tr-space-3) 2px 2px;
  }
  .err {
    color: var(--tr-danger);
  }
</style>
