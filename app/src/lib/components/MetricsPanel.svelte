<script lang="ts">
  import { Button, Callout, Stat } from '@ruckc/truss-ui';
  import { formatDuration, formatTokens } from '../format';
  import { computeMetrics } from '../transcripts/metrics';
  import type { TranscriptRecord } from '../host/transcripts';

  let {
    records,
    total,
    loading = false,
    onloadall
  }: {
    records: TranscriptRecord[];
    total: number;
    loading?: boolean;
    onloadall: () => void;
  } = $props();

  const m = $derived(computeMetrics(records));
  const partial = $derived(records.length < total);

  // Chart geometry (viewBox units; the SVG scales to its container).
  const W = 640;
  const H = 170;
  const L = 48;
  const R = 8;
  const T = 8;
  const B = 24;
  const pw = W - L - R;
  const ph = H - T - B;

  const maxDur = $derived(Math.max(1, ...m.requests.map((r) => r.durationMs)));
  const barW = $derived(m.requests.length ? pw / m.requests.length : pw);
  const maxIn = $derived(Math.max(1, m.totals.inputTokens));
  const maxOut = $derived(Math.max(1, m.totals.outputTokens));

  const y = (v: number, max: number): number => T + ph - (v / max) * ph;
  const x = (i: number, n: number): number => L + (n <= 1 ? pw / 2 : (i / (n - 1)) * pw);

  function line(key: 'cumInput' | 'cumOutput', max: number): string {
    const n = m.requests.length;
    return m.requests.map((r, i) => `${x(i, n).toFixed(1)},${y(r[key], max).toFixed(1)}`).join(' ');
  }

  const fmt1 = (n: number | null): string => (n === null ? '-' : n.toFixed(1));
</script>

