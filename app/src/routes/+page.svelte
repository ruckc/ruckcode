<script lang="ts">
  import { onMount } from 'svelte';
  import AskDialog from '../lib/components/AskDialog.svelte';
  import ChatView from '../lib/components/ChatView.svelte';
  import Composer from '../lib/components/Composer.svelte';
  import PermissionDialog from '../lib/components/PermissionDialog.svelte';
  import PlanPanel from '../lib/components/PlanPanel.svelte';
  import Sidebar from '../lib/components/Sidebar.svelte';
  import TopBar from '../lib/components/TopBar.svelte';
  import { browseFolder, getPlatform, getWsl, hostMode } from '../lib/host';
  import { ChatController } from '../lib/state/chat.svelte';
  import {
    loadRecent,
    pushRecent,
    removeRecent,
    saveRecent,
    type Project,
    type ProjectHost
  } from '../lib/state/projects';

  const chat = new ChatController();
  const mode = hostMode();

  let recent = $state<Project[]>([]);
  let cwd = $state('');
  let host = $state<ProjectHost>('local');
  let distro = $state('');
  let rcPath = $state('');
  let platform = $state('linux');
  let current = $state<Project | null>(null);
  let sidebarOpen = $state(true);
  let planOpen = $state(true);
  let composer: { focus(): void } | undefined = $state();

  onMount(() => {
    void getPlatform().then((p) => {
      platform = p;
      // A remembered WSL project only makes sense on Windows.
      if (p !== 'windows') recent = recent.filter((r) => r.host !== 'wsl');
      if (p !== 'windows' && host === 'wsl') host = 'local';
    });
    recent = loadRecent();
    const last = recent[0];
    if (last) {
      cwd = last.path;
      host = last.host;
      distro = last.distro ?? '';
      rcPath = last.rcPath ?? '';
    } else if (mode === 'mock') {
      cwd = '/demo/project';
    }
    if (window.innerWidth < 900) {
      sidebarOpen = false;
      planOpen = false;
    }
  });

  const active = $derived(chat.active);
  const dialogOpen = $derived(!!chat.permission || !!chat.question);

  async function connectProject(project: Project) {
    const path = project.path.trim();
    if (!path) return;
    let p: Project = { ...project, path };
    cwd = path;
    host = p.host;
    // WSL: remember the Linux path so reopening it needs no translation, and fail early with a
    // clear message for paths in another distro.
    if (p.host === 'wsl' && p.distro) {
      try {
        const wsl = await getWsl();
        p = { ...p, path: await wsl.translatePath(p.distro, path) };
        cwd = p.path;
      } catch (e) {
        chat.error = e instanceof Error ? e.message : String(e);
        return;
      }
    }
    current = p;
    recent = pushRecent(recent, p);
    saveRecent(recent);
    await chat.connect({
      cwd: p.path,
      host: p.host,
      rcPath: p.host === 'subprocess' ? p.rcPath : undefined,
      distro: p.host === 'wsl' ? p.distro : undefined
    });
  }

  async function browse() {
    const dir = await browseFolder({ host, distro }).catch(() => null);
    if (!dir) return;
    cwd = dir;
    if (host === 'wsl' && distro) {
      // Show the Linux path the picked folder maps to (e.g. \\wsl.localhost\Ubuntu\home\x).
      try {
        cwd = await (await getWsl()).translatePath(distro, dir);
      } catch {
        // keep the raw path; the host picker shows why it cannot be used
      }
    }
  }

  function forget(project: Project) {
    recent = removeRecent(recent, project);
    saveRecent(recent);
  }

  // Keep the prompt box focused when the session changes or a dialog closes.
  $effect(() => {
    void chat.activeId;
    if (!dialogOpen && chat.connected) queueMicrotask(() => composer?.focus());
  });

  function onWindowKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && !e.defaultPrevented && active?.running && !dialogOpen) {
      e.preventDefault();
      void chat.cancel();
    }
  }

  const placeholder = $derived(
    !chat.connected
      ? 'Not connected'
      : active?.running
        ? 'The agent is working. Enter queues your message.'
        : 'Message the agent'
  );
</script>

<svelte:window onkeydown={onWindowKey} />

