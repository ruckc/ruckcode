<script lang="ts">
  import { focusTrap } from '../a11y';
  import type { SetupApi, SetupTarget } from '../host/setup';
  import { SetupWizard } from '../state/wizard.svelte';
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

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && !e.defaultPrevented) {
      e.preventDefault();
      close();
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="backdrop" role="presentation">
  <div
    class="dialog setup"
    role="dialog"
    aria-modal="true"
    aria-labelledby="setup-title"
    use:focusTrap
  >
    <header>
      <h2 id="setup-title">{firstRun ? 'Set up ruckcode' : 'Add a provider'}</h2>
      <span class="muted proj">
        {target.project}{target.distro ? ` (WSL: ${target.distro})` : ''}
      </span>
      <button type="button" class="ghost" aria-label="Close setup" onclick={close}>&times;</button>
    </header>

    {#if firstRun && wizard?.step !== 'done'}
      <p class="welcome" role="note">
        Welcome! No model provider is configured yet. Add an API key and endpoint to get started.
      </p>
    {/if}

    <div class="panel">
      {#if loadError}
        <p class="err" role="alert">{loadError}</p>
        <button type="button" onclick={() => load()}>Retry</button>
      {:else if !wizard}
        <p class="muted" role="status">Loading...</p>
      {:else}
        <SetupWizardView {wizard} onclose={close} {onreconnect} />
      {/if}
    </div>
  </div>
</div>

<style>
  .setup {
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
    margin: 8px 0;
    padding: 6px 10px;
    border-radius: 6px;
    background: var(--code-bg);
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
