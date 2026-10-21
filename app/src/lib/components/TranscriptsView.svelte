<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
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

  const sorts = Object.keys(SORT_LABELS) as SortKey[];
</script>

<div class="tv" class:has-detail={ctl.selectedId !== null}>
  <aside class="list" aria-label="Transcript list">
    <div class="lhead">
      <h2>Transcripts</h2>
      <span class="muted count" data-testid="count">{ctl.visible.length} of {ctl.items.length}</span
      >
      <button type="button" class="small" onclick={() => ctl.refresh()} disabled={ctl.loading}
        >Refresh</button
      >
    </div>
    <input
      type="search"
      class="search"
      placeholder="Search prompts, replies, tools, output"
      aria-label="Search transcripts"
      value={ctl.searchText}
      oninput={(e) => ctl.setSearch(e.currentTarget.value)}
    />
    <div class="controls">
      <label>
        <span class="muted">Sort</span>
        <select aria-label="Sort by" bind:value={ctl.sort}>
          {#each sorts as k (k)}<option value={k}>{SORT_LABELS[k]}</option>{/each}
        </select>
      </label>
      <button
        type="button"
        class="small"
        aria-label={ctl.asc ? 'Sort ascending' : 'Sort descending'}
        title={ctl.asc ? 'Ascending' : 'Descending'}
        onclick={() => (ctl.asc = !ctl.asc)}>{ctl.asc ? '↑' : '↓'}</button
      >
      <label>
        <span class="muted">Mode</span>
        <select aria-label="Filter by mode" bind:value={ctl.mode}>
          <option value="">All</option>
          {#each ctl.modes as m (m)}<option value={m}>{m}</option>{/each}
        </select>
      </label>
      <label>
        <span class="muted">Model</span>
        <select aria-label="Filter by model" bind:value={ctl.model}>
          <option value="">All</option>
          {#each ctl.models as m (m)}<option value={m}>{m}</option>{/each}
        </select>
      </label>
    </div>

    {#if ctl.error}
      <p class="error" role="alert">{ctl.error}</p>
    {/if}

    {#if searching}
      <section class="results" aria-label="Search results" aria-busy={ctl.searching}>
        {#if ctl.searchError}
          <p class="error" role="alert">{ctl.searchError}</p>
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
                    ><span class="chip">{typeLabel(h.type)}</span> #{h.recordIndex} in
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
                <span class="chip">{s.mode}</span>
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
                  <span class="warn" title="Unreadable lines were skipped"
                    >{s.skippedLines} bad line{s.skippedLines === 1 ? '' : 's'}</span
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
      <div class="empty muted">
        <p>Select a transcript to see its timeline and metrics.</p>
      </div>
    {:else if ctl.detailLoading}
      <p class="muted pad">Loading transcript...</p>
    {:else if ctl.detailError && !ctl.detail}
      <p class="error pad" role="alert">{ctl.detailError}</p>
    {:else if ctl.detail}
      {@const d = ctl.detail}
      <header class="dhead">
        <button type="button" class="small back" onclick={() => ctl.close()}>Back to list</button>
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
          <button type="button" class="primary" onclick={() => onresume(d.meta.id)}
            >Resume session</button
          >
          <button type="button" onclick={copyMarkdown} disabled={copyState === 'busy'}>
            {copyState === 'busy'
              ? 'Preparing...'
              : copyState === 'done'
                ? 'Copied'
                : copyState === 'failed'
                  ? 'Copy failed'
                  : 'Copy as markdown'}
          </button>
        </div>
      </header>
      {#if d.skippedLines > 0}
        <p class="banner" role="status" data-testid="skipped">
          {d.skippedLines} unreadable line{d.skippedLines === 1 ? ' was' : 's were'} skipped (a truncated
          or corrupt part of the file). The remaining records are shown.
        </p>
      {/if}
      <div class="tabs" role="tablist" aria-label="Transcript views">
        <button
          type="button"
          role="tab"
          id="tab-timeline"
          aria-selected={tab === 'timeline'}
          aria-controls="panel-timeline"
          onclick={() => (tab = 'timeline')}>Timeline</button
        >
        <button
          type="button"
          role="tab"
          id="tab-metrics"
          aria-selected={tab === 'metrics'}
          aria-controls="panel-metrics"
          onclick={() => (tab = 'metrics')}>Metrics</button
        >
        <span class="muted loaded" data-testid="loaded"
          >{d.records.length.toLocaleString()} of {d.total.toLocaleString()} records loaded</span
        >
      </div>
      {#if ctl.detailError}
        <p class="error pad" role="alert">{ctl.detailError}</p>
      {/if}
      <div
        class="scroll"
        bind:this={scroller}
        role="tabpanel"
        id={tab === 'timeline' ? 'panel-timeline' : 'panel-metrics'}
        aria-labelledby={tab === 'timeline' ? 'tab-timeline' : 'tab-metrics'}
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
              <button type="button" disabled={ctl.loadingMore} onclick={() => ctl.loadMore()}>
                {ctl.loadingMore
                  ? 'Loading...'
                  : `Load more (${ctl.remaining.toLocaleString()} remaining)`}
              </button>
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
    border-right: 1px solid var(--border);
    background: var(--surface);
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
    padding: 8px;
    gap: 8px;
  }
  .lhead {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    margin-left: auto;
    font-size: 12px;
  }
  .small {
    padding: 2px 8px;
    font-size: 12px;
  }
  .search {
    width: 100%;
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 10px;
    align-items: center;
    font-size: 12px;
  }
  .controls label {
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .controls select {
    max-width: 130px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  .session,
  .hit {
    display: grid;
    gap: 2px;
    width: 100%;
    text-align: left;
    padding: 6px 8px;
    background: var(--bg);
  }
  .session[aria-current='true'] {
    border-color: var(--accent);
    background: var(--code-bg);
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
    gap: 2px 10px;
    font-size: 12px;
  }
  .chip {
    display: inline-block;
    font-size: 11px;
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: 999px;
  }
  .warn {
    color: var(--warn);
  }
  .hitmeta {
    font-size: 12px;
    color: var(--muted);
  }
  .snippet {
    overflow-wrap: anywhere;
  }
  .pad {
    padding: 8px;
  }
  .error {
    color: var(--err);
    margin: 0;
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
    gap: 12px;
    align-items: flex-start;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .back {
    display: none;
  }
  .titles {
    flex: 1;
    min-width: 200px;
  }
  .titles h2 {
    font-size: 17px;
  }
  .ids {
    margin: 2px 0 0;
    font-size: 12px;
  }
  .actions {
    display: flex;
    gap: 8px;
  }
  .banner {
    margin: 0;
    padding: 6px 12px;
    background: var(--code-bg);
    border-bottom: 1px solid var(--border);
    color: var(--warn);
  }
  .tabs {
    display: flex;
    gap: 4px;
    align-items: center;
    padding: 6px 12px 0;
    border-bottom: 1px solid var(--border);
  }
  .tabs [role='tab'] {
    border-radius: 6px 6px 0 0;
    border-bottom-color: transparent;
  }
  .tabs [role='tab'][aria-selected='true'] {
    background: var(--code-bg);
    border-color: var(--accent);
    border-bottom-color: var(--code-bg);
  }
  .loaded {
    margin-left: auto;
    font-size: 12px;
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 8px 12px;
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
    padding: 12px;
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
    .back {
      display: inline-block;
    }
  }
</style>
