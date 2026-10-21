import type {
  Block,
  ReadResult,
  RecordType,
  SearchHit,
  SessionItem,
  TranscriptMeta,
  TranscriptRecord,
  TranscriptsApi,
  TranscriptTarget
} from './transcripts';

/** A seeded transcript: its readable records and how many lines were unreadable. */
export interface SeedSession {
  id: string;
  cwd: string;
  profile: string;
  model: string;
  mode: string;
  records: TranscriptRecord[];
  skippedLines: number;
}

type Draft = { type: RecordType } & Record<string, unknown>;

class Builder {
  records: TranscriptRecord[] = [];
  private t: number;
  constructor(start: string) {
    this.t = Date.parse(start);
  }
  /** Advance the clock and add a record. */
  add(rec: Draft, afterMs = 400): void {
    this.t += afterMs;
    this.records.push({
      index: this.records.length,
      ts: new Date(this.t).toISOString(),
      ...rec
    } as TranscriptRecord);
  }
}

const text = (t: string): Block => ({ type: 'text', text: t });

let rng = 7;
/** Deterministic pseudo-random numbers so the big transcript is the same on every load. */
const rand = (): number => {
  rng = (rng * 1664525 + 1013904223) % 4294967296;
  return rng / 4294967296;
};

interface Req {
  b: Builder;
  id: { n: number };
  model: string;
}

function assistant(
  r: Req,
  content: Block[],
  usage: [number, number],
  stop: string,
  ms: number,
  ttft: number | null
): void {
  r.b.add(
    {
      type: 'assistant_message',
      requestId: r.id.n++,
      model: r.model,
      stopReason: stop,
      usage: { inputTokens: usage[0], outputTokens: usage[1] },
      metrics: {
        durationMs: ms,
        timeToFirstTokenMs: ttft,
        outputTokensPerSec: ttft === null ? null : Math.round((usage[1] / (ms / 1000)) * 10) / 10
      },
      content
    },
    ms
  );
}

const AUTH_DIFF = `--- a/src/auth.rs
+++ b/src/auth.rs
@@ -38,7 +38,10 @@ impl Session {
 fn refresh(token: &str) -> bool {
-    true
+    if token.is_empty() {
+        return false;
+    }
+    verify_signature(token)
 }
 
 fn login(user: &str, pass: &str) -> Result<Session> {
`;

