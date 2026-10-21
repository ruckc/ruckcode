<script lang="ts">
  import {
    baseName,
    hostLabel,
    projectKey,
    type Project,
    type ProjectHost
  } from '../state/projects';
  import HostPicker from './HostPicker.svelte';
  import { relativeTime } from '../format';
  import type { SessionSummary } from '../state/chat.svelte';

  let {
    cwd = $bindable(),
    host = $bindable('local'),
    distro = $bindable(''),
    rcPath = $bindable(''),
    platform = 'linux',
    current = null,
    connected,
    connecting,
    canBrowse,
    noHost,
    recent,
    sessions,
    activeId,
    onconnect,
    ondisconnect,
    onbrowse,
    onforget,
    onnew,
    onopen,
    onclose
  }: {
    cwd: string;
    host?: ProjectHost;
    distro?: string;
    rcPath?: string;
    platform?: string;
    /** The project the open connection belongs to. */
    current?: Project | null;
    connected: boolean;
    connecting: boolean;
    canBrowse: boolean;
    noHost: boolean;
    recent: Project[];
    sessions: (SessionSummary & { open: boolean })[];
    activeId: string | null;
    onconnect: (project: Project) => void;
    ondisconnect: () => void;
    onbrowse: () => void;
    onforget: (project: Project) => void;
    onnew: () => void;
    onopen: (id: string) => void;
    onclose: (id: string) => void;
  } = $props();

  let picking = $state(false);
  const showPicker = $derived(picking || !connected);
  let hostReady = $state(true);
  const here = $derived<Project>({
    path: cwd,
    host,
    ...(host === 'wsl' ? { distro } : {}),
    ...(host === 'subprocess' ? { rcPath } : {})
  });
</script>

<nav class="sidebar" aria-label="Projects and sessions">
  <section class="project">
    <div class="phead">
      <strong class="pname" title={current?.path ?? cwd}
        >{(current?.path ?? cwd) ? baseName(current?.path ?? cwd) : 'No project'}</strong
      >
      {#if connected}
        <button
          type="button"
          class="small"
          aria-expanded={picking}
          onclick={() => (picking = !picking)}>Change</button
        >
      {/if}
    </div>
    {#if current && connected}
      <div class="ppath muted" title={current.path}>
        {#if hostLabel(current)}<span class="badge">{hostLabel(current)}</span>{/if}
        {current.path}
      </div>
    {/if}
    {#if showPicker}
      <form
        class="picker"
        onsubmit={(e) => {
          e.preventDefault();
          picking = false;
          onconnect(here);
        }}
      >
        <HostPicker
          bind:host
          bind:distro
          bind:rcPath
          onready={(r) => (hostReady = r)}
          {cwd}
          {platform}
          disabled={connecting}
        />
        <input
          type="text"
          placeholder={host === 'wsl' ? 'Project folder (Windows or Linux path)' : 'Project folder'}
          aria-label="Project folder"
          bind:value={cwd}
        />
        <div class="row">
          {#if canBrowse}<button type="button" onclick={onbrowse}>Browse…</button>{/if}
          <button
            class="primary"
            type="submit"
            disabled={!cwd.trim() || connecting || noHost || !hostReady}
          >
            {connecting ? 'Connecting…' : connected ? 'Open' : 'Connect'}
          </button>
        </div>
        {#if recent.length}
          <h3>Recent projects</h3>
          <ul class="recent">
            {#each recent as r (projectKey(r))}
              <li>
                <button
                  type="button"
                  class="link"
                  title={r.path}
                  disabled={connecting || noHost}
                  onclick={() => {
                    picking = false;
                    cwd = r.path;
                    host = r.host;
                    distro = r.distro ?? '';
                    rcPath = r.rcPath ?? '';
                    onconnect(r);
                  }}
                  >{baseName(r.path)}
                  <span class="muted">{hostLabel(r) ? `${hostLabel(r)} · ` : ''}{r.path}</span
                  ></button
                >
                <button
                  type="button"
                  class="x"
                  aria-label={`Forget ${r.path}`}
                  onclick={() => onforget(r)}>&times;</button
                >
              </li>
            {/each}
          </ul>
        {/if}
      </form>
    {/if}
    {#if connected}
      <button type="button" class="small disc" onclick={ondisconnect}>Disconnect</button>
    {/if}
  </section>

  {#if connected}
    <section class="sessions">
      <div class="shead">
        <h2>Sessions</h2>
        <button type="button" class="small primary" onclick={onnew}>New session</button>
      </div>
      <ul aria-label="Sessions">
        {#each sessions as s (s.id)}
          <li class:active={s.id === activeId} data-open={s.open}>
            <button
              type="button"
              class="item"
              aria-current={s.id === activeId ? 'true' : undefined}
              onclick={() => onopen(s.id)}
            >
              <span class="title">{s.title}</span>
              <span class="sub muted">
                {#if s.open}<span class="dot" title="open"></span>{/if}
                {relativeTime(s.updatedAt) || 'now'}{s.mode ? ` · ${s.mode}` : ''}
              </span>
            </button>
            {#if s.open}
              <button
                type="button"
                class="x"
                aria-label={`Close session ${s.title}`}
                title="Close session"
                onclick={() => onclose(s.id)}>&times;</button
              >
            {/if}
          </li>
        {:else}
          <li class="muted empty">No sessions yet.</li>
        {/each}
      </ul>
    </section>
  {/if}
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--surface);
    border-right: 1px solid var(--border);
    overflow-y: auto;
  }
  section {
    padding: 10px 12px;
  }
  .project {
    border-bottom: 1px solid var(--border);
    display: grid;
    gap: 6px;
  }
  .phead,
  .shead {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: space-between;
  }
  .pname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ppath {
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 4px;
    margin-right: 4px;
  }
  .picker {
    display: grid;
    gap: 6px;
  }
  .picker input {
    width: 100%;
  }
  .row {
    display: flex;
    gap: 6px;
  }
  h2,
  h3 {
    margin: 0;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
  }
  .small {
    padding: 2px 8px;
    font-size: 12px;
  }
  .disc {
    justify-self: start;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .recent li,
  .sessions li {
    display: flex;
    align-items: center;
  }
  .link {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    text-align: left;
    padding: 3px 0;
    display: flex;
    flex-direction: column;
  }
  .link .muted {
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .x {
    border: 0;
    background: none;
    padding: 0 6px;
    font-size: 16px;
    line-height: 1;
    color: var(--muted);
  }
  .sessions {
    flex: 1;
    min-height: 0;
  }
  .shead {
    margin-bottom: 6px;
  }
  .sessions li {
    border-radius: 6px;
    border: 1px solid transparent;
  }
  .sessions li.active {
    background: var(--code-bg);
    border-color: var(--border);
  }
  .item {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    text-align: left;
    padding: 5px 8px;
    display: flex;
    flex-direction: column;
  }
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    font-size: 11px;
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ok);
  }
  .empty {
    padding: 6px 8px;
  }
</style>
