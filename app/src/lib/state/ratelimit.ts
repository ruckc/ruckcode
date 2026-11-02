import type { PromptResponse } from '@agentclientprotocol/sdk';
import type { RateLimitNotification } from '../acp/ext';

/** A retry wait announced by the agent; the UI counts down to `resumeAtMs`. */
export interface RateWait {
  reason: string;
  /** Unix epoch milliseconds when the request is sent again. */
  resumeAtMs: number;
  attempt: number;
  maxAttempts: number;
}

/** The wait a `_ruckcode/rate_limit` notification starts, or null when it clears one. */
export function rateWaitOf(n: RateLimitNotification): RateWait | null {
  if (n.status !== 'waiting') return null;
  return {
    reason: n.reason ?? 'Rate limited',
    resumeAtMs: n.resumeAtMs ?? 0,
    attempt: n.attempt ?? 0,
    maxAttempts: n.maxAttempts ?? 0
  };
}

/** "51s", "5m 10s", "1h 0m". */
export function formatWait(totalSeconds: number): string {
  const s = Math.max(0, Math.ceil(totalSeconds));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

/** "Rate limited (tokens/minute) — retrying in 51s (retry 1/6)", counting down to zero. */
export function rateWaitText(w: RateWait, nowMs: number): string {
  const left = formatWait(Math.max(0, w.resumeAtMs - nowMs) / 1000);
  const retry = w.attempt > 0 ? ` (retry ${w.attempt}/${w.maxAttempts})` : '';
  return `${w.reason} — retrying in ${left}${retry}`;
}

/**
 * The turn's real stop reason. `rate_limited` is not an ACP stop reason, so the agent reports
 * `end_turn` and carries it in `_meta.ruckcode.stopReason`.
 */
export function stopReasonOf(res: PromptResponse): string {
  const meta = res._meta as { ruckcode?: { stopReason?: unknown } } | null | undefined;
  const real = meta?.ruckcode?.stopReason;
  return typeof real === 'string' ? real : res.stopReason;
}
