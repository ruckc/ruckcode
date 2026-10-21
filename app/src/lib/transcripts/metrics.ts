/** Per-request and per-turn metrics computed from (the loaded part of) a transcript. */
import type { TranscriptRecord } from '../host/transcripts';

export interface RequestPoint {
  /** 1-based position among the loaded requests. */
  n: number;
  requestId: number;
  index: number;
  model: string;
  durationMs: number;
  ttftMs: number | null;
  tokensPerSec: number | null;
  inputTokens: number;
  outputTokens: number;
  cumInput: number;
  cumOutput: number;
}

export interface TurnPoint {
  n: number;
  index: number;
  durationMs: number;
  modelRequests: number;
  toolCalls: number;
  inputTokens: number;
  outputTokens: number;
  stopReason: string;
}

export interface Metrics {
  requests: RequestPoint[];
  turns: TurnPoint[];
  totals: {
    requests: number;
    inputTokens: number;
    outputTokens: number;
    avgDurationMs: number | null;
    maxDurationMs: number | null;
    avgTtftMs: number | null;
    avgTokensPerSec: number | null;
  };
}

const mean = (xs: number[]): number | null =>
  xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null;

export function computeMetrics(records: TranscriptRecord[]): Metrics {
  const requests: RequestPoint[] = [];
  const turns: TurnPoint[] = [];
  let cumInput = 0;
  let cumOutput = 0;
  for (const r of records) {
    if (r.type === 'assistant_message') {
      cumInput += r.usage.inputTokens;
      cumOutput += r.usage.outputTokens;
      requests.push({
        n: requests.length + 1,
        requestId: r.requestId,
        index: r.index,
        model: r.model,
        durationMs: r.metrics.durationMs,
        ttftMs: r.metrics.timeToFirstTokenMs,
        tokensPerSec: r.metrics.outputTokensPerSec,
        inputTokens: r.usage.inputTokens,
        outputTokens: r.usage.outputTokens,
        cumInput,
        cumOutput
      });
    } else if (r.type === 'turn_end') {
      turns.push({
        n: turns.length + 1,
        index: r.index,
        durationMs: r.durationMs,
        modelRequests: r.modelRequests,
        toolCalls: r.toolCalls,
        inputTokens: r.inputTokens,
        outputTokens: r.outputTokens,
        stopReason: r.stopReason
      });
    }
  }
  const durations = requests.map((r) => r.durationMs);
  return {
    requests,
    turns,
    totals: {
      requests: requests.length,
      inputTokens: cumInput,
      outputTokens: cumOutput,
      avgDurationMs: mean(durations),
      maxDurationMs: durations.length ? Math.max(...durations) : null,
      avgTtftMs: mean(requests.flatMap((r) => (r.ttftMs === null ? [] : [r.ttftMs]))),
      avgTokensPerSec: mean(
        requests.flatMap((r) => (r.tokensPerSec === null ? [] : [r.tokensPerSec]))
      )
    }
  };
}
