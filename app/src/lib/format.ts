/** Short single-line summary of a tool's raw input (for permission dialogs and cards). */
export function summarizeInput(input: unknown, max = 160): string[] {
  if (input === null || input === undefined) return [];
  if (typeof input !== 'object') return [clip(String(input), max)];
  return Object.entries(input as Record<string, unknown>).map(([k, v]) => {
    const s = typeof v === 'string' ? v : JSON.stringify(v);
    return `${k}: ${clip(s.replace(/\s+/g, ' '), max)}`;
  });
}

function clip(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

/** First `max` lines of text and how many were left out. */
export function truncateLines(text: string, max: number): { shown: string; hidden: number } {
  const lines = text.split('\n');
  if (lines.length <= max) return { shown: text, hidden: 0 };
  return { shown: lines.slice(0, max).join('\n'), hidden: lines.length - max };
}

export function relativeTime(iso: string | null, now = Date.now()): string {
  if (!iso) return '';
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return '';
  const s = Math.max(0, Math.round((now - t) / 1000));
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

/** Split a trailing "(Recommended)" marker off an option label. */
export function parseRecommended(label: string): { label: string; recommended: boolean } {
  const m = /^(.*?)\s*\(recommended\)\s*$/i.exec(label);
  return m ? { label: m[1], recommended: true } : { label, recommended: false };
}

/** `850 ms`, `2.5 s`, `1m 05s`, `1h 02m`. */
export function formatDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return '-';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s < 10 ? s.toFixed(1) : Math.round(s)} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(Math.round(s - m * 60)).padStart(2, '0')}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${String(m - h * 60).padStart(2, '0')}m`;
}

/** `1,234` / `12.3k` / `1.2M` for token counts. */
export function formatTokens(n: number): string {
  if (n < 10_000) return n.toLocaleString('en-US');
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 100_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
