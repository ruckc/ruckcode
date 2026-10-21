/** WSL2 host types and the API the UI uses (Tauri commands, or a fake in browser mock mode). */

export interface WslDistro {
  name: string;
  /** "Running", "Stopped", ... (localized by Windows; null from the `-l -q` fallback). */
  state: string | null;
  version: number | null;
  default: boolean;
  selectable: boolean;
  reason: string | null;
}

export interface RcStatus {
  installed: boolean;
  path: string | null;
  version: string | null;
  /** The version the app expects (its own). */
  expected: string;
}

export interface WslApi {
  listDistros(): Promise<WslDistro[]>;
  checkRc(distro: string): Promise<RcStatus>;
  /** Install the matching `rc`; `onProgress` receives the installer's output lines. */
  installRc(distro: string, onProgress: (line: string) => void): Promise<RcStatus>;
  /** Linux path for a Windows or Linux path (rejects paths of other distros). */
  translatePath(distro: string, path: string): Promise<string>;
}

/** A semver-ish comparison good enough for "does the installed rc match the app". */
export function versionMatches(rc: RcStatus): boolean {
  if (!rc.installed || !rc.version) return false;
  const core = (v: string) => v.split(/[-+]/)[0];
  return core(rc.version) === core(rc.expected);
}

export function distroLabel(d: WslDistro): string {
  const bits = [
    d.default ? 'default' : '',
    d.state ?? '',
    d.version ? `WSL${d.version}` : ''
  ].filter(Boolean);
  return bits.length ? `${d.name} (${bits.join(', ')})` : d.name;
}
