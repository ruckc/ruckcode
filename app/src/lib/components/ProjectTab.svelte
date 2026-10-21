<script lang="ts">
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
  {#if error}<p class="errbox" role="alert">{error}</p>{/if}
  {#if notice}<p class="ok" role="status">{notice}</p>{/if}

  {#if modes.status === 'missing'}
    <div class="box">
      <strong>No modes.toml</strong>
      <p class="muted">
        <code>.ruckcode/modes.toml</code> defines the agent's modes (tools, permissions, hooks).
        Create it from the built-in templates and this project's mise tasks, like
        <code>rc init</code>.
      </p>
      <label class="choice"
        ><input type="checkbox" bind:checked={minimal} /> Minimal (plan, build, verify)</label
      >
      <div class="actions">
        <button type="button" class="primary" disabled={busy} onclick={() => init(false)}>
          {busy ? 'Creating...' : 'Create modes.toml'}
        </button>
      </div>
    </div>
  {:else if modes.status === 'broken'}
    <p class="errbox" role="alert">modes.toml cannot be read: {modes.error}</p>
    <div class="actions">
      <button type="button" class="danger" disabled={busy} onclick={() => init(true)}
        >Regenerate (overwrite)</button
      >
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
          {#if m.model}<span class="tag">{m.profile ? `${m.profile}: ` : ''}{m.model}</span>{/if}
        </li>
      {/each}
    </ul>
    <p class="muted">Use the Providers &amp; models tab to assign models to modes.</p>
  {/if}
</div>

<style>
  .project {
    display: grid;
    gap: 8px;
  }
  .box {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
  }
  .box p {
    margin: 4px 0;
  }
  .list {
    margin: 0;
    padding-left: 18px;
  }
  .tag {
    font-size: 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0 6px;
  }
  .errbox {
    color: var(--err);
    margin: 0;
  }
  .ok {
    color: var(--ok);
    margin: 0;
  }
</style>