function authSession(): SeedSession {
  const b = new Builder('2026-03-01T10:15:00.000Z');
  const r: Req = { b, id: { n: 1 }, model: 'mock-large' };
  b.add({
    type: 'session_start',
    id: '20260301-101500-a1b2c3',
    cwd: '/demo/project',
    createdAt: '2026-03-01T10:15:00.000Z',
    ruckcodeVersion: '0.1.0',
    profile: 'anthropic',
    model: 'mock-large',
    mode: 'explore'
  });
  b.add({
    type: 'mcp_servers',
    servers: [
      { name: 'filesystem', ok: true, tools: 4, error: null },
      { name: 'github', ok: false, tools: 0, error: 'spawn github-mcp: No such file or directory' }
    ]
  });
  b.add({
    type: 'user_message',
    source: 'user',
    content: [text('Fix the login bug in src/auth.rs: refresh() always returns true.')]
  });
  assistant(
    r,
    [
      {
        type: 'thinking',
        thinking: 'The failing path is probably the token refresh. Let me read the file first.'
      },
      text('I will start by reading `src/auth.rs`.'),
      { type: 'tool_use', id: 'tu1', name: 'Read', input: { file_path: 'src/auth.rs' } }
    ],
    [1200, 64],
    'tool_use',
    1400,
    310
  );
  b.add({
    type: 'tool_call',
    toolUseId: 'tu1',
    name: 'Read',
    input: { file_path: 'src/auth.rs' },
    title: 'Read src/auth.rs',
    kind: 'read'
  });
  b.add(
    {
      type: 'tool_result',
      toolUseId: 'tu1',
      name: 'Read',
      output: 'fn refresh(token: &str) -> bool {\n    true\n}\n',
      isError: false,
      permission: 'allow',
      durationMs: 6,
      edits: []
    },
    6
  );
  b.add({
    type: 'user_message',
    source: 'tool_results',
    content: [
      {
        type: 'tool_result',
        toolUseId: 'tu1',
        content: 'fn refresh(token: &str) -> bool {\n    true\n}\n',
        isError: false
      }
    ]
  });
  b.add({
    type: 'mode_change',
    from: 'explore',
    to: 'implement',
    reason: 'The bug is understood, switching to implement to edit the file.',
    initiatedBy: 'agent'
  });
  assistant(
    r,
    [
      text('`refresh` never validates the token. I will make it check the signature.'),
      {
        type: 'tool_use',
        id: 'tu2',
        name: 'Edit',
        input: {
          file_path: 'src/auth.rs',
          old_string: '    true',
          new_string: '    verify_signature(token)'
        }
      }
    ],
    [1500, 120],
    'tool_use',
    2100,
    280
  );
  b.add({
    type: 'tool_call',
    toolUseId: 'tu2',
    name: 'Edit',
    input: {
      file_path: 'src/auth.rs',
      old_string: '    true',
      new_string: '    verify_signature(token)'
    },
    title: 'Edit src/auth.rs',
    kind: 'edit'
  });
  b.add(
    {
      type: 'tool_result',
      toolUseId: 'tu2',
      name: 'Edit',
      output: 'Edited src/auth.rs',
      isError: false,
      permission: 'ask_approved',
      durationMs: 14,
      edits: [{ path: 'src/auth.rs', diff: AUTH_DIFF }]
    },
    3200
  );
  b.add({
    type: 'hook_run',
    event: 'post_tool',
    command: 'cargo fmt --check',
    success: true,
    exitCode: 0,
    output: '',
    durationMs: 210
  });
  b.add({
    type: 'hook_run',
    event: 'post_tool',
    command: 'cargo clippy -- -D warnings',
    success: false,
    exitCode: 101,
    output: 'error: unused variable: `pass`\n --> src/auth.rs:44:21\n',
    durationMs: 4800
  });
  b.add({
    type: 'user_message',
    source: 'hook',
    content: [
      text('Hook `cargo clippy -- -D warnings` failed (exit 101):\nerror: unused variable: `pass`')
    ]
  });
  b.add({
    type: 'model_change',
    from: null,
    to: 'mock-small'
  });
  r.model = 'mock-small';
  assistant(
    r,
    [
      { type: 'thinking', thinking: 'Clippy complains about an unused parameter; prefix it.' },
      text('Fixing the warning.'),
      {
        type: 'tool_use',
        id: 'tu3',
        name: 'Bash',
        input: { command: 'sed -i s/pass/_pass/ src/auth.rs' }
      }
    ],
    [1700, 40],
    'tool_use',
    900,
    150
  );
  b.add({
    type: 'tool_call',
    toolUseId: 'tu3',
    name: 'Bash',
    input: { command: 'sed -i s/pass/_pass/ src/auth.rs' },
    title: 'Run sed -i s/pass/_pass/ src/auth.rs',
    kind: 'execute'
  });
  b.add(
    {
      type: 'tool_result',
      toolUseId: 'tu3',
      name: 'Bash',
      output: 'Command not allowed in mode implement: sed -i',
      isError: true,
      permission: 'denied',
      durationMs: 0,
      edits: []
    },
    2
  );
  b.add({
    type: 'compaction',
    summary:
      'The user asked to fix `refresh()` in src/auth.rs. It now verifies the signature; clippy flagged an unused parameter.',
    replacedMessages: 6
  });
  assistant(
    r,
    [text('Done: `refresh` now verifies the token signature and rejects empty tokens.')],
    [900, 30],
    'end_turn',
    700,
    120
  );
  b.add({
    type: 'turn_end',
    stopReason: 'end_turn',
    durationMs: 17000,
    modelRequests: 4,
    toolCalls: 3,
    inputTokens: 5300,
    outputTokens: 254
  });
  b.add({ type: 'session_resumed' }, 3_600_000);
  b.add({
    type: 'user_message',
    source: 'user',
    content: [text('Please also add a unit test for the empty token case.')]
  });
  assistant(
    r,
    [
      text(
        'Added a test:\n\n```rust\n#[test]\nfn empty_token_is_rejected() {\n    assert!(!refresh(""));\n}\n```\n'
      )
    ],
    [2100, 90],
    'end_turn',
    1300,
    260
  );
  b.add({
    type: 'turn_end',
    stopReason: 'end_turn',
    durationMs: 1400,
    modelRequests: 1,
    toolCalls: 0,
    inputTokens: 2100,
    outputTokens: 90
  });
  return {
    id: '20260301-101500-a1b2c3',
    cwd: '/demo/project',
    profile: 'anthropic',
    model: 'mock-large',
    mode: 'explore',
    records: b.records,
    skippedLines: 0
  };
}

