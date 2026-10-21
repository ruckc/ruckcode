import type {
  ReadResult,
  SearchHit,
  SessionItem,
  TranscriptMeta,
  TranscriptRecord,
  TranscriptsApi,
  TranscriptTarget
} from '../host/transcripts';
import { distinct, filterItems, sortItems, type SortKey } from '../transcripts/list';
import { buildTimeline, keyForRecord, toMarkdown } from '../transcripts/timeline';

/** Records fetched per page. */
export const PAGE = 200;
const SEARCH_DEBOUNCE_MS = 250;
const SEARCH_LIMIT = 100;

const msg = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export interface Detail {
  meta: TranscriptMeta;
  records: TranscriptRecord[];
  total: number;
  skippedLines: number;
}

/** State of the transcript browser: the list, search, and the open transcript (paged). */
export class TranscriptsController {
  items = $state<SessionItem[]>([]);
  loading = $state(false);
  error = $state('');

  sort = $state<SortKey>('updated');
  asc = $state(false);
  mode = $state('');
  model = $state('');

  searchText = $state('');
  hits = $state<SearchHit[]>([]);
  searching = $state(false);
  searchError = $state('');

  selectedId = $state<string | null>(null);
  detail = $state<Detail | null>(null);
  detailLoading = $state(false);
  loadingMore = $state(false);
  detailError = $state('');
  /** Record to scroll to and highlight (after a search jump). */
  focus = $state<number | null>(null);
  /** Bumped on every jump so the view scrolls again even to the same record. */
  focusSeq = $state(0);

  readonly visible = $derived(
    sortItems(filterItems(this.items, { mode: this.mode, model: this.model }), this.sort, this.asc)
  );
  readonly modes = $derived(distinct(this.items, 'mode'));
  readonly models = $derived(distinct(this.items, 'model'));
  readonly timeline = $derived(this.detail ? buildTimeline(this.detail.records) : []);
  readonly complete = $derived(
    this.detail ? this.detail.records.length >= this.detail.total : true
  );
  readonly remaining = $derived(
    this.detail ? Math.max(0, this.detail.total - this.detail.records.length) : 0
  );

  private openSeq = 0;
  private searchSeq = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    private api: TranscriptsApi,
    private target: TranscriptTarget,
    private opts: { debounceMs?: number; pageSize?: number } = {}
  ) {}

  private get page(): number {
    return this.opts.pageSize ?? PAGE;
  }

  async refresh(): Promise<void> {
    this.loading = true;
    this.error = '';
    try {
      this.items = await this.api.list(this.target);
    } catch (e) {
      this.error = msg(e);
    } finally {
      this.loading = false;
    }
  }

  // ---- search ----------------------------------------------------------------------------

  /** Update the search text; the server-side search runs after a short pause in typing. */
  setSearch(text: string): void {
    this.searchText = text;
    clearTimeout(this.timer);
    const seq = ++this.searchSeq;
    if (!text.trim()) {
      this.hits = [];
      this.searching = false;
      this.searchError = '';
      return;
    }
    this.searching = true;
    this.timer = setTimeout(
      () => void this.runSearch(text.trim(), seq),
      this.opts.debounceMs ?? SEARCH_DEBOUNCE_MS
    );
  }

  private async runSearch(text: string, seq: number): Promise<void> {
    try {
      const hits = await this.api.search(this.target, text, SEARCH_LIMIT);
      if (seq !== this.searchSeq) return; // a newer query superseded this one
      this.hits = hits;
      this.searchError = '';
    } catch (e) {
      if (seq !== this.searchSeq) return;
      this.hits = [];
      this.searchError = msg(e);
    } finally {
      if (seq === this.searchSeq) this.searching = false;
    }
  }

  dispose(): void {
    clearTimeout(this.timer);
    this.searchSeq++;
    this.openSeq++;
  }

  // ---- the open transcript -----------------------------------------------------------------

  /** Open a transcript; with `jumpTo`, load through that record and focus it. */
  async open(id: string, jumpTo?: number): Promise<void> {
    const seq = ++this.openSeq;
    if (this.selectedId === id && this.detail && jumpTo !== undefined) {
      await this.jump(jumpTo);
      return;
    }
    this.selectedId = id;
    this.detail = null;
    this.detailError = '';
    this.focus = null;
    this.detailLoading = true;
    try {
      const limit = jumpTo === undefined ? this.page : Math.max(this.page, jumpTo + 1);
      const r = await this.api.read(this.target, id, { offset: 0, limit });
      if (seq !== this.openSeq) return;
      this.detail = fromRead(r);
      if (jumpTo !== undefined) this.setFocus(jumpTo);
    } catch (e) {
      if (seq !== this.openSeq) return;
      this.detailError = msg(e);
    } finally {
      if (seq === this.openSeq) this.detailLoading = false;
    }
  }

  close(): void {
    this.openSeq++;
    this.selectedId = null;
    this.detail = null;
    this.detailError = '';
    this.focus = null;
  }

  private setFocus(index: number): void {
    this.focus = index;
    this.focusSeq++;
  }

  /** Load the next page; resolves when it is merged in. */
  async loadMore(count = this.page): Promise<void> {
    const d = this.detail;
    const id = this.selectedId;
    if (!d || !id || this.loadingMore || this.complete) return;
    const seq = this.openSeq;
    this.loadingMore = true;
    try {
      const r = await this.api.read(this.target, id, {
        offset: d.records.length,
        limit: count
      });
      if (seq !== this.openSeq || !this.detail) return;
      this.detail = {
        ...fromRead(r),
        // Keep what is loaded; a transcript that grew since is picked up in the totals.
        records: [...this.detail.records, ...r.records]
      };
    } catch (e) {
      if (seq === this.openSeq) this.detailError = msg(e);
    } finally {
      this.loadingMore = false;
    }
  }

  /** Load every remaining record (for metrics and export). */
  async loadAll(): Promise<void> {
    let guard = 0;
    while (this.detail && !this.complete && !this.detailError && guard++ < 1000) {
      const before = this.detail.records.length;
      await this.loadMore(500);
      if (this.detail && this.detail.records.length === before) break; // no progress
    }
  }

  /** Load through record `index` and focus it. */
  async jump(index: number): Promise<void> {
    while (this.detail && this.detail.records.length <= index && !this.complete) {
      const before = this.detail.records.length;
      await this.loadMore(Math.max(this.page, index + 1 - before));
      if (this.detail.records.length === before) break;
    }
    this.setFocus(index);
  }

  /** Timeline item that shows the focused record, if loaded. */
  get focusKey(): string | null {
    return this.focus === null ? null : keyForRecord(this.timeline, this.focus);
  }

  async markdown(): Promise<string> {
    await this.loadAll();
    return this.detail ? toMarkdown(this.detail.meta, this.detail.records) : '';
  }
}

function fromRead(r: ReadResult): Detail {
  return {
    meta: r.meta,
    records: r.records,
    total: r.total,
    skippedLines: r.skippedLines
  };
}