<div class="metrics" data-testid="metrics">
  {#if partial}
    <div role="status">
      <Callout intent="primary">
        Based on {records.length.toLocaleString()} of {total.toLocaleString()} records.
        {#snippet actions()}
          <Button
            variant="outlined"
            intent="neutral"
            size="sm"
            disabled={loading}
            onclick={onloadall}
          >
            {loading ? 'Loading...' : 'Load all records'}
          </Button>
        {/snippet}
      </Callout>
    </div>
  {/if}

  {#if m.requests.length === 0}
    <p class="muted">No model requests in the loaded records.</p>
  {:else}
    {#snippet requestsValue()}<span data-testid="m-requests">{m.totals.requests}</span>{/snippet}
    <div class="tiles" role="group" aria-label="Summary">
      <Stat class="tile" value={requestsValue} label="Requests" />
      <Stat class="tile" value={formatDuration(m.totals.avgDurationMs)} label="Avg latency" />
      <Stat class="tile" value={formatDuration(m.totals.avgTtftMs)} label="Avg TTFT" />
      <Stat class="tile" value={fmt1(m.totals.avgTokensPerSec)} label="Avg tokens/s" />
      <Stat class="tile" value={formatTokens(m.totals.inputTokens)} label="Input tokens" />
      <Stat class="tile" value={formatTokens(m.totals.outputTokens)} label="Output tokens" />
    </div>

    <figure>
      <figcaption>
        Request duration <span class="muted">(darker part: time to first token)</span>
      </figcaption>
      <svg
        viewBox="0 0 {W} {H}"
        role="img"
        aria-label={`Bar chart of ${m.requests.length} request durations. Longest ${formatDuration(maxDur)}, average ${formatDuration(m.totals.avgDurationMs)}.`}
        data-testid="chart-durations"
      >
        <line class="axis" x1={L} y1={T + ph} x2={W - R} y2={T + ph} />
        <line class="axis" x1={L} y1={T} x2={L} y2={T + ph} />
        <text class="tick" x={L - 6} y={T + 4} text-anchor="end">{formatDuration(maxDur)}</text>
        <text class="tick" x={L - 6} y={T + ph} text-anchor="end">0</text>
        <text class="tick" x={L} y={H - 6}>request 1</text>
        <text class="tick" x={W - R} y={H - 6} text-anchor="end">request {m.requests.length}</text>
        {#each m.requests as r, i (r.index)}
          {@const h = (r.durationMs / maxDur) * ph}
          {@const th = r.ttftMs === null ? 0 : (Math.min(r.ttftMs, r.durationMs) / maxDur) * ph}
          <g>
            <title
              >Request {r.requestId}: {formatDuration(r.durationMs)}{r.ttftMs !== null
                ? `, TTFT ${formatDuration(r.ttftMs)}`
                : ''}</title
            >
            <rect
              class="bar"
              x={L + i * barW}
              y={T + ph - h}
              width={Math.max(0.5, barW - (barW > 4 ? 1 : 0))}
              height={h}
            />
            {#if th > 0}
              <rect
                class="ttft"
                x={L + i * barW}
                y={T + ph - th}
                width={Math.max(0.5, barW - (barW > 4 ? 1 : 0))}
                height={th}
              />
            {/if}
          </g>
        {/each}
      </svg>
    </figure>

    <div class="pair">
      {#each [{ key: 'cumInput' as const, max: maxIn, label: 'Cumulative input tokens', cls: 'in' }, { key: 'cumOutput' as const, max: maxOut, label: 'Cumulative output tokens', cls: 'out' }] as s (s.key)}
        <figure>
          <figcaption>{s.label}</figcaption>
          <svg
            viewBox="0 0 {W} {H}"
            role="img"
            aria-label={`Line chart: ${s.label.toLowerCase()} over ${m.requests.length} requests, ending at ${formatTokens(s.max)}.`}
            data-testid={`chart-${s.cls}`}
          >
            <line class="axis" x1={L} y1={T + ph} x2={W - R} y2={T + ph} />
            <line class="axis" x1={L} y1={T} x2={L} y2={T + ph} />
            <text class="tick" x={L - 6} y={T + 4} text-anchor="end">{formatTokens(s.max)}</text>
            <text class="tick" x={L - 6} y={T + ph} text-anchor="end">0</text>
            <text class="tick" x={L} y={H - 6}>request 1</text>
            <text class="tick" x={W - R} y={H - 6} text-anchor="end"
              >request {m.requests.length}</text
            >
            <polyline class="series {s.cls}" points={line(s.key, s.max)} />
          </svg>
        </figure>
      {/each}
    </div>

    <details>
      <summary>Per-request table ({m.requests.length})</summary>
      <div class="scroll">
        <table aria-label="Per-request metrics">
          <thead>
            <tr>
              <th scope="col">Request</th>
              <th scope="col">Model</th>
              <th scope="col">Latency</th>
              <th scope="col">TTFT</th>
              <th scope="col">Tokens/s</th>
              <th scope="col">Input</th>
              <th scope="col">Output</th>
            </tr>
          </thead>
          <tbody>
            {#each m.requests as r (r.index)}
              <tr>
                <th scope="row">{r.requestId}</th>
                <td class="l">{r.model}</td>
                <td>{formatDuration(r.durationMs)}</td>
                <td>{formatDuration(r.ttftMs)}</td>
                <td>{fmt1(r.tokensPerSec)}</td>
                <td>{r.inputTokens.toLocaleString()}</td>
                <td>{r.outputTokens.toLocaleString()}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </details>
  {/if}

  {#if m.turns.length}
    <details open>
      <summary>Per-turn totals ({m.turns.length})</summary>
      <div class="scroll">
        <table aria-label="Per-turn totals">
          <thead>
            <tr>
              <th scope="col">Turn</th>
              <th scope="col">Duration</th>
              <th scope="col">Requests</th>
              <th scope="col">Tool calls</th>
              <th scope="col">Input</th>
              <th scope="col">Output</th>
              <th scope="col">Stop</th>
            </tr>
          </thead>
          <tbody>
            {#each m.turns as t (t.index)}
              <tr>
                <th scope="row">{t.n}</th>
                <td>{formatDuration(t.durationMs)}</td>
                <td>{t.modelRequests}</td>
                <td>{t.toolCalls}</td>
                <td>{t.inputTokens.toLocaleString()}</td>
                <td>{t.outputTokens.toLocaleString()}</td>
                <td>{t.stopReason.replace('_', ' ')}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </details>
  {/if}
</div>

<style>
  /* Chart colours are truss tokens, so light and dark follow the theme without media queries. */
  .metrics {
    --chart-bar: var(--tr-primary);
    --chart-ttft: var(--tr-primary);
    --chart-in: var(--tr-primary);
    --chart-out: var(--tr-warning);
    --chart-axis: var(--tr-outline-variant);
    display: grid;
    gap: var(--tr-space-3);
    padding: var(--tr-space-1) 2px var(--tr-space-4);
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
    gap: var(--tr-space-2);
  }
  .tiles :global(.tile) {
    border: 1px solid var(--tr-outline-variant);
    border-radius: var(--tr-radius-md);
    background: var(--tr-surface-container-low);
    padding: var(--tr-space-2) var(--tr-space-3);
    --tr-text-2xl: var(--tr-text-xl);
  }
  figure {
    margin: 0;
    border: 1px solid var(--tr-outline-variant);
    border-radius: 8px;
    background: var(--tr-surface-container-low);
    padding: 6px 10px 2px;
  }
  figcaption {
    font-weight: 600;
    font-size: 13px;
  }
  .pair {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: 12px;
  }
  svg {
    width: 100%;
    height: auto;
    display: block;
  }
  .axis {
    stroke: var(--chart-axis);
    stroke-width: 1;
  }
  .tick {
    fill: var(--tr-on-surface-variant);
    font-size: 10px;
  }
  .bar {
    fill: var(--chart-bar);
    fill-opacity: 0.4;
  }
  .ttft {
    fill: var(--chart-ttft);
  }
  .series {
    fill: none;
    stroke-width: 2;
    stroke-linejoin: round;
  }
  .series.in {
    stroke: var(--chart-in);
  }
  .series.out {
    stroke: var(--chart-out);
  }
  @media (prefers-reduced-motion: no-preference) {
    .bar:hover,
    .ttft:hover {
      opacity: 0.8;
    }
  }
  details > summary {
    cursor: pointer;
    color: var(--tr-on-surface-variant);
  }
  .scroll {
    overflow: auto;
    max-height: 320px;
    margin-top: 6px;
  }
  table {
    border-collapse: collapse;
    width: 100%;
    font-variant-numeric: tabular-nums;
  }
  th,
  td {
    border-bottom: 1px solid var(--tr-outline-variant);
    padding: 2px 8px;
    text-align: right;
    white-space: nowrap;
  }
  th[scope='col']:first-child,
  th[scope='row'],
  td.l {
    text-align: left;
  }
  thead th {
    position: sticky;
    top: 0;
    background: var(--tr-surface-container-low);
  }
</style>