function planSession(): SeedSession {
  const b = new Builder('2026-03-02T14:30:00.000Z');
  const r: Req = { b, id: { n: 1 }, model: 'mock-reasoning' };
  b.add({
    type: 'session_start',
    id: '20260302-143000-b2c3d4',
    cwd: '/demo/project',
    createdAt: '2026-03-02T14:30:00.000Z',
    ruckcodeVersion: '0.1.0',
    profile: 'anthropic',
    model: 'mock-reasoning',
    mode: 'plan'
  });
  b.add({
    type: 'user_message',
    source: 'user',
    content: [text('Plan the database migration from SQLite to Postgres.')]
  });
  assistant(
    r,
    [
      {
        type: 'thinking',
        thinking:
          'Constraints: zero downtime is not required. Steps: schema export, data copy, dual-run, cutover.\n\nRisks: type affinity differences, autoincrement semantics.'
      },
      text(
        '## Migration plan\n\n1. **Export** the schema with `sqlite3 .schema`.\n2. **Translate** types (`INTEGER PRIMARY KEY` to `BIGSERIAL`).\n3. **Copy** data in batches.\n4. **Dual-run** reads for a week.\n\n| Step | Risk |\n|------|------|\n| Types | affinity differences |\n| Copy | long locks |\n'
      )
    ],
    [980, 410],
    'end_turn',
    5200,
    640
  );
  b.add({
    type: 'turn_end',
    stopReason: 'end_turn',
    durationMs: 5300,
    modelRequests: 1,
    toolCalls: 0,
    inputTokens: 980,
    outputTokens: 410
  });
  b.add({
    type: 'mode_change',
    from: 'plan',
    to: 'plan',
    reason: 'Plan re-confirmed by the user.',
    initiatedBy: 'user'
  });
  b.add({
    type: 'user_message',
    source: 'user',
    content: [text('Looks good. Add a rollback step.')]
  });
  assistant(
    r,
    [text('Added step 5: keep the SQLite file read-only for 30 days as a rollback path.')],
    [1500, 60],
    'end_turn',
    1100,
    230
  );
  b.add({
    type: 'turn_end',
    stopReason: 'end_turn',
    durationMs: 1200,
    modelRequests: 1,
    toolCalls: 0,
    inputTokens: 1500,
    outputTokens: 60
  });
  return {
    id: '20260302-143000-b2c3d4',
    cwd: '/demo/project',
    profile: 'anthropic',
    model: 'mock-reasoning',
    mode: 'plan',
    records: b.records,
    skippedLines: 0
  };
}

