<script lang="ts">
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
    <p class="partial" role="status">
      Based on {records.length.toLocaleString()} of {total.toLocaleString()} records.
      <button type="button" class="small" disabled={loading} onclick={onloadall}>
        {loading ? 'Loading...' : 'Load all records'}
      </button>
    </p>
  {/if}

  {#if m.requests.length === 0}
    <p class="muted">No model requests in the loaded records.</p>
  {:else}
    <dl class="tiles" aria-label="Summary">
      <div>
        <dt>Requests</dt>
        <dd data-testid="m-requests">{m.totals.requests}</dd>
      </div>
      <div>
        <dt>Avg latency</dt>
        <dd>{formatDuration(m.totals.avgDurationMs)}</dd>
      </div>
      <div>
        <dt>Avg TTFT</dt>
        <dd>{formatDuration(m.totals.avgTtftMs)}</dd>
      </div>
      <div>
        <dt>Avg tokens/s</dt>
        <dd>{fmt1(m.totals.avgTokensPerSec)}</dd>
      </div>
      <div>
        <dt>Input tokens</dt>
        <dd>{formatTokens(m.totals.inputTokens)}</dd>
      </div>
      <div>
        <dt>Output tokens</dt>
        <dd>{formatTokens(m.totals.outputTokens)}</dd>
      </div>
    </dl>

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
  .metrics {
    --chart-bar: #93c5fd;
    --chart-ttft: #1d4ed8;
    --chart-in: #1d4ed8;
    --chart-out: #b45309;
    --chart-axis: var(--border);
    display: grid;
    gap: 12px;
    padding: 4px 2px 16px;
  }
  @media (prefers-color-scheme: dark) {
    .metrics {
      --chart-bar: #1e40af;
      --chart-ttft: #93c5fd;
      --chart-in: #93c5fd;
      --chart-out: #fbbf24;
    }
  }
  .partial {
    margin: 0;
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--code-bg);
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
    gap: 8px;
    margin: 0;
  }
  .tiles div {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    padding: 6px 10px;
  }
  dt {
    color: var(--muted);
    font-size: 12px;
  }
  dd {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  figure {
    margin: 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
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
    fill: var(--muted);
    font-size: 10px;
  }
  .bar {
    fill: var(--chart-bar);
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
    color: var(--muted);
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
    border-bottom: 1px solid var(--border);
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
    background: var(--surface);
  }
</style>
