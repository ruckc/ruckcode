<script lang="ts">
  import { onMount } from 'svelte';
  import AskDialog from '../lib/components/AskDialog.svelte';
  import ChatView from '../lib/components/ChatView.svelte';
  import Composer from '../lib/components/Composer.svelte';
  import PermissionDialog from '../lib/components/PermissionDialog.svelte';
  import PlanPanel from '../lib/components/PlanPanel.svelte';
  import Sidebar from '../lib/components/Sidebar.svelte';
  import SessionsDialog from '../lib/components/SessionsDialog.svelte';
  import TopBar from '../lib/components/TopBar.svelte';
  import {
    browseFolder,
    getModes,
    getPlatform,
    getSetup,
    getTranscripts,
    getUpdates,
    getWsl,
    hostMode
  } from '../lib/host';
  import TranscriptsView from '../lib/components/TranscriptsView.svelte';
  import UpdateBanner from '../lib/components/UpdateBanner.svelte';
  import { UpdateController } from '../lib/state/updates.svelte';
  import type { ModesApi } from '../lib/host/modes';
  import type { TranscriptsApi, TranscriptTarget } from '../lib/host/transcripts';
  import type { SettingsTab } from '../lib/state/wizard.svelte';
  import SettingsDialog from '../lib/components/SettingsDialog.svelte';
  import SetupWizardDialog from '../lib/components/SetupWizardDialog.svelte';
  import type { SetupApi, SetupTarget } from '../lib/host/setup';
  import { ChatController } from '../lib/state/chat.svelte';
  import {
    baseName,
    loadRecent,
    pushRecent,
    removeRecent,
    saveRecent,
    type Project,
    type ProjectHost
  } from '../lib/state/projects';

  const chat = new ChatController();
  const mode = hostMode();
  /** Set once the host reports an updater (desktop app, or `?mock=1&update=1`). */
  let updates = $state<UpdateController | null>(null);

  let recent = $state<Project[]>([]);
  let cwd = $state('');
  let host = $state<ProjectHost>('local');
  let distro = $state('');
  let rcPath = $state('');
  let platform = $state('linux');
  let current = $state<Project | null>(null);
  /** The project & session picker (a dialog: switching sessions is not an everyday action). */
  let sessionsOpen = $state(false);
  let view = $state<'chat' | 'transcripts'>('chat');
  let transcripts = $state<{ api: TranscriptsApi; target: TranscriptTarget } | null>(null);
  let composer: { focus(): void } | undefined = $state();
  let settings = $state<{
    api: SetupApi;
    target: SetupTarget;
    tab: SettingsTab;
    modesApi: ModesApi;
  } | null>(null);
  /** The setup wizard, a dialog of its own (first run, or "Add provider" from Settings). */
  let setup = $state<{ api: SetupApi; target: SetupTarget; firstRun: boolean } | null>(null);
  /** A project whose connection waits for the first-run dialog to close. */
  let pendingConnect: Project | null = null;

  onMount(() => {
    void getUpdates().then((api) => {
      if (!api) return;
      updates = new UpdateController(api, {
        // Never restart under a running agent turn.
        isBusy: () => Object.values(chat.sessions).some((s) => s.running)
      });
      updates.start();
    });
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
    return () => updates?.stop();
  });

  const active = $derived(chat.active);
  /** The plan panel appears only while the agent has a plan. */
  const showPlan = $derived(view === 'chat' && (active?.model.plan.length ?? 0) > 0);
  const dialogOpen = $derived(!!chat.permission || !!chat.question);

  async function connectProject(project: Project, skipOnboarding = false) {
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
    if (!skipOnboarding && (await firstRunCheck(p))) {
      pendingConnect = p;
      return;
    }
    await chat.connect({
      cwd: p.path,
      host: p.host,
      rcPath: p.host === 'subprocess' ? p.rcPath : undefined,
      distro: p.host === 'wsl' ? p.distro : undefined
    });
  }

  const targetOf = (p: { path: string; host: ProjectHost; distro?: string }): SetupTarget => ({
    project: p.path,
    distro: p.host === 'wsl' ? p.distro : undefined
  });

  /** Opens the setup wizard when the project's host has no usable profile. */
  async function firstRunCheck(p: Project): Promise<boolean> {
    if (mode === 'none') return false;
    try {
      const api = await getSetup();
      const target = targetOf(p);
      if (!(await api.state(target)).needsOnboarding) return false;
      setup = { api, target, firstRun: true };
      return true;
    } catch {
      return false; // cannot tell (e.g. no rc yet): just connect
    }
  }

  /** The project the typed/connected path refers to (WSL paths translated to Linux paths). */
  async function resolveProject(): Promise<Project> {
    // The connected project; otherwise whatever is chosen in the picker right now (the last
    // connected project would be stale after switching).
    const live = chat.connected ? current : null;
    const p: Project = live ?? { path: cwd.trim(), host, distro: distro || undefined };
    if (!live && p.host === 'wsl' && p.distro) {
      try {
        return { ...p, path: await (await getWsl()).translatePath(p.distro, p.path) };
      } catch {
        // keep as typed
      }
    }
    return p;
  }

  async function openSettings(tab: SettingsTab = 'providers') {
    const p = await resolveProject();
    const [api, modesApi] = await Promise.all([getSetup(), getModes()]);
    settings = { api, target: targetOf(p), tab, modesApi };
  }

  async function openTranscripts() {
    const p = await resolveProject();
    if (!p.path) {
      chat.error = 'Choose a project folder first.';
      return;
    }
    const target = targetOf(p);
    // A new view per project, so switching projects never shows another project's list.
    if (
      !transcripts ||
      transcripts.target.project !== target.project ||
      transcripts.target.distro !== target.distro
    ) {
      transcripts = { api: await getTranscripts(), target };
    }
    view = 'transcripts';
  }

  /** Open a stored transcript's session in chat (connecting first when needed). */
  async function resumeSession(id: string) {
    view = 'chat';
    if (!chat.connected) {
      const p = await resolveProject();
      await connectProject(p, true);
    }
    if (chat.connected) await chat.openSession(id);
  }

  function addProvider() {
    if (!settings) return;
    setup = { api: settings.api, target: settings.target, firstRun: false };
    settings = null;
  }

  function closeSettings() {
    settings = null;
    setup = null;
    const p = pendingConnect;
    pendingConnect = null;
    if (p) void connectProject(p, true);
  }

  function reconnectAfterSetup() {
    settings = null;
    setup = null;
    const p = pendingConnect;
    pendingConnect = null;
    if (p) void connectProject(p, true);
    else if (chat.status === 'connected' || chat.status === 'lost') void chat.reconnect();
    else if (current) void connectProject(current, true);
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

  // Close the picker once connected; reopen it after a disconnect (nothing else to do then).
  let wasConnected = false;
  $effect(() => {
    const c = chat.connected;
    if (c && !wasConnected) sessionsOpen = false;
    wasConnected = c;
  });

  // Keep the prompt box focused when the session changes or a dialog closes.
  $effect(() => {
    void chat.activeId;
    if (!dialogOpen && !sessionsOpen && chat.connected) queueMicrotask(() => composer?.focus());
  });

  function onWindowKey(e: KeyboardEvent) {
    if (e.defaultPrevented || !active?.running) return;
    // Esc / Ctrl+C always stop a running turn, even with a question or permission dialog
    // open (which cancel() also dismisses). Ctrl+C keeps its copy meaning over a selection.
    const copy = e.key.toLowerCase() === 'c' && (e.ctrlKey || e.metaKey) && !e.shiftKey;
    const selecting = copy && (window.getSelection()?.toString() ?? '') !== '';
    if (e.key === 'Escape' || (e.ctrlKey && copy && !selecting)) {
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

<div class="shell" class:with-plan={showPlan}>
  <div class="main">
    <TopBar
      model={active?.model ?? null}
      title={chat.sidebar.find((s) => s.id === chat.activeId)?.title ??
        (chat.connected ? 'New session' : current ? baseName(current.path) : 'Open a project')}
      project={chat.connected && current ? baseName(current.path) : ''}
      disabled={!chat.connected}
      connected={chat.connected}
      {view}
      onback={() => (view = 'chat')}
      onsessions={() => (sessionsOpen = true)}
      onmode={(id) => chat.setMode(id)}
      onconfig={(id, v) => chat.setConfig(id, v)}
      menu={[
        { label: 'New session', onclick: () => void chat.newSession(), disabled: !chat.connected },
        { label: 'Open project or session…', onclick: () => (sessionsOpen = true) },
        'sep',
        { label: 'Transcripts', onclick: () => void openTranscripts() },
        { label: 'Settings', onclick: () => void openSettings() },
        ...(chat.connected
          ? [
              'sep' as const,
              { label: 'Disconnect', onclick: () => chat.disconnect(), danger: true }
            ]
          : [])
      ]}
    />

    {#if updates}
      <UpdateBanner {updates} openSessions={Object.keys(chat.sessions).length} {platform} />
    {/if}

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

    {#if view === 'transcripts' && transcripts}
      {#key `${transcripts.target.distro ?? ''}\0${transcripts.target.project}`}
        <TranscriptsView
          api={transcripts.api}
          target={transcripts.target}
          onresume={resumeSession}
        />
      {/key}
    {:else if active}
      <ChatView items={active.model.items} running={active.running} loading={active.loading} />
    {:else}
      <div class="welcome">
        {#if chat.connected}
          <p class="muted">No session is open.</p>
          <button class="primary" onclick={() => chat.newSession()}>New session</button>
        {:else if chat.status === 'lost'}
          <p class="muted">The agent is not running. Reconnect to continue.</p>
        {:else}
          <div class="start">{@render picker()}</div>
        {/if}
      </div>
    {/if}

    {#if view === 'chat' && (chat.connected || chat.status === 'lost')}
      <Composer
        bind:this={composer}
        disabled={!chat.connected || !active || active.loading}
        running={active?.running ?? false}
        queue={active?.queue ?? []}
        history={chat.history}
        {placeholder}
        commands={active?.model.commands ?? []}
        onsend={(t) => chat.send(t)}
        oncancel={() => chat.cancel()}
        onunqueue={(i) => chat.unqueue(i)}
      />
    {/if}
  </div>

  {#if showPlan}
    <div class="planp"><PlanPanel plan={active?.model.plan ?? []} /></div>
  {/if}
</div>

{#snippet picker()}
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
    onconnect={(p) => void connectProject(p)}
    onbrowse={browse}
    onforget={forget}
    onnew={() => {
      sessionsOpen = false;
      void chat.newSession();
    }}
    onopen={(id) => {
      sessionsOpen = false;
      void chat.openSession(id);
    }}
    onclose={(id) => chat.closeSession(id)}
  />
{/snippet}

{#if sessionsOpen && chat.connected}
  <SessionsDialog onclose={() => (sessionsOpen = false)}>
    {@render picker()}
  </SessionsDialog>
{/if}

{#if settings}
  <SettingsDialog
    api={settings.api}
    target={settings.target}
    bind:tab={settings.tab}
    modesApi={settings.modesApi}
    canReconnect={chat.connected}
    onreconnect={() => void chat.reconnect()}
    {updates}
    onclose={closeSettings}
    onaddprovider={addProvider}
  />
{/if}

{#if setup}
  <SetupWizardDialog
    api={setup.api}
    target={setup.target}
    firstRun={setup.firstRun}
    onclose={closeSettings}
    onreconnect={reconnectAfterSetup}
  />
{/if}

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
    grid-template-columns: minmax(0, 1fr);
    height: 100vh;
  }
  .shell.with-plan {
    grid-template-columns: minmax(0, 1fr) 280px;
  }
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
  .start {
    width: min(460px, 94vw);
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
  }
  @media (max-width: 900px) {
    .shell.with-plan {
      grid-template-columns: minmax(0, 1fr);
    }
    .planp {
      display: none;
    }
  }
</style>
