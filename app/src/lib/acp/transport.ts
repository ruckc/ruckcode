import { ndJsonStream, type Stream } from '@agentclientprotocol/sdk';

/**
 * One newline-delimited JSON-RPC message per call; no trailing newline. Every agent host
 * (in-process, subprocess, WSL2 later, or a scripted fake) implements this one interface.
 */
export interface LineTransport {
  send(line: string): void;
  /** Subscribe to lines from the agent. Returns an unsubscribe function. */
  onLine(cb: (line: string) => void): () => void;
  /**
   * Subscribe to the connection ending unexpectedly (agent exit, broken pipe). Optional. `reason`
   * is a human-readable explanation when the host knows one (exit code, stderr tail).
   */
  onClose?(cb: (reason?: string) => void): () => void;
  /** Why the connection ended, once it has (set before `onClose` callbacks run). */
  closeReason?: string;
  /** Tear down the underlying connection (optional). */
  close?(): void | Promise<void>;
}

/** Adapt a {@link LineTransport} to the ACP SDK's `Stream` via its NDJSON framing. */
export function transportToStream(transport: LineTransport): Stream {
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  let unsubscribe: (() => void) | undefined;
  let unsubscribeClose: (() => void) | undefined;

  const input = new ReadableStream<Uint8Array>({
    start(controller) {
      unsubscribeClose = transport.onClose?.(() => {
        try {
          controller.close();
        } catch {
          // already closed
        }
      });
      unsubscribe = transport.onLine((line) => {
        try {
          controller.enqueue(encoder.encode(line + '\n'));
        } catch {
          // stream already closed or cancelled
        }
      });
    },
    cancel() {
      unsubscribe?.();
      unsubscribeClose?.();
    }
  });

  let pending = '';
  const output = new WritableStream<Uint8Array>({
    write(chunk) {
      pending += decoder.decode(chunk, { stream: true });
      let nl: number;
      while ((nl = pending.indexOf('\n')) >= 0) {
        const line = pending.slice(0, nl);
        pending = pending.slice(nl + 1);
        if (line.trim()) transport.send(line);
      }
    },
    close() {
      unsubscribe?.();
    }
  });

  return ndJsonStream(output, input);
}
