<script lang="ts">
  import { untrack } from 'svelte';
  import { getWsl } from '../host';
  import {
    distroLabel,
    versionMatches,
    type RcStatus,
    type WslApi,
    type WslDistro
  } from '../host/wsl';
  import type { ProjectHost } from '../state/projects';

  let {
    host = $bindable('local'),
    distro = $bindable(''),
    rcPath = $bindable(''),
    onready,
    cwd,
    platform,
    disabled = false
  }: {
    host: ProjectHost;
    distro: string;
    rcPath: string;
    /** Whether the chosen host is usable (WSL: a WSL2 distro with `rc`; custom: a binary path). */
    onready?: (ready: boolean) => void;
    cwd: string;
    platform: string;
    disabled?: boolean;
  } = $props();

  const hosts = $derived<{ id: ProjectHost; label: string }[]>([
    { id: 'local', label: 'Local' },
    ...(platform === 'windows' ? [{ id: 'wsl' as const, label: 'WSL2' }] : []),
    { id: 'subprocess', label: 'Custom rc' }
  ]);

  let api: WslApi | undefined;
  let distros = $state.raw<WslDistro[]>([]);
  let listing = $state(false);
  let listError = $state('');
  let rc = $state.raw<RcStatus | null>(null);
  let checking = $state(false);
  let checkError = $state('');
  /** A manual "Reinstall rc" is running (the automatic install runs inside `checking`). */
  let installing = $state(false);
  let progress = $state.raw<string[]>([]);
  let installError = $state('');
  let linux = $state<{ ok: boolean; text: string } | null>(null);

  const selected = $derived(distros.find((d) => d.name === distro));
  const matches = $derived(rc ? versionMatches(rc) : false);
  /** Progress lines arrive only when `rc` is actually being copied/installed. */
  const provisioning = $derived(installing || (checking && progress.length > 0));
  /** This session installed the bundled copy (so the picker can say so). */
  const viaBundle = $derived(progress.some((l) => l.includes('bundled rc (')));

  $effect(() => {
    onready?.(
      host === 'local' ||
        (host === 'subprocess' && rcPath.trim().length > 0) ||
        (host === 'wsl' && !!selected?.selectable && !!rc?.installed && !installing && !checking)
    );
  });

  async function refresh() {
    listing = true;
    listError = '';
    try {
      api ??= await getWsl();
      const list = await api.listDistros();
      distros = list;
      const current = list.find((d) => d.name === distro && d.selectable);
      if (!current) {
        distro =
          (list.find((d) => d.default && d.selectable) ?? list.find((d) => d.selectable))?.name ??
          '';
      }
      if (!list.length) listError = 'No WSL distributions are installed.';
      else if (!list.some((d) => d.selectable)) listError = 'No WSL2 distribution is available.';
    } catch (e) {
      distros = [];
      listError = e instanceof Error ? e.message : String(e);
    } finally {
      listing = false;
    }
  }

  // Load distros the first time WSL2 is chosen.
  $effect(() => {
    if (host === 'wsl' && untrack(() => !distros.length && !listing)) void refresh();
  });

  // Provision `rc` whenever the distro changes: the backend checks it and, when it is missing or
  // differs from the app's version, installs the copy bundled with the app (no button needed).
  let checkSeq = 0;
  $effect(() => {
    const d = host === 'wsl' ? distro : '';
    if (!d) {
      rc = null;
      return;
    }
    const seq = ++checkSeq;
    untrack(() => {
      rc = null;
      checkError = '';
      installError = '';
      progress = [];
      checking = true;
    });
    void (async () => {
      try {
        api ??= await getWsl();
        const r = await api.ensureRc(d, (line) => {
          if (seq === checkSeq) progress = [...progress, line];
        });
        if (seq === checkSeq) rc = r;
      } catch (e) {
        if (seq !== checkSeq) return;
        const msg = e instanceof Error ? e.message : String(e);
        // Keep whatever is installed usable (a version mismatch only warns).
        try {
          const r = await api!.checkRc(d);
          if (seq !== checkSeq) return;
          rc = r;
          installError = msg;
        } catch {
          if (seq === checkSeq) checkError = msg;
        }
      } finally {
        if (seq === checkSeq) checking = false;
      }
    })();
  });

  // Show the Linux path the project path resolves to (debounced).
  let pathSeq = 0;
  $effect(() => {
    const d = host === 'wsl' ? distro : '';
    const p = cwd.trim();
    if (!d || !p) {
      linux = null;
      return;
    }
    const seq = ++pathSeq;
    const t = setTimeout(async () => {
      try {
        api ??= await getWsl();
        const text = await api.translatePath(d, p);
        if (seq === pathSeq) linux = { ok: true, text };
      } catch (e) {
        if (seq === pathSeq)
          linux = { ok: false, text: e instanceof Error ? e.message : String(e) };
      }
    }, 120);
    return () => clearTimeout(t);
  });

  /** Manual (re)install of the bundled rc, e.g. after a failure or to repair a broken copy. */
  async function reinstall() {
    if (!api || !distro || installing || checking) return;
    installing = true;
    installError = '';
    progress = [];
    const d = distro;
    try {
      const r = await api.installBundledRc(d, (line) => (progress = [...progress, line]));
      if (d === distro) rc = r;
    } catch (e) {
      installError = e instanceof Error ? e.message : String(e);
    } finally {
      installing = false;
    }
  }
