<script lang="ts">
  import { formatDuration, formatTokens } from '../format';
  import type { TimelineItem } from '../transcripts/timeline';
  import { blocksText, permissionLabel, userSourceLabel } from '../transcripts/timeline';
  import DiffView from './DiffView.svelte';
  import KindIcon from './KindIcon.svelte';
  import Markdown from './Markdown.svelte';

  let {
    item,
    focused = false,
    complete = true
  }: {
    item: TimelineItem;
    focused?: boolean;
    /** False while later records (a tool result) may not be loaded yet. */
    complete?: boolean;
  } = $props();

  let showInput = $state(false);
  let userOpen = $state<boolean | null>(null);
  const toolOpen = $derived(
    userOpen ??
      (item.kind === 'tool' &&
        (focused || (item.result?.edits.length ?? 0) > 0 || item.result?.isError === true))
  );

  const pretty = (v: unknown): string => {
    try {
      return JSON.stringify(v, null, 2) ?? '';
    } catch {
      return String(v);
    }
  };
  const time = (ts: string): string => (ts.length >= 19 ? ts.slice(11, 19) : ts);
</script>

<li
  class="item {item.kind}"
  class:focused
  data-key={item.key}
  data-testid="tl-{item.kind}"
  data-index={item.indexes[0]}
