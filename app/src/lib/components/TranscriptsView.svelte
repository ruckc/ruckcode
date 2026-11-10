<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import { Button, Callout, EmptyState, Input, Select, Tabs, Tag } from '@ruckc/truss-ui';
  import { formatBytes, formatDuration, formatTokens, relativeTime } from '../format';
  import type { TranscriptsApi, TranscriptTarget } from '../host/transcripts';
  import { TranscriptsController } from '../state/transcripts.svelte';
  import { SORT_LABELS, type SortKey } from '../transcripts/list';
  import { typeLabel } from '../transcripts/timeline';
  import MetricsPanel from './MetricsPanel.svelte';
  import TimelineItemView from './TimelineItemView.svelte';

  let {
    api,
    target,
    onresume,
    debounceMs
  }: {
    api: TranscriptsApi;
    target: TranscriptTarget;
    /** Open the transcript's session in chat. */
    onresume: (id: string) => void;
    debounceMs?: number;
  } = $props();

  // The view is re-created per project, so the initial props are what the controller needs.
  // svelte-ignore state_referenced_locally
  const ctl = new TranscriptsController(api, target, { debounceMs });
  let tab = $state<'timeline' | 'metrics'>('timeline');
  let copyState = $state<'' | 'busy' | 'done' | 'failed'>('');
  let scroller: HTMLElement | undefined = $state();
  let sentinel: HTMLElement | undefined = $state();

  onMount(() => void ctl.refresh());
  onDestroy(() => ctl.dispose());

  const searching = $derived(ctl.searchText.trim() !== '');
  const titleOf = (id: string): string => {
    const s = ctl.items.find((i) => i.id === id);
    return s?.firstPrompt ? s.firstPrompt : id;
  };

  async function openSession(id: string, jumpTo?: number) {
    tab = 'timeline';
    await ctl.open(id, jumpTo);
  }

  $effect(() => {
    // Open the Metrics tab: make sure everything is loaded (it can be a lot of records).
    if (tab === 'metrics' && ctl.detail && !ctl.complete && !ctl.loadingMore) void ctl.loadAll();
  });

  // Scroll to a record after a search jump (or any focus request).
  $effect(() => {
    void ctl.focusSeq;
    const key = ctl.focusKey;
    if (!key || !scroller || tab !== 'timeline') return;
    void tick().then(() => {
      const el = scroller?.querySelector<HTMLElement>(`[data-key="${key}"]`);
      const reduce =
        typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;
      el?.scrollIntoView?.({ block: 'center', behavior: reduce ? 'auto' : 'smooth' });
    });
  });

  // Load the next page when the end of the loaded records scrolls into view.
  $effect(() => {
    if (!sentinel || !scroller || ctl.complete || typeof IntersectionObserver === 'undefined') {
      return;
    }
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) void ctl.loadMore();
      },
      { root: scroller, rootMargin: '300px' }
    );
    io.observe(sentinel);
    return () => io.disconnect();
  });

  async function copyMarkdown() {
    copyState = 'busy';
    try {
      await navigator.clipboard.writeText(await ctl.markdown());
      copyState = 'done';
    } catch {
      copyState = 'failed';
    }
    setTimeout(() => (copyState = ''), 2500);
  }

  const sortOptions = (Object.keys(SORT_LABELS) as SortKey[]).map((k) => ({
    value: k,
    label: SORT_LABELS[k]
  }));
  const modeOptions = $derived([
    { value: '', label: 'All modes' },
    ...ctl.modes.map((m) => ({ value: m, label: m }))
  ]);
  const modelOptions = $derived([
    { value: '', label: 'All models' },
    ...ctl.models.map((m) => ({ value: m, label: m }))
  ]);
  const tabs = [
    { id: 'timeline', label: 'Timeline' },
    { id: 'metrics', label: 'Metrics' }
  ];
  const copyLabel = $derived(
    copyState === 'busy'
      ? 'Preparing...'
      : copyState === 'done'
        ? 'Copied'
        : copyState === 'failed'
          ? 'Copy failed'
          : 'Copy as markdown'
  );