function exploreSession(): SeedSession {
  const b = new Builder('2026-03-03T09:00:00.000Z');
  const r: Req = { b, id: { n: 1 }, model: 'mock-small' };
  b.add({
    type: 'session_start',
    id: '20260303-090000-c3d4e5',
    cwd: '/demo/project',
    createdAt: '2026-03-03T09:00:00.000Z',
    ruckcodeVersion: '0.1.0',
    profile: 'anthropic',
    model: 'mock-small',
    mode: 'explore'
  });
  b.add({
    type: 'user_message',
    source: 'user',
    content: [text('Where is the config loaded?')]
  });
  assistant(
    r,
    [{ type: 'tool_use', id: 'g1', name: 'Grep', input: { pattern: 'fn load', path: 'src' } }],
    [600, 25],
    'tool_use',
    500,
    90
  );
  b.add({
    type: 'tool_call',
    toolUseId: 'g1',
    name: 'Grep',
    input: { pattern: 'fn load', path: 'src' },
    title: 'Search "fn load"',
    kind: 'search'
  });
  b.add(
    {
      type: 'tool_result',
      toolUseId: 'g1',
      name: 'Grep',
      output:
        'src/config.rs:12:pub fn load(project: &Path) -> Result<Config>\nsrc/keys.rs:30:fn load_all()',
      isError: false,
      permission: 'allow',
      durationMs: 31,
      edits: []
    },
    31
  );
  assistant(
    r,
    [text('Configuration is loaded in `Config::load` (`src/config.rs:12`).')],
    [800, 28],
    'end_turn',
    450,
    80
  );
  b.add({
    type: 'turn_end',
    stopReason: 'end_turn',
    durationMs: 1100,
    modelRequests: 2,
    toolCalls: 1,
    inputTokens: 1400,
    outputTokens: 53
  });
  return {
    id: '20260303-090000-c3d4e5',
    cwd: '/demo/project',
    profile: 'anthropic',
    model: 'mock-small',
    mode: 'explore',
    records: b.records,
    skippedLines: 0
  };
}

/** About 2,000 records: 95 turns of six tool steps each. */
function largeSession(): SeedSession {
  rng = 7;
  const b = new Builder('2026-03-04T12:00:00.000Z');
  const r: Req = { b, id: { n: 1 }, model: 'mock-large' };
  const tools = [
    ['Read', 'read', 'Read src/parser/mod.rs'],
    ['Grep', 'search', 'Search "Token"'],
    ['Edit', 'edit', 'Edit src/parser/lexer.rs'],
    ['Bash', 'execute', 'Run cargo test -p parser']
  ] as const;
  b.add({
    type: 'session_start',
    id: '20260304-120000-d4e5f6',
    cwd: '/demo/project',
    createdAt: '2026-03-04T12:00:00.000Z',
    ruckcodeVersion: '0.1.0',
    profile: 'anthropic',
    model: 'mock-large',
    mode: 'implement'
  });
  for (let turn = 1; turn <= 95; turn++) {
    b.add({
      type: 'user_message',
      source: 'user',
      content: [text(`Refactor step ${turn}: split the parser module further (part ${turn}).`)]
    });
    let inTok = 0;
    let outTok = 0;
    let ms = 0;
    for (let s = 0; s < 6; s++) {
      const [name, kind, title] = tools[(turn + s) % tools.length];
      const id = `t${turn}-${s}`;
      const input = { file_path: `src/parser/part${turn}.rs`, step: s };
      const dur = 600 + Math.round(rand() * 2400);
      const usage: [number, number] = [
        800 + Math.round(rand() * 3000),
        30 + Math.round(rand() * 300)
      ];
      inTok += usage[0];
      outTok += usage[1];
      ms += dur;
      assistant(
        r,
        [text(`Step ${s + 1} of turn ${turn}.`), { type: 'tool_use', id, name, input }],
        usage,
        'tool_use',
        dur,
        Math.round(80 + rand() * 500)
      );
      b.add({
        type: 'tool_call',
        toolUseId: id,
        name,
        input,
        title: `${title} (${turn}.${s})`,
        kind
      });
      b.add(
        {
          type: 'tool_result',
          toolUseId: id,
          name,
          output: `ok ${turn}.${s}`,
          isError: false,
          permission: 'allow',
          durationMs: Math.round(rand() * 80),
          edits: []
        },
        20
      );
    }
    // A distinctive word to search for deep in the transcript.
    assistant(
      r,
      [text(turn === 90 ? 'All done; the zebra refactor landed.' : `Turn ${turn} complete.`)],
      [1000, 50],
      'end_turn',
      500,
      100
    );
    inTok += 1000;
    outTok += 50;
    ms += 500;
    b.add({
      type: 'turn_end',
      stopReason: 'end_turn',
      durationMs: ms,
      modelRequests: 7,
      toolCalls: 6,
      inputTokens: inTok,
      outputTokens: outTok
    });
  }
  return {
    id: '20260304-120000-d4e5f6',
    cwd: '/demo/project',
    profile: 'anthropic',
    model: 'mock-large',
    mode: 'implement',
    records: b.records,
    skippedLines: 0
  };
}

