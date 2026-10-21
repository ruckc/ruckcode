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
