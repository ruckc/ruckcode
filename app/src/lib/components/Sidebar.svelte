<script lang="ts">
  import { Badge, Button, Input } from '@ruckc/truss-ui';
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
    onbrowse: () => void;
    onforget: (project: Project) => void;
    onnew: () => void;
    onopen: (id: string) => void;
    onclose: (id: string) => void;
  } = $props();

  let picking = $state(false);
  const showPicker = $derived(picking || !connected);
  /** Choosing a new project: the host/folder form is rolled up otherwise (recent projects stay). */
  let choosing = $state(false);
  const showForm = $derived(choosing || !recent.length);
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
        <Button
          size="sm"
          variant="outlined"
          aria-expanded={picking}
          onclick={() => (picking = !picking)}>Switch project</Button
        >
      {/if}
    </div>
    {#if current && connected}
      <div class="prow">
        <div class="ppath muted" title={current.path}>
          {#if hostLabel(current)}<Badge intent="neutral">{hostLabel(current)}</Badge>{/if}
          {current.path}
        </div>
      </div>
    {/if}
    {#if showPicker && !showForm}
      <Button onclick={() => (choosing = true)}>Open a project…</Button>
    {/if}
    {#if showPicker && showForm}
      <form
        class="picker"
        onsubmit={(e) => {
          e.preventDefault();
          picking = false;
          choosing = false;
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
        <Input
          placeholder={host === 'wsl' ? 'Project folder (Windows or Linux path)' : 'Project folder'}
          aria-label="Project folder"
          bind:value={cwd}
        />
        <div class="row">
          {#if canBrowse}<Button type="button" variant="outlined" onclick={onbrowse}>
              Browse…
            </Button>{/if}
          {#if recent.length}
            <Button type="button" variant="outlined" onclick={() => (choosing = false)}>
              Cancel
            </Button>
          {/if}
          <Button type="submit" disabled={!cwd.trim() || connecting || noHost || !hostReady}>
            {connecting ? 'Connecting…' : connected ? 'Open' : 'Connect'}
          </Button>
        </div>
      </form>
    {/if}
    {#if showPicker}
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
                  choosing = false;
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
              <Button
                variant="minimal"
                intent="neutral"
                size="sm"
                icon="cross"
                aria-label={`Forget ${r.path}`}
                onclick={() => onforget(r)}
              />
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  </section>
  {#if connected}
    <section class="sessions">
      <div class="shead">
        <h2>Sessions</h2>
        <Button size="sm" onclick={onnew}>New session</Button>
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
              <Button
                variant="minimal"
                intent="neutral"
                size="sm"
                icon="cross"
                aria-label={`Close session ${s.title}`}
                title="Close session"
                onclick={() => onclose(s.id)}
              />
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
    min-width: 0;
  }
  section {
    padding: var(--tr-space-3) var(--tr-space-4);
    min-width: 0;
  }
  .project {
    border-bottom: 1px solid var(--tr-outline-variant);
    display: grid;
    gap: var(--tr-space-2);
  }
  .phead,
  .shead,
  .prow {
    display: flex;
    gap: var(--tr-space-2);
    align-items: center;
    justify-content: space-between;
  }
  .pname {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ppath {
    flex: 1;
    min-width: 0;
    font-size: var(--tr-text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .picker {
    display: grid;
    gap: var(--tr-space-2);
  }
  .row {
    display: flex;
    gap: var(--tr-space-2);
  }
  h2,
  h3 {
    margin: 0;
    font-size: var(--tr-text-sm);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--tr-on-surface-variant);
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
  .link,
  .item {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    border-radius: var(--tr-radius-sm);
  }
  .link {
    padding: 3px 0;
  }
  .item {
    padding: 5px var(--tr-space-2);
  }
  .link:hover:not(:disabled),
  .item:hover {
    background: var(--tr-surface-container-high);
  }
  .link .muted {
    font-size: var(--tr-text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sessions {
    flex: 1;
    min-height: 0;
  }
  .shead {
    margin-bottom: var(--tr-space-2);
  }
  .sessions li {
    border-radius: var(--tr-radius-md);
    border: 1px solid transparent;
  }
  .sessions li.active {
    background: var(--tr-surface-container);
    border-color: var(--tr-outline-variant);
  }
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    font-size: var(--tr-text-xs);
    display: flex;
    gap: var(--tr-space-1);
    align-items: center;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--tr-success);
  }
  .empty {
    padding: var(--tr-space-2);
  }
</style>
