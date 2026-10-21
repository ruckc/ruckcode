/**
 * Transcript browser: types (mirroring the JSON of `ruckcode_core::transcript_api`, camelCase) and
 * the API the UI uses. Implementations: Tauri commands (local core, or `rc sessions --json` inside
 * WSL) and a seeded in-memory fake for browser mock mode.
 */

export interface SessionItem {
  id: string;
  createdAt: string;
  updatedAt: string;
  firstPrompt: string;
  mode: string;
  model: string;
  turns: number;
  toolCalls: number;
  inputTokens: number;
  outputTokens: number;
  durationMs: number;
  sizeBytes: number;
  recordCount: number;
  /** Lines that could not be read (truncated tail, corruption). */
  skippedLines: number;
}

export interface TranscriptMeta extends SessionItem {
  cwd: string;
  profile: string;
  ruckcodeVersion: string;
}

export type Block =
  | { type: 'text'; text: string }
  | { type: 'thinking'; thinking: string }
  | { type: 'redacted_thinking' }
  | { type: 'tool_use'; id: string; name: string; input: unknown }
  | { type: 'tool_result'; toolUseId: string; content: string; isError: boolean };

export type UserSource = 'user' | 'fix_turn' | 'hook' | 'tool_results';
export type Permission = 'allow' | 'allow_always' | 'ask_approved' | 'denied' | 'rejected';

interface Base {
  index: number;
  ts: string;
}

export interface RequestMetrics {
  durationMs: number;
  timeToFirstTokenMs: number | null;
  outputTokensPerSec: number | null;
}

export type TranscriptRecord = Base &
  (
    | {
        type: 'session_start';
        id: string;
        cwd: string;
        createdAt: string;
        ruckcodeVersion: string;
        profile: string;
        model: string;
        mode: string;
      }
    | { type: 'user_message'; source: UserSource; content: Block[] }
    | {
        type: 'assistant_message';
        requestId: number;
        model: string;
        stopReason: string;
        usage: { inputTokens: number; outputTokens: number };
        metrics: RequestMetrics;
        content: Block[];
      }
    | {
        type: 'tool_call';
        toolUseId: string;
        name: string;
        input: unknown;
        title: string;
        kind: string;
      }
    | {
        type: 'tool_result';
        toolUseId: string;
        name: string;
        output: string;
        isError: boolean;
        permission: Permission;
        durationMs: number;
        /** Unified diffs. */
        edits: { path: string; diff: string }[];
      }
    | {
        type: 'hook_run';
        event: string;
        command: string;
        success: boolean;
        exitCode: number | null;
        output: string;
        durationMs: number;
      }
    | {
        type: 'mode_change';
        from: string;
        to: string;
        reason: string;
        initiatedBy: 'user' | 'agent';
      }
    | { type: 'model_change'; from: string | null; to: string | null }
    | { type: 'compaction'; summary: string; replacedMessages: number }
    | {
        type: 'turn_end';
        stopReason: string;
        durationMs: number;
        modelRequests: number;
        toolCalls: number;
        inputTokens: number;
        outputTokens: number;
      }
    | { type: 'session_resumed' }
    | {
        type: 'mcp_servers';
        servers: { name: string; ok: boolean; tools: number; error: string | null }[];
      }
  );

export type RecordType = TranscriptRecord['type'];

export interface ReadResult {
  meta: TranscriptMeta;
  records: TranscriptRecord[];
  total: number;
  offset: number;
  limit: number;
  skippedLines: number;
}

export interface SearchHit {
  id: string;
  recordIndex: number;
  type: RecordType;
  snippet: string;
}

/** The project a request is about: a local path, or a Linux path inside `distro`. */
export interface TranscriptTarget {
  project: string;
  distro?: string;
}

export interface TranscriptsApi {
  list(t: TranscriptTarget, query?: string): Promise<SessionItem[]>;
  read(
    t: TranscriptTarget,
    id: string,
    opts?: { offset?: number; limit?: number }
  ): Promise<ReadResult>;
  search(t: TranscriptTarget, text: string, limit?: number): Promise<SearchHit[]>;
}
