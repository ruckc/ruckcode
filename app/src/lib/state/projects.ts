/** Recent projects, persisted per viewer in localStorage (always optional: storage can throw). */
const KEY = 'ruckcode.recent';
const LEGACY_KEY = 'ruckcode.cwd';
export const MAX_RECENT = 8;

export type ProjectHost = 'local' | 'wsl' | 'subprocess';

/** A project and how to reach its agent; remembered per project in the recent list. */
export interface Project {
  path: string;
  host: ProjectHost;
  /** WSL distro (host `wsl`). */
  distro?: string;
  /** Custom `rc` binary (host `subprocess`). */
  rcPath?: string;
}

/** Same project = same path on the same host (and distro). */
export function projectKey(p: Project): string {
  return `${p.host}\0${p.distro ?? ''}\0${p.path}`;
}

/** Short host label for lists, empty for the default local host. */
export function hostLabel(p: Project): string {
  return p.host === 'wsl' ? `WSL: ${p.distro ?? '?'}` : p.host === 'subprocess' ? 'custom rc' : '';
}

export function pushRecent(list: Project[], project: Project): Project[] {
  const path = project.path.trim();
  if (!path) return list;
  const p = { ...project, path };
  const key = projectKey(p);
  return [p, ...list.filter((x) => projectKey(x) !== key)].slice(0, MAX_RECENT);
}

function toProject(v: unknown): Project | null {
  if (typeof v === 'string') return v ? { path: v, host: 'local' } : null; // legacy entries
  if (!v || typeof v !== 'object') return null;
  const o = v as Record<string, unknown>;
  if (typeof o.path !== 'string' || !o.path) return null;
  const host: ProjectHost = o.host === 'wsl' || o.host === 'subprocess' ? o.host : 'local';
  const p: Project = { path: o.path, host };
  if (host === 'wsl' && typeof o.distro === 'string') p.distro = o.distro;
  if (host === 'subprocess' && typeof o.rcPath === 'string') p.rcPath = o.rcPath;
  return p;
}

export function loadRecent(storage: Pick<Storage, 'getItem'> | null = safeStorage()): Project[] {
  try {
    const raw = storage?.getItem(KEY);
    if (raw) {
      const v: unknown = JSON.parse(raw);
      if (Array.isArray(v)) return v.map(toProject).filter((x): x is Project => !!x);
    }
    const legacy = storage?.getItem(LEGACY_KEY);
    return legacy ? [{ path: legacy, host: 'local' }] : [];
  } catch {
    return [];
  }
}

export function saveRecent(
  list: Project[],
  storage: Pick<Storage, 'setItem'> | null = safeStorage()
) {
  try {
    storage?.setItem(KEY, JSON.stringify(list));
  } catch {
    // storage unavailable; recents just aren't remembered
  }
}

export function removeRecent(list: Project[], project: Project): Project[] {
  const key = projectKey(project);
  return list.filter((x) => projectKey(x) !== key);
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
