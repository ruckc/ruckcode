<script lang="ts">
  import { Button, Callout, Checkbox, Tag } from '@ruckc/truss-ui';
  import type { SetupApi, SetupState, SetupTarget } from '../host/setup';

  let {
    api,
    target,
    info,
    onchange
  }: {
    api: SetupApi;
    target: SetupTarget;
    info: SetupState;
    onchange: (s: SetupState) => void;
  } = $props();

  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let minimal = $state(false);

  async function init(force: boolean) {
    busy = true;
    error = null;
    notice = null;
    try {
      const r = await api.initModes(target, { minimal, force });
      notice = `Wrote ${r.path} (${r.tasksDiscovered} mise task${r.tasksDiscovered === 1 ? '' : 's'} discovered).`;
      onchange(await api.state(target));
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  const modes = $derived(info.modes);
</script>

<div class="project">
  <p>
    Project: <code>{target.project}</code>
    {#if target.distro}<span class="muted">(WSL: {target.distro})</span>{/if}
  </p>
  {#if error}<Callout intent="danger">{error}</Callout>{/if}
  {#if notice}<Callout intent="success" role="status">{notice}</Callout>{/if}
  {#if modes.status === 'missing'}
    <div class="box">
      <strong>No modes.toml</strong>
      <p class="muted">
        <code>.ruckcode/modes.toml</code> defines the agent's modes (tools, permissions, hooks).
        Create it from the built-in templates and this project's mise tasks, like
        <code>rc init</code>.
      </p>
      <Checkbox label="Minimal (plan, build, verify)" bind:checked={minimal} />
      <div class="actions">
        <Button disabled={busy} loading={busy} onclick={() => init(false)}>
          {busy ? 'Creating...' : 'Create modes.toml'}
        </Button>
      </div>
    </div>
  {:else if modes.status === 'broken'}
    <Callout intent="danger">modes.toml cannot be read: {modes.error}</Callout>
    <div class="actions">
      <Button intent="danger" variant="outlined" disabled={busy} onclick={() => init(true)}>
        Regenerate (overwrite)
      </Button>
    </div>
  {:else}
    <p>
      <code>.ruckcode/modes.toml</code> has {modes.modes.length} modes (default
      <code>{modes.defaultMode}</code>).
    </p>
    <ul class="list" aria-label="Modes">
      {#each modes.modes as m (m.id)}
        <li>
          <code>{m.id}</code>
          <span class="muted">{m.description}</span>
          {#if m.model}<Tag size="sm">{m.profile ? `${m.profile}: ` : ''}{m.model}</Tag>{/if}
        </li>
      {/each}
    </ul>
    <p class="muted">Use the Providers &amp; models tab to assign models to modes.</p>
  {/if}
</div>

<style>
  .project {
    display: grid;
    gap: var(--tr-space-2);
  }
  .box {
    border: 1px solid var(--tr-outline-variant);
    border-radius: var(--tr-radius-md);
    padding: var(--tr-space-3);
    display: grid;
    gap: var(--tr-space-2);
  }
  .box p {
    margin: 0;
  }
  .actions {
    display: flex;
    gap: var(--tr-space-2);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--tr-space-1);
  }
  .list li {
    display: flex;
    gap: var(--tr-space-2);
    align-items: baseline;
    flex-wrap: wrap;
  }
</style>
