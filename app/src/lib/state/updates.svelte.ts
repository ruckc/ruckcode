import type { UpdatesApi } from '../host/updates';

/** How often to look for a new version while the app stays open. */
export const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;
const AUTO_KEY = 'ruckcode.autoUpdate';

export type UpdatePhase =
  | 'idle'
  | 'checking'
  /** A newer version exists; downloading is off (the user opted out of automatic downloads). */
  | 'available'
  | 'downloading'
  | 'ready'
  | 'installing'
  /** A newer version exists but this install cannot update itself. */
  | 'manual'
  | 'unconfigured'
  | 'uptodate'
  | 'error';

interface Options {
  /** True while an agent turn runs: the app must not restart under it. */
  isBusy?: () => boolean;
  intervalMs?: number;
  storage?: Pick<Storage, 'getItem' | 'setItem'> | null;
}

function defaultStorage(): Options['storage'] {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    return null;
  }
}

/**
 * Drives the update flow: check at startup and every 6 hours, download in the background (unless
 * switched off), then wait for the user to restart, never while an agent turn is running.
 */
export class UpdateController {
  phase = $state<UpdatePhase>('idle');
  version = $state('');
  current = $state('');
  notes = $state<string | null>(null);
  downloaded = $state(0);
  total = $state<number | null>(null);
  /** Why the update is manual / why updates are not configured. */
  reason = $state('');
  downloadPage = $state('');
  /** The last failure (a failed check is quiet; a failed download shows in the banner). */
  error = $state('');
  lastChecked = $state<number | null>(null);
  /** "Automatically download updates" (persisted). */
  auto = $state(true);
  /** The user chose "Later" (or dismissed): hide the banner until the next version. */
  dismissed = $state(false);

  private timer: ReturnType<typeof setInterval> | undefined;
  private readonly storage: Options['storage'];
  private readonly isBusy: () => boolean;
  private readonly intervalMs: number;

  constructor(
    private readonly api: UpdatesApi,
    opts: Options = {}
  ) {
    this.storage = opts.storage === undefined ? defaultStorage() : opts.storage;
    this.isBusy = opts.isBusy ?? (() => false);
    this.intervalMs = opts.intervalMs ?? CHECK_INTERVAL_MS;
    try {
      if (this.storage?.getItem(AUTO_KEY) === '0') this.auto = false;
    } catch {
      // storage can be blocked: keep the default
    }
  }

  /** Check now and then every interval. */
  start(): void {
    this.stop();
    void this.check();
    this.timer = setInterval(() => void this.check(), this.intervalMs);
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
  }

  get busy(): boolean {
    return this.isBusy();
  }

  /** Progress 0..1 while downloading, or null when the size is unknown. */
  get progress(): number | null {
    return this.total ? Math.min(1, this.downloaded / this.total) : null;
  }

  /** The banner shows for these phases (and not after "Later"). */
  get visible(): boolean {
    if (this.dismissed) return false;
    return (
      this.phase === 'downloading' ||
      this.phase === 'ready' ||
      this.phase === 'installing' ||
      this.phase === 'available' ||
      this.phase === 'manual' ||
      (this.phase === 'error' && !!this.version)
    );
  }

  setAuto(on: boolean): void {
    this.auto = on;
    try {
      this.storage?.setItem(AUTO_KEY, on ? '1' : '0');
    } catch {
      // not persisted
    }
    // Turning it on picks up a pending update.
    if (on && this.phase === 'available') void this.download();
  }

  /** Look for an update. `manual` (the user asked) also reports failures and un-dismisses. */
  async check(manual = false): Promise<void> {
    // Never disturb a download or an install in flight, or an update that is already waiting.
    if (['checking', 'downloading', 'ready', 'installing'].includes(this.phase)) return;
    const before = this.phase;
    this.phase = 'checking';
    this.error = '';
    try {
      const r = await this.api.check();
      this.lastChecked = Date.now();
      if (r.state === 'notConfigured') {
        this.phase = 'unconfigured';
        this.reason = r.reason;
      } else if (r.state === 'upToDate') {
        this.phase = 'uptodate';
        this.current = r.version;
      } else {
        if (r.version !== this.version || manual) this.dismissed = false;
        this.version = r.version;
        this.current = r.current;
        this.notes = r.notes;
        this.downloadPage = r.downloadPage;
        this.reason = r.manualReason ?? '';
        if (r.manualReason) this.phase = 'manual';
        else if (this.auto) await this.download();
        else this.phase = 'available';
      }
    } catch (e) {
      this.lastChecked = Date.now();
      this.error = e instanceof Error ? e.message : String(e);
      // A failed periodic check is quiet: keep what we knew, show it only in Settings.
      this.phase = before === 'checking' ? 'idle' : before === 'error' ? 'idle' : before;
      if (manual) this.phase = 'error';
    }
  }

  /** Download the staged update in the background. */
  async download(): Promise<void> {
    if (this.phase === 'downloading') return;
    this.phase = 'downloading';
    this.downloaded = 0;
    this.total = null;
    this.error = '';
    try {
      await this.api.download((done, total) => {
        this.downloaded = done;
        this.total = total;
      });
      this.phase = 'ready';
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
      this.phase = 'error';
    }
  }

  /** "Later": hide the banner; the downloaded update stays staged. */
  later(): void {
    this.dismissed = true;
  }

  /**
   * Install and relaunch. Refuses while an agent turn runs unless `force` (the user confirmed
   * ending it). Returns whether it went ahead.
   */
  async restartNow(force = false): Promise<boolean> {
    if (this.phase !== 'ready') return false;
    if (this.busy && !force) return false;
    this.phase = 'installing';
    try {
      await this.api.install();
      await this.api.restart();
      return true;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
      this.phase = 'error';
      return false;
    }
  }
}
