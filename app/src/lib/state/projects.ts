/** Recent projects, persisted per viewer in localStorage (always optional: storage can throw). */
const KEY = 'ruckcode.recent';
const LEGACY_KEY = 'ruckcode.cwd';
export const MAX_RECENT = 8;

export function pushRecent(list: string[], path: string): string[] {
  const p = path.trim();
  if (!p) return list;
  return [p, ...list.filter((x) => x !== p)].slice(0, MAX_RECENT);
}

export function loadRecent(storage: Pick<Storage, 'getItem'> | null = safeStorage()): string[] {
  try {
    const raw = storage?.getItem(KEY);
    if (raw) {
      const v: unknown = JSON.parse(raw);
      if (Array.isArray(v)) return v.filter((x): x is string => typeof x === 'string');
    }
    const legacy = storage?.getItem(LEGACY_KEY);
    return legacy ? [legacy] : [];
  } catch {
    return [];
  }
}

export function saveRecent(
  list: string[],
  storage: Pick<Storage, 'setItem'> | null = safeStorage()
) {
  try {
    storage?.setItem(KEY, JSON.stringify(list));
  } catch {
    // storage unavailable; recents just aren't remembered
  }
}

export function removeRecent(list: string[], path: string): string[] {
  return list.filter((x) => x !== path);
}

function safeStorage(): Storage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    return null;
  }
}

/** Last path segment, for compact display. */
export function baseName(path: string): string {
  const parts = path.replace(/[\\/]+$/, '').split(/[\\/]/);
  return parts[parts.length - 1] || path;
}