<div class="shell" class:no-sidebar={!sidebarOpen} class:no-plan={!planOpen}>
  {#if sidebarOpen}
    <div class="side">
      <Sidebar
        bind:cwd
        bind:host
        bind:distro
        bind:rcPath
        {platform}
        {current}
        connected={chat.connected}
        connecting={chat.status === 'connecting'}
        canBrowse={mode === 'tauri' || (mode === 'mock' && platform === 'windows')}
        noHost={mode === 'none'}
        {recent}
        sessions={chat.sidebar}
        activeId={chat.activeId}
        onconnect={connectProject}
        ondisconnect={() => chat.disconnect()}
        onbrowse={browse}
        onforget={forget}
        onnew={() => chat.newSession()}
        onopen={(id) => chat.openSession(id)}
        onclose={(id) => chat.closeSession(id)}
      />
    </div>
  {/if}

  <div class="main">
    <TopBar
      model={active?.model ?? null}
      title={chat.sidebar.find((s) => s.id === chat.activeId)?.title ??
        (chat.connected ? 'New session' : 'ruckcode')}
      disabled={!chat.connected}
      bind:planOpen
      bind:sidebarOpen
      onmode={(id) => chat.setMode(id)}
      onconfig={(id, v) => chat.setConfig(id, v)}
    />

    {#if mode === 'none'}
      <p class="banner">
        No agent host available. Run the desktop app (<code>mise run app:dev</code>), or open this
        page with <code>?mock=1</code> for a scripted demo agent.
      </p>
    {/if}
    {#if chat.status === 'lost'}
      <div class="banner error" role="alert">
        <span>{chat.error}</span>
        <button class="primary" onclick={() => chat.reconnect()}>Reconnect</button>
      </div>
    {:else if chat.error}
      <div class="banner error" role="alert">
        <span>{chat.error}</span>
        <button onclick={() => chat.dismissError()}>Dismiss</button>
      </div>
    {/if}

    {#if active}
      <ChatView items={active.model.items} running={active.running} loading={active.loading} />
    {:else}
      <div class="welcome">
        {#if chat.connected}
          <p class="muted">No session is open.</p>
          <button class="primary" onclick={() => chat.newSession()}>New session</button>
        {:else if chat.status === 'lost'}
          <p class="muted">The agent is not running. Reconnect to continue.</p>
        {:else}
          <h1>ruckcode</h1>
          <p class="muted">Choose a project folder and connect to start a session.</p>
        {/if}
      </div>
    {/if}

    <Composer
      bind:this={composer}
      disabled={!chat.connected || !active || active.loading}
      running={active?.running ?? false}
      queue={active?.queue ?? []}
      history={chat.history}
      {placeholder}
      onsend={(t) => chat.send(t)}
      oncancel={() => chat.cancel()}
      onunqueue={(i) => chat.unqueue(i)}
    />
  </div>

  {#if planOpen}
    <div class="planp"><PlanPanel plan={active?.model.plan ?? []} /></div>
  {/if}
</div>

{#if chat.permission}
  {#key chat.permission}
    <PermissionDialog pending={chat.permission} total={chat.permissions.length} />
  {/key}
{:else if chat.question}
  {#key chat.question}
    <AskDialog pending={chat.question} total={chat.questions.length} />
  {/key}
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: 260px minmax(0, 1fr) 280px;
    height: 100vh;
  }
  .shell.no-sidebar {
    grid-template-columns: minmax(0, 1fr) 280px;
  }
  .shell.no-plan {
    grid-template-columns: 260px minmax(0, 1fr);
  }
  .shell.no-sidebar.no-plan {
    grid-template-columns: minmax(0, 1fr);
  }
  .side,
  .planp {
    min-height: 0;
    overflow-y: auto;
  }
  .planp {
    border-left: 1px solid var(--border);
    background: var(--surface);
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .banner {
    margin: 0;
    padding: 6px 12px;
    background: var(--code-bg);
    border-bottom: 1px solid var(--border);
    display: flex;
    gap: 12px;
    align-items: center;
    justify-content: space-between;
  }
  .banner.error {
    color: var(--err);
  }
  .welcome {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }
  .welcome h1 {
    margin: 0;
    font-size: 28px;
  }
  @media (max-width: 900px) {
    .shell,
    .shell.no-sidebar,
    .shell.no-plan,
    .shell.no-sidebar.no-plan {
      grid-template-columns: minmax(0, 1fr);
    }
    .side,
    .planp {
      position: fixed;
      top: 0;
      bottom: 0;
      z-index: 5;
      width: min(300px, 88vw);
      background: var(--surface);
    }
    .side {
      left: 0;
    }
    .planp {
      right: 0;
    }
  }
</style>
