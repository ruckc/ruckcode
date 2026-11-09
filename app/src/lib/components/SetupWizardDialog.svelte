<script lang="ts">
  import { Button, Callout } from '@ruckc/truss-ui';
  import type { SetupApi, SetupTarget } from '../host/setup';
  import { SetupWizard } from '../state/wizard.svelte';
  import Modal from './Modal.svelte';
  import SetupWizardView from './SetupWizardView.svelte';

  let {
    api,
    target,
    firstRun = false,
    onclose,
    onreconnect
  }: {
    api: SetupApi;
    target: SetupTarget;
    /** Opened automatically because there is no usable profile. */
    firstRun?: boolean;
    onclose: () => void;
    onreconnect: () => void;
  } = $props();

  let wizard = $state<SetupWizard | null>(null);
  let loadError = $state<string | null>(null);

  async function load() {
    loadError = null;
    try {
      const [p, s] = await Promise.all([api.presets(), api.state(target)]);
      wizard = new SetupWizard(api, target, p, s);
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
    }
  }
  void load();

  function close() {
    wizard?.clearSecrets();
    onclose();
  }
</script>

<Modal title={firstRun ? 'Set up ruckcode' : 'Add a provider'} size="lg" onclose={close}>
  <p class="muted proj">
    {target.project}{target.distro ? ` (WSL: ${target.distro})` : ''}
  </p>
  {#if firstRun && wizard?.step !== 'done'}
    <Callout intent="primary">
      Welcome! No model provider is configured yet. Add an API key and endpoint to get started.
    </Callout>
  {/if}
  <div class="panel">
    {#if loadError}
      <p class="err" role="alert">{loadError}</p>
      <Button variant="outlined" onclick={() => load()}>Retry</Button>
    {:else if !wizard}
      <p class="muted" role="status">Loading...</p>
    {:else}
      <SetupWizardView {wizard} onclose={close} {onreconnect} />
    {/if}
  </div>
</Modal>

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
    padding: var(--tr-space-2) 2px 2px;
  }
  .err {
    color: var(--tr-danger);
  }
</style>