>
  <div class="gutter" aria-hidden="true">
    <span class="idx">#{item.indexes[0]}</span>
    <span class="ts">{time(item.ts)}</span>
  </div>
  <div class="content">
    {#if item.kind === 'session_start'}
      <section class="note" aria-label="Session start">
        <strong>Session started</strong>
        <dl class="kv">
          <dt>Project</dt>
          <dd><code>{item.rec.cwd}</code></dd>
          <dt>Profile</dt>
          <dd>{item.rec.profile}</dd>
          <dt>Model</dt>
          <dd>{item.rec.model}</dd>
          <dt>Mode</dt>
          <dd>{item.rec.mode}</dd>
          <dt>rc version</dt>
          <dd>{item.rec.ruckcodeVersion}</dd>
        </dl>
      </section>
    {:else if item.kind === 'user'}
      <section class="msg user" data-source={item.rec.source}>
        <header>
          <strong>{userSourceLabel(item.rec.source)}</strong>
        </header>
        {#if item.rec.source === 'tool_results'}
          <details>
            <summary
              >{item.rec.content.filter((b) => b.type === 'tool_result').length} tool results sent to
              the model</summary
            >
            {#each item.rec.content as b, i (i)}
              {#if b.type === 'tool_result'}
                <pre class="out" class:err={b.isError}>{b.content}</pre>
              {/if}
            {/each}
          </details>
        {:else}
          <div class="plain">{blocksText(item.rec.content)}</div>
        {/if}
      </section>
    {:else if item.kind === 'assistant'}
      {@const thinking = blocksText(item.rec.content, 'thinking')}
      {@const uses = item.rec.content.flatMap((b) => (b.type === 'tool_use' ? [b.name] : []))}
      <section class="msg assistant">
        <header>
          <strong>Assistant</strong>
          <span class="muted">{item.rec.model} · request {item.rec.requestId}</span>
        </header>
        {#if thinking}
          <details class="thinking">
            <summary>Thinking</summary>
            <div class="plain">{thinking}</div>
          </details>
        {/if}
        {#if blocksText(item.rec.content)}
          <Markdown text={blocksText(item.rec.content)} />
        {/if}
        {#if uses.length}
          <p class="uses muted">Requested: {uses.join(', ')}</p>
        {/if}
        <footer class="muted" aria-label="Request metrics">
          <span>{formatDuration(item.rec.metrics.durationMs)}</span>
          {#if item.rec.metrics.timeToFirstTokenMs !== null}
            <span>TTFT {formatDuration(item.rec.metrics.timeToFirstTokenMs)}</span>
          {/if}
          {#if item.rec.metrics.outputTokensPerSec !== null}
            <span>{item.rec.metrics.outputTokensPerSec} tok/s</span>
          {/if}
          <span
            >{formatTokens(item.rec.usage.inputTokens)} in / {formatTokens(
              item.rec.usage.outputTokens
            )} out</span
          >
          <span>stop: {item.rec.stopReason.replace('_', ' ')}</span>
        </footer>
      </section>
    {:else if item.kind === 'tool'}
      {@const kind = item.call?.kind ?? 'other'}
      <section
        class="card"
        class:bad={item.result?.isError}
        aria-label={`Tool ${item.call?.title ?? item.result?.name ?? ''}`}
      >
        <button
          type="button"
          class="head"
          aria-expanded={toolOpen}
          onclick={() => (userOpen = !toolOpen)}
        >
          <KindIcon {kind} />
          <span class="title">{item.call?.title ?? item.result?.name ?? 'tool'}</span>
          <span class="muted kindname">{item.call?.name ?? ''}</span>
          <span
            class="status"
            data-state={item.result ? (item.result.isError ? 'error' : 'ok') : 'pending'}
          >
            {#if !item.result}{complete ? 'no result' : 'result not loaded'}
            {:else if item.result.isError}error
            {:else}ok{/if}
          </span>
          {#if item.result}
            <span class="muted">{formatDuration(item.result.durationMs)}</span>
          {/if}
        </button>
        {#if toolOpen}
          <div class="body">
            {#if item.result}
              <p class="perm muted">
                Permission: <strong data-testid="permission"
                  >{permissionLabel(item.result.permission)}</strong
                >
              </p>
            {/if}
            {#each item.result?.edits ?? [] as e, i (i)}
              <DiffView patch={{ path: e.path, text: e.diff }} />
            {/each}
            {#if item.result?.output}
              <pre class="out" class:err={item.result.isError} aria-label="Tool output">{item.result
                  .output}</pre>
            {/if}
            {#if item.call}
              <button
                type="button"
                class="link"
                aria-expanded={showInput}
                onclick={() => (showInput = !showInput)}
                >{showInput ? 'Hide input' : 'Show input'}</button
              >
              {#if showInput}
                <pre class="out" aria-label="Tool input">{pretty(item.call.input)}</pre>
              {/if}
            {/if}
          </div>
        {/if}
      </section>
    {:else if item.kind === 'hook'}
      <section
        class="card hook"
        class:bad={!item.rec.success}
        aria-label={`Hook ${item.rec.event}`}
      >
        <div class="head static">
          <strong>Hook</strong>
          <span class="muted">{item.rec.event}</span>
          <code class="cmd">{item.rec.command}</code>
          <span class="status" data-state={item.rec.success ? 'ok' : 'error'}>
            {item.rec.success ? 'passed' : `failed (exit ${item.rec.exitCode ?? '?'})`}
          </span>
          <span class="muted">{formatDuration(item.rec.durationMs)}</span>
        </div>
        {#if item.rec.output}
          <details open={!item.rec.success}>
            <summary>Output</summary>
            <pre class="out" class:err={!item.rec.success}>{item.rec.output}</pre>
          </details>
        {/if}
      </section>
    {:else if item.kind === 'mode'}
      <p class="line">
        <span class="chip">Mode</span>
        <code>{item.rec.from}</code> &rarr; <code>{item.rec.to}</code>
        <span class="muted"
          >({item.rec.initiatedBy === 'agent' ? 'agent' : 'you'}) {item.rec.reason}</span
        >
      </p>
    {:else if item.kind === 'model'}
      <p class="line">
        <span class="chip">Model</span>
        <code>{item.rec.from ?? 'default'}</code> &rarr; <code>{item.rec.to ?? 'default'}</code>
      </p>
    {:else if item.kind === 'compaction'}
      <section class="card compaction" aria-label="Compaction">
        <div class="head static">
          <strong>Compaction</strong>
          <span class="muted">{item.rec.replacedMessages} messages replaced by a summary</span>
        </div>
        <details>
          <summary>Summary</summary>
          <div class="plain">{item.rec.summary}</div>
        </details>
      </section>
    {:else if item.kind === 'turn_end'}
      <p class="line turn" aria-label="Turn summary">
        <span class="chip">Turn end</span>
        {item.rec.stopReason.replace('_', ' ')} · {formatDuration(item.rec.durationMs)} ·
        {item.rec.modelRequests} requests · {item.rec.toolCalls} tool calls ·
        {formatTokens(item.rec.inputTokens)} in / {formatTokens(item.rec.outputTokens)} out
      </p>
    {:else if item.kind === 'mcp'}
      <section class="note" aria-label="MCP servers">
        <strong>MCP servers</strong>
        <ul class="servers">
          {#each item.rec.servers as s (s.name)}
            <li data-ok={s.ok}>
              <code>{s.name}</code>
              {#if s.ok}
                <span class="ok">{s.tools} tools</span>
              {:else}
                <span class="bad-text">failed: {s.error ?? 'unknown error'}</span>
              {/if}
            </li>
          {/each}
        </ul>
      </section>
    {:else if item.kind === 'resumed'}
      <p class="line resumed"><span class="chip">Session resumed</span></p>
    {/if}
  </div>
</li>

<style>
  .item {
    display: grid;
    grid-template-columns: 64px minmax(0, 1fr);
    gap: 8px;
    padding: 4px 0;
    border-radius: 6px;
  }
  .item.focused {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
    background: var(--code-bg);
  }
  .gutter {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    font-size: 11px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    padding-top: 6px;
  }
  .content {
    min-width: 0;
  }
  .msg,
  .card,
  .note {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    padding: 6px 10px;
  }
  .msg.user {
    border-left: 4px solid var(--accent);
  }
  .msg.user[data-source='hook'],
  .msg.user[data-source='fix_turn'] {
    border-left-color: var(--warn);
  }
  .msg.user[data-source='tool_results'] {
    border-left-color: var(--border);
  }
  .msg.assistant {
    border-left: 4px solid var(--ok);
  }
  header {
    display: flex;
    gap: 8px;
    align-items: baseline;
  }
  .plain {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .thinking {
    margin: 4px 0;
    color: var(--muted);
  }
  .thinking .plain {
    font-style: italic;
    padding: 4px 0 4px 10px;
    border-left: 2px solid var(--border);
  }
  footer {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
    font-size: 12px;
    margin-top: 4px;
  }
  .uses {
    margin: 4px 0 0;
    font-size: 12px;
  }
  .card {
    padding: 0;
    border-left: 4px solid var(--ok);
  }
  .card.bad {
    border-left-color: var(--err);
  }
  .card.hook {
    border-left-color: var(--warn);
  }
  .card.hook.bad {
    border-left-color: var(--err);
  }
  .card.compaction {
    border-left-color: var(--accent);
  }
  .head {
    display: flex;
    gap: 8px;
    align-items: center;
    width: 100%;
    border: 0;
    background: none;
    text-align: left;
    padding: 6px 10px;
    flex-wrap: wrap;
  }
  .head.static {
    cursor: default;
  }
  .title {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .kindname {
    font-size: 11px;
  }
  .status {
    margin-left: auto;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
  }
  .status[data-state='ok'] {
    color: var(--ok);
  }
  .status[data-state='error'] {
    color: var(--err);
  }
  .body,
  details {
    padding: 0 10px 8px;
    display: grid;
    gap: 6px;
  }
  .msg details,
  .note details {
    padding: 0;
  }
  summary {
    cursor: pointer;
    color: var(--muted);
  }
  .perm {
    margin: 0;
    font-size: 12px;
  }
  .out {
    margin: 0;
    padding: 6px 8px;
    background: var(--code-bg);
    border-radius: 4px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 320px;
    overflow: auto;
  }
  .out.err {
    color: var(--err);
  }
  .cmd {
    overflow-wrap: anywhere;
  }
  .link {
    justify-self: start;
    border: 0;
    background: none;
    color: var(--accent-link);
    padding: 0;
    font-size: 12px;
    text-decoration: underline;
  }
  .line {
    margin: 0;
    padding: 4px 0;
    font-size: 13px;
  }
  .line.turn {
    color: var(--muted);
    border-bottom: 1px dashed var(--border);
    padding-bottom: 8px;
  }
  .line.resumed {
    text-align: center;
  }
  .chip {
    display: inline-block;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0 6px;
    margin-right: 4px;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--muted);
  }
  .note {
    background: var(--code-bg);
  }
  .kv {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 2px 12px;
    margin: 4px 0 0;
  }
  .kv dt {
    color: var(--muted);
  }
  .kv dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .servers {
    margin: 4px 0 0;
    padding-left: 18px;
  }
  .ok {
    color: var(--ok);
  }
  .bad-text {
    color: var(--err);
  }
  @media (max-width: 600px) {
    .item {
      grid-template-columns: minmax(0, 1fr);
    }
    .gutter {
      flex-direction: row;
      gap: 8px;
      align-items: baseline;
      padding-top: 0;
    }
  }
</style>