function corruptSession(): SeedSession {
  const b = new Builder('2026-03-05T16:00:00.000Z');
  const r: Req = { b, id: { n: 1 }, model: 'mock-small' };
  b.add({
    type: 'session_start',
    id: '20260305-160000-e5f6a7',
    cwd: '/demo/project',
    createdAt: '2026-03-05T16:00:00.000Z',
    ruckcodeVersion: '0.1.0',
    profile: 'anthropic',
    model: 'mock-small',
    mode: 'implement'
  });
  b.add({
    type: 'user_message',
    source: 'user',
    content: [text('Rename the helper to parse_header.')]
  });
  assistant(r, [text('Renaming now.')], [500, 12], 'end_turn', 300, 70);
  // A crash cut the last line: it is skipped and counted.
  return {
    id: '20260305-160000-e5f6a7',
    cwd: '/demo/project',
    profile: 'anthropic',
    model: 'mock-small',
    mode: 'implement',
    records: b.records,
    skippedLines: 1
  };
}

let seeds: SeedSession[] | undefined;
export function seedSessions(): SeedSession[] {
  seeds ??= [authSession(), planSession(), exploreSession(), largeSession(), corruptSession()];
  return seeds;
}

/** First user prompt of a seeded session (the fake agent resumes these). */
export function seedFirstPrompt(id: string): string | null {
  const s = seedSessions().find((x) => x.id === id);
  if (!s) return null;
  return firstPrompt(s.records);
}

function firstPrompt(records: TranscriptRecord[]): string {
  for (const r of records) {
    if (r.type === 'user_message' && r.source === 'user') {
      return r.content.map((b) => (b.type === 'text' ? b.text : '')).join('');
    }
  }
  return '';
}

function summarize(s: SeedSession): TranscriptMeta {
  let mode = s.mode;
  let model = s.model;
  let turns = 0;
  let toolCalls = 0;
  let inTok = 0;
  let outTok = 0;
  let dur = 0;
  for (const r of s.records) {
    if (r.type === 'mode_change') mode = r.to;
    else if (r.type === 'model_change') model = r.to ?? s.model;
    else if (r.type === 'tool_call') toolCalls++;
    else if (r.type === 'assistant_message') {
      inTok += r.usage.inputTokens;
      outTok += r.usage.outputTokens;
    } else if (r.type === 'turn_end') {
      turns++;
      dur += r.durationMs;
    }
  }
  return {
    id: s.id,
    createdAt: s.records[0]?.ts ?? '',
    updatedAt: s.records.at(-1)?.ts ?? '',
    firstPrompt: firstPrompt(s.records).slice(0, 300),
    mode,
    model,
    turns,
    toolCalls,
    inputTokens: inTok,
    outputTokens: outTok,
    durationMs: dur,
    sizeBytes: JSON.stringify(s.records).length,
    recordCount: s.records.length,
    skippedLines: s.skippedLines,
    cwd: s.cwd,
    profile: s.profile,
    ruckcodeVersion: '0.1.0'
  };
}

