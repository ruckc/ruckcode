import type { RcStatus, WslApi, WslDistro } from './wsl';
import { windowsToLinux } from './wsl-paths';

const EXPECTED = '0.1.0';

const DISTROS: WslDistro[] = [
  {
    name: 'Ubuntu-24.04',
    state: 'Running',
    version: 2,
    default: true,
    selectable: true,
    reason: null
  },
  { name: 'Debian', state: 'Stopped', version: 2, default: false, selectable: true, reason: null },
  {
    name: 'Alpine',
    state: 'Stopped',
    version: 1,
    default: false,
    selectable: false,
    reason: 'WSL1 distro (WSL2 required)'
  },
  {
    name: 'docker-desktop',
    state: 'Running',
    version: 2,
    default: false,
    selectable: false,
    reason: 'Docker Desktop internal distro'
  }
];

export interface FakeWslOptions {
  /** Delay between install progress lines (ms). */
  delayMs?: number;
}

/**
 * A scripted WSL2 environment for browser mock mode: Ubuntu-24.04 has no `rc` yet (it is installed automatically from the "bundled" copy),
 * Debian has an older one (version mismatch), and everything else is not selectable. A distro named
 * `Broken` makes the install fail, for the error path.
 */
export class FakeWsl implements WslApi {
  private installed = new Map<string, string>([['Debian', '0.0.9']]);
  readonly calls: string[] = [];
  distros: WslDistro[] = DISTROS;
  private delayMs: number;

  constructor(opts: FakeWslOptions = {}) {
    this.delayMs = opts.delayMs ?? 120;
  }

  async listDistros(): Promise<WslDistro[]> {
    this.calls.push('list');
    return this.distros;
  }

  async checkRc(distro: string): Promise<RcStatus> {
    this.calls.push(`check:${distro}`);
    const version = this.installed.get(distro) ?? null;
    return {
      installed: !!version,
      path: version ? '/home/dev/.local/bin/rc' : null,
      version,
      expected: EXPECTED
    };
  }

  /** Pages download (fallback path). */
  async installRc(distro: string, onProgress: (line: string) => void): Promise<RcStatus> {
    this.calls.push(`install:${distro}`);
    const steps = [
      '[rc-install] architecture x86_64, target x86_64-unknown-linux-musl',
      `[rc-install] downloading rc-${EXPECTED}-x86_64-unknown-linux-musl.tar.gz`,
      '[rc-install] verifying checksum',
      '[rc-install] extracting'
    ];
    return this.run(distro, steps, onProgress);
  }

  /** The rc shipped in the installer, copied over stdin. */
  async installBundledRc(distro: string, onProgress: (line: string) => void): Promise<RcStatus> {
    this.calls.push(`bundled:${distro}`);
    const steps = [
      `[rc-install] copying bundled rc (9216 KiB, sha256 0123456789ab) into ${distro}`,
      '[rc-install] sha256 verified'
    ];
    return this.run(distro, steps, onProgress);
  }

  async ensureRc(distro: string, onProgress: (line: string) => void): Promise<RcStatus> {
    this.calls.push(`ensure:${distro}`);
    const status = await this.checkRc(distro);
    if (status.installed && status.version === EXPECTED) return status;
    return this.installBundledRc(distro, onProgress);
  }

  private async run(
    distro: string,
    steps: string[],
    onProgress: (line: string) => void
  ): Promise<RcStatus> {
    for (const s of steps) {
      await new Promise((r) => setTimeout(r, this.delayMs));
      onProgress(s);
    }
    if (distro === 'Broken') throw new Error('checksum mismatch (expected aa, got bb)');
    await new Promise((r) => setTimeout(r, this.delayMs));
    onProgress(`[rc-install] installed /home/dev/.local/bin/rc (version ${EXPECTED})`);
    this.installed.set(distro, EXPECTED);
    return this.checkRc(distro);
  }

  async translatePath(distro: string, path: string): Promise<string> {
    const t = windowsToLinux(path, distro);
    if (!t.ok) throw new Error(t.error);
    return t.linux;
  }
}