</script>

<div class="tv" class:has-detail={ctl.selectedId !== null}>
  <aside class="list" aria-label="Transcript list">
    <div class="lhead">
      <h2>Transcripts</h2>
      <span class="muted count" data-testid="count">{ctl.visible.length} of {ctl.items.length}</span
      >
      <Button
        variant="outlined"
        intent="neutral"
        size="sm"
        icon="refresh"
        onclick={() => ctl.refresh()}
        disabled={ctl.loading}>Refresh</Button
      >
    </div>
    <Input
      type="search"
      size="sm"
      icon="search"
      placeholder="Search prompts, replies, tools, output"
      aria-label="Search transcripts"
      value={ctl.searchText}
      oninput={(e) => ctl.setSearch(e.currentTarget.value)}
    />
    <div class="controls">
      <Select
        size="sm"
        searchable={false}
        ariaLabel="Sort by"
        options={sortOptions}
        value={ctl.sort}
        onchange={(v) => (ctl.sort = v)}
      />
      <Button
        variant="outlined"
        intent="neutral"
        size="sm"
        icon={ctl.asc ? 'arrow-up' : 'arrow-down'}
        aria-label={ctl.asc ? 'Sort ascending' : 'Sort descending'}
        title={ctl.asc ? 'Ascending' : 'Descending'}
        onclick={() => (ctl.asc = !ctl.asc)}
      />
      <Select
        size="sm"
        searchable={false}
        ariaLabel="Filter by mode"
        options={modeOptions}
        value={ctl.mode}
        onchange={(v) => (ctl.mode = v)}
      />
      <Select
        size="sm"
        ariaLabel="Filter by model"
        options={modelOptions}
        value={ctl.model}
        onchange={(v) => (ctl.model = v)}
      />
    </div>

    {#if ctl.error}
      <Callout intent="danger">{ctl.error}</Callout>
    {/if}

    {#if searching}
      <section class="results" aria-label="Search results" aria-busy={ctl.searching}>
        {#if ctl.searchError}
          <Callout intent="danger">{ctl.searchError}</Callout>
        {:else if ctl.searching}
          <p class="muted">Searching...</p>
        {:else if ctl.hits.length === 0}
          <p class="muted">No matches.</p>
        {:else}
          <p class="muted" data-testid="hit-count">
            {ctl.hits.length}{ctl.hits.length >= 100 ? '+' : ''} matches
          </p>
          <ul>
            {#each ctl.hits as h, i (`${h.id}:${h.recordIndex}:${i}`)}
              <li>
                <button
                  type="button"
                  class="hit"
                  data-testid="hit"
                  onclick={() => openSession(h.id, h.recordIndex)}
                >
                  <span class="hitmeta"
                    ><Tag size="sm" class="chip">{typeLabel(h.type)}</Tag> #{h.recordIndex} in
                    <em>{titleOf(h.id)}</em></span
                  >
                  <span class="snippet">{h.snippet}</span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {:else if ctl.loading && ctl.items.length === 0}
      <p class="muted pad">Loading transcripts...</p>
    {:else if ctl.items.length === 0 && !ctl.error}
      <p class="muted pad">No transcripts in this project yet.</p>
    {:else}
      <ul class="sessions" aria-label="Transcripts">
        {#each ctl.visible as s (s.id)}
          <li>
            <button
              type="button"
              class="session"
              data-testid="session"
              aria-current={ctl.selectedId === s.id ? 'true' : undefined}
              onclick={() => openSession(s.id)}
            >
              <span class="prompt">{s.firstPrompt || '(no prompt)'}</span>
              <span class="meta muted">
                <Tag size="sm" class="chip">{s.mode}</Tag>
                <span>{s.model}</span>
                <span title={s.updatedAt}
                  >{relativeTime(s.updatedAt) || s.updatedAt.slice(0, 10)}</span
                >
              </span>
              <span class="meta muted">
                <span>{formatTokens(s.inputTokens + s.outputTokens)} tokens</span>
                <span>{formatDuration(s.durationMs)}</span>
                <span>{s.turns} turns</span>
                <span>{s.toolCalls} tools</span>
                {#if s.skippedLines > 0}
                  <Tag size="sm" intent="warning" title="Unreadable lines were skipped"
                    >{s.skippedLines} bad line{s.skippedLines === 1 ? '' : 's'}</Tag
                  >
                {/if}
              </span>
            </button>
          </li>
        {:else}
          <li class="muted pad">No transcripts match the filters.</li>
        {/each}
      </ul>
    {/if}
  </aside>

  <section class="detail" aria-label="Transcript detail">
    {#if ctl.selectedId === null}
      <div class="empty">
        <EmptyState
          icon="file"
          title="No transcript selected"
          description="Select a transcript to see its timeline and metrics."
        />
      </div>
    {:else if ctl.detailLoading}
      <p class="muted pad">Loading transcript...</p>
    {:else if ctl.detailError && !ctl.detail}
      <div class="pad"><Callout intent="danger">{ctl.detailError}</Callout></div>
    {:else if ctl.detail}
      {@const d = ctl.detail}
      <header class="dhead">
        <Button
          class="back"
          variant="outlined"
          intent="neutral"
          size="sm"
          icon="arrow-left"
          onclick={() => ctl.close()}>Back to list</Button
        >
        <div class="titles">
          <h2 title={d.meta.firstPrompt}>{d.meta.firstPrompt || d.meta.id}</h2>
          <p class="muted ids">
            <code>{d.meta.id}</code> · {d.meta.mode} · {d.meta.model} · {d.meta.turns} turns · {d
              .meta.toolCalls}
            tool calls · {formatTokens(d.meta.inputTokens)} in / {formatTokens(d.meta.outputTokens)} out
            ·
            {formatDuration(d.meta.durationMs)} · {formatBytes(d.meta.sizeBytes)}
          </p>
        </div>
        <div class="actions">
          <Button onclick={() => onresume(d.meta.id)}>Resume session</Button>
          <Button
            variant="outlined"
            intent="neutral"
            icon="copy"
            onclick={copyMarkdown}
            disabled={copyState === 'busy'}>{copyLabel}</Button
          >
        </div>
      </header>
      {#if d.skippedLines > 0}
        <div class="banner" data-testid="skipped">
          <Callout intent="warning">
            {d.skippedLines} unreadable line{d.skippedLines === 1 ? ' was' : 's were'} skipped (a truncated
            or corrupt part of the file). The remaining records are shown.
          </Callout>
        </div>
      {/if}
      <div class="tabs">
        <Tabs
          label="Transcript views"
          {tabs}
          value={tab}
          onchange={(v) => (tab = v as 'timeline' | 'metrics')}
          panel={false}
          controls={(id) => (id === tab ? 'transcript-panel' : undefined)}
          tabId={(id) => `transcript-tab-${id}`}
        />
        <span class="muted loaded" data-testid="loaded"
          >{d.records.length.toLocaleString()} of {d.total.toLocaleString()} records loaded</span
        >
      </div>
      {#if ctl.detailError}
        <div class="pad"><Callout intent="danger">{ctl.detailError}</Callout></div>
      {/if}
      <div
        class="scroll"
        bind:this={scroller}
        role="tabpanel"
        id="transcript-panel"
        aria-labelledby={`transcript-tab-${tab}`}
        tabindex="-1"
      >
        {#if tab === 'timeline'}
          <ol class="timeline" aria-label="Timeline">
            {#each ctl.timeline as item (item.key)}
              <TimelineItemView
                {item}
                focused={ctl.focusKey === item.key}
                complete={ctl.complete}
              />
            {/each}
          </ol>
          {#if !ctl.complete}
            <div class="more" bind:this={sentinel}>
              <Button
                variant="outlined"
                intent="neutral"
                disabled={ctl.loadingMore}
                onclick={() => ctl.loadMore()}
              >
                {ctl.loadingMore
                  ? 'Loading...'
                  : `Load more (${ctl.remaining.toLocaleString()} remaining)`}
              </Button>
            </div>
          {:else}
            <p class="muted end">End of transcript</p>
          {/if}
        {:else}
          <MetricsPanel
            records={d.records}
            total={d.total}
            loading={ctl.loadingMore}
            onloadall={() => ctl.loadAll()}
          />
        {/if}
      </div>
    {/if}
  </section>
</div>

<style>
  .tv {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(280px, 360px) minmax(0, 1fr);
  }
  .list {
    border-right: 1px solid var(--tr-outline-variant);
    background: var(--tr-surface-container-low);
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
    padding: var(--tr-space-2);
    gap: var(--tr-space-2);
  }
  .lhead {
    display: flex;
    align-items: center;
    gap: var(--tr-space-2);
  }
  h2 {
    margin: 0;
    font-size: var(--tr-text-lg);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    margin-left: auto;
    font-size: var(--tr-text-sm);
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    gap: var(--tr-space-2);
    align-items: center;
  }
  .controls :global(.tr-field) {
    flex: 1 1 110px;
    min-width: 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--tr-space-2);
  }
  .session,
  .hit {
    display: grid;
    gap: 2px;
    width: 100%;
    text-align: left;
    font: inherit;
    color: var(--tr-on-surface);
    padding: var(--tr-space-2);
    background: var(--tr-surface);
    border: 1px solid var(--tr-outline-variant);
    border-radius: var(--tr-radius-md);
    cursor: pointer;
  }
  .session:hover,
  .hit:hover {
    background: var(--tr-surface-container);
  }
  .session:focus-visible,
  .hit:focus-visible {
    outline: 2px solid var(--tr-primary);
    outline-offset: 1px;
  }
  .session[aria-current='true'] {
    border-color: var(--tr-primary);
    background: var(--tr-surface-container);
  }
  .prompt {
    font-weight: 600;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px var(--tr-space-3);
    font-size: var(--tr-text-sm);
  }
  .hitmeta {
    font-size: var(--tr-text-sm);
    color: var(--tr-on-surface-variant);
  }
  .snippet {
    overflow-wrap: anywhere;
  }
  .pad {
    padding: var(--tr-space-2);
  }
  .detail {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .empty {
    flex: 1;
    display: grid;
    place-items: center;
  }
  .dhead {
    display: flex;
    gap: var(--tr-space-3);
    align-items: flex-start;
    padding: var(--tr-space-2) var(--tr-space-3);
    border-bottom: 1px solid var(--tr-outline-variant);
    flex-wrap: wrap;
  }
  .dhead :global(button.back) {
    display: none;
  }
  .titles {
    flex: 1;
    min-width: 200px;
  }
  .ids {
    margin: 2px 0 0;
    font-size: var(--tr-text-sm);
  }
  .actions {
    display: flex;
    gap: var(--tr-space-2);
  }
  .banner {
    padding: var(--tr-space-2) var(--tr-space-3) 0;
  }
  .tabs {
    display: flex;
    gap: var(--tr-space-1);
    align-items: center;
    padding: 0 var(--tr-space-3);
    border-bottom: 1px solid var(--tr-outline-variant);
  }
  .loaded {
    margin-left: auto;
    font-size: var(--tr-text-sm);
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--tr-space-2) var(--tr-space-3);
  }
  .timeline {
    list-style: none;
    margin: 0;
    padding: 0;
    display: block;
  }
  .more,
  .end {
    text-align: center;
    padding: var(--tr-space-3);
    margin: 0;
  }
  @media (max-width: 900px) {
    .tv {
      grid-template-columns: minmax(0, 1fr);
    }
    .tv.has-detail .list {
      display: none;
    }
    .tv:not(.has-detail) .detail {
      display: none;
    }
    .dhead :global(button.back) {
      display: inline-flex;
    }
  }
</style>