</script>

<div class="hostpicker">
  <div class="seg" role="radiogroup" aria-label="Agent host">
    {#each hosts as h (h.id)}
      <button
        type="button"
        role="radio"
        aria-checked={host === h.id}
        class:on={host === h.id}
        {disabled}
        onclick={() => (host = h.id)}>{h.label}</button
      >
    {/each}
  </div>

  {#if host === 'subprocess'}
    <label class="field">
      <span class="muted">Path to the rc binary</span>
      <input type="text" placeholder="/path/to/rc" bind:value={rcPath} {disabled} />
    </label>
  {:else if host === 'wsl'}
    <div class="wsl">
      <div class="row">
        <label class="grow">
          <span class="muted">Distribution</span>
          <select
            aria-label="WSL distro"
            bind:value={distro}
            disabled={disabled || listing || !distros.length}
          >
            {#each distros as d (d.name)}
              <option value={d.name} disabled={!d.selectable} title={d.reason ?? ''}>
                {distroLabel(d)}{d.reason ? ` - ${d.reason}` : ''}
              </option>
            {/each}
          </select>
        </label>
        <button
          type="button"
          class="small"
          disabled={listing}
          aria-label="Refresh distros"
          onclick={refresh}>{listing ? '…' : 'Refresh'}</button
        >
      </div>
      {#if listError}<p class="err" role="alert">{listError}</p>{/if}

      {#if distro}
        <div
          class="rc"
          role="status"
          aria-label="rc status"
          data-state={provisioning
            ? 'installing'
            : checking
              ? 'checking'
              : checkError
                ? 'error'
                : !rc?.installed
                  ? 'missing'
                  : matches
                    ? 'ok'
                    : 'mismatch'}
        >
          {#if provisioning}
            <span class="muted">Installing bundled rc into {distro}…</span>
          {:else if checking}
            <span class="muted">Checking rc in {distro}…</span>
          {:else if checkError}
            <span class="err">{checkError}</span>
          {:else if rc && !rc.installed}
            <span class="warn">rc is not installed in {distro}.</span>
          {:else if rc && matches}
            <span class="ok">rc {rc.version} {viaBundle ? '(bundled)' : 'installed'}</span>
          {:else if rc}
            <span class="warn"
              >rc {rc.version ?? '(unknown version)'} is installed, but this app is {rc.expected}.</span
            >
          {/if}
          {#if !checking && !installing && (rc || checkError)}
            <button type="button" class="small" onclick={reinstall}>
              {rc?.installed ? 'Reinstall rc' : 'Install rc'}
            </button>
          {/if}
        </div>
        {#if progress.length}
          <pre class="log" aria-label="Install progress" aria-live="polite">{progress.join(
              '\n'
            )}</pre>
        {/if}
        {#if installError}<p class="err" role="alert">Installing rc failed: {installError}</p>{/if}
      {/if}
    </div>
  {/if}

  {#if host === 'wsl' && linux}
    <p class="linux" class:err={!linux.ok} aria-label="Linux path" data-ok={linux.ok}>
      {#if linux.ok}<span class="muted">Linux path:</span>
        <code>{linux.text}</code>{:else}{linux.text}{/if}
    </p>
  {/if}
</div>

<style>
  .hostpicker {
    display: grid;
    gap: 6px;
  }
  .seg {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }
  .seg button {
    flex: 1;
    border: 0;
    border-radius: 0;
    padding: 3px 6px;
    font-size: 12px;
    background: var(--surface);
  }
  .seg button + button {
    border-left: 1px solid var(--border);
  }
  .seg button.on {
    background: var(--accent);
    color: var(--accent-text);
  }
  .field,
  label {
    display: grid;
    gap: 2px;
    font-size: 12px;
  }
  .row {
    display: flex;
    gap: 6px;
    align-items: end;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  select,
  input {
    width: 100%;
  }
  .wsl {
    display: grid;
    gap: 6px;
  }
  .rc {
    display: flex;
    gap: 6px;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    font-size: 12px;
  }
  .small {
    padding: 2px 8px;
    font-size: 12px;
  }
  .ok {
    color: var(--ok);
  }
  .warn {
    color: var(--warn);
  }
  .err {
    color: var(--err);
    margin: 0;
    font-size: 12px;
  }
  .log {
    margin: 0;
    padding: 6px;
    font-size: 11px;
    background: var(--code-bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 120px;
    overflow: auto;
  }
  .linux {
    margin: 0;
    font-size: 12px;
    word-break: break-all;
  }
</style>