function searchable(r: TranscriptRecord): string[] {
  const blockTexts = (c: Block[]): string[] =>
    c.flatMap((b) =>
      b.type === 'text'
        ? [b.text]
        : b.type === 'thinking'
          ? [b.thinking]
          : b.type === 'tool_use'
            ? [b.name, JSON.stringify(b.input)]
            : b.type === 'tool_result'
              ? [b.content]
              : []
    );
  switch (r.type) {
    case 'user_message':
    case 'assistant_message':
      return blockTexts(r.content);
    case 'tool_call':
      return [r.name, r.title, r.kind, JSON.stringify(r.input)];
    case 'tool_result':
      return [r.name, r.output, ...r.edits.flatMap((e) => [e.path, e.diff])];
    case 'hook_run':
      return [r.event, r.command, r.output];
    case 'compaction':
      return [r.summary];
    case 'mode_change':
      return [r.from, r.to, r.reason];
    case 'mcp_servers':
      return r.servers.flatMap((s) => [s.name, s.error ?? '']);
    default:
      return [];
  }
}

function snippet(text: string, needle: string): string | null {
  const at = text.toLowerCase().indexOf(needle);
  if (at < 0) return null;
  const start = Math.max(0, at - 60);
  const end = Math.min(text.length, at + needle.length + 100);
  const body = text.slice(start, end).split(/\s+/).filter(Boolean).join(' ');
  return `${start > 0 ? '…' : ''}${body}${end < text.length ? '…' : ''}`;
}

export interface FakeTranscriptsOptions {
  delayMs?: number;
}

/** In-memory transcripts for browser mock mode; mirrors `ruckcode_core::transcript_api`. */
export class FakeTranscripts implements TranscriptsApi {
  /** Every call as `op:project[@distro]`, for tests. */
  calls: string[] = [];
  private delayMs: number;

  constructor(opts: FakeTranscriptsOptions = {}) {
    this.delayMs = opts.delayMs ?? 60;
  }

  private async tick(op: string, t: TranscriptTarget): Promise<void> {
    this.calls.push(`${op}:${t.project}${t.distro ? `@${t.distro}` : ''}`);
    const w = globalThis as unknown as { __ruckcodeTranscriptCalls?: unknown[] };
    if (typeof window !== 'undefined') {
      (w.__ruckcodeTranscriptCalls ??= []).push({
        op,
        project: t.project,
        distro: t.distro ?? null
      });
    }
    if (this.delayMs > 0) await new Promise((r) => setTimeout(r, this.delayMs));
  }

  async list(t: TranscriptTarget, query?: string): Promise<SessionItem[]> {
    await this.tick('list', t);
    const q = query?.trim().toLowerCase();
    return seedSessions()
      .map((s) => {
        const { cwd: _cwd, profile: _profile, ruckcodeVersion: _v, ...item } = summarize(s);
        void [_cwd, _profile, _v];
        return item;
      })
      .filter(
        (s) => !q || [s.id, s.firstPrompt, s.mode, s.model].some((h) => h.toLowerCase().includes(q))
      )
      .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }

  async read(
    t: TranscriptTarget,
    id: string,
    opts: { offset?: number; limit?: number } = {}
  ): Promise<ReadResult> {
    await this.tick('read', t);
    if (!/^[A-Za-z0-9_-]{1,128}$/.test(id)) throw new Error(`invalid session id \`${id}\``);
    const s = seedSessions().find((x) => x.id === id);
    if (!s) throw new Error(`session not found: ${id}`);
    const offset = opts.offset ?? 0;
    const limit = Math.min(Math.max(opts.limit ?? 500, 1), 5000);
    return {
      meta: summarize(s),
      records: s.records.slice(offset, offset + limit),
      total: s.records.length,
      offset,
      limit,
      skippedLines: s.skippedLines
    };
  }

  async search(t: TranscriptTarget, text: string, limit = 50): Promise<SearchHit[]> {
    await this.tick('search', t);
    const needle = text.trim().toLowerCase();
    const hits: SearchHit[] = [];
    if (!needle) return hits;
    const sessions = [...seedSessions()].sort((a, b) => b.id.localeCompare(a.id));
    for (const s of sessions) {
      for (const r of s.records) {
        if (hits.length >= limit) return hits;
        for (const t of searchable(r)) {
          const sn = snippet(t, needle);
          if (sn) {
            hits.push({ id: s.id, recordIndex: r.index, type: r.type, snippet: sn });
            break;
          }
        }
      }
    }
    return hits;
  }
}
