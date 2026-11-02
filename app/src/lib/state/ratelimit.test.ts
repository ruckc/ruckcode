import { describe, expect, it, vi } from 'vitest';
import { FakeAgent, FakeStore } from '../host/fake-agent';
import { ChatController } from './chat.svelte';
import { formatWait, rateWaitOf, rateWaitText, stopReasonOf } from './ratelimit';

describe('rate limit countdown text', () => {
  const wait = rateWaitOf({
    sessionId: 's',
    status: 'waiting',
    reason: 'Rate limited (tokens/minute)',
    resumeAtMs: 1_051_000,
    attempt: 1,
    maxAttempts: 6
  })!;

  it('counts down and never goes negative', () => {
    expect(rateWaitText(wait, 1_000_000)).toBe(
      'Rate limited (tokens/minute) — retrying in 51s (retry 1/6)'
    );
    expect(rateWaitText(wait, 1_040_500)).toContain('retrying in 11s');
    expect(rateWaitText(wait, 9_000_000)).toContain('retrying in 0s');
  });

  it('clears on resumed and stopped', () => {
    expect(rateWaitOf({ sessionId: 's', status: 'resumed' })).toBeNull();
    expect(rateWaitOf({ sessionId: 's', status: 'stopped', message: 'x' })).toBeNull();
  });

  it('formats long waits', () => {
    expect(formatWait(310)).toBe('5m 10s');
    expect(formatWait(3600)).toBe('1h 0m');
  });

  it('reads the real stop reason from _meta', () => {
    expect(stopReasonOf({ stopReason: 'end_turn' })).toBe('end_turn');
    expect(
      stopReasonOf({ stopReason: 'end_turn', _meta: { ruckcode: { stopReason: 'rate_limited' } } })
    ).toBe('rate_limited');
  });
});

describe('rate limits in the chat controller', () => {
  async function ready() {
    const chat = new ChatController({ transport: () => new FakeAgent({ store: new FakeStore() }) });
    await chat.connect({ cwd: '/proj' });
    return chat;
  }

  it('shows a countdown while waiting, then completes the turn', async () => {
    const chat = await ready();
    const turn = chat.send('mock:ratelimit 300');
    await vi.waitFor(() => expect(chat.active!.rateWait).not.toBeNull());
    expect(chat.active!.rateWait).toMatchObject({
      reason: 'Rate limited (tokens/minute)',
      attempt: 1,
      maxAttempts: 6
    });
    await turn;
    expect(chat.active!.rateWait).toBeNull();
    expect(chat.active!.model.items.at(-1)).toMatchObject({
      kind: 'turn_end',
      stopReason: 'end_turn'
    });
  });

  it('cancel during the wait ends the turn as cancelled, not as an error', async () => {
    const chat = await ready();
    const turn = chat.send('mock:ratelimit 30000');
    await vi.waitFor(() => expect(chat.active!.rateWait).not.toBeNull());
    await chat.cancel();
    await turn;
    expect(chat.active!.rateWait).toBeNull();
    expect(chat.active!.running).toBe(false);
    const items = chat.active!.model.items;
    expect(items.at(-1)).toMatchObject({ kind: 'turn_end', stopReason: 'cancelled' });
    expect(items.some((i) => i.kind === 'notice' && i.severity === 'error')).toBe(false);
  });

  it('a daily budget ends the turn as rate_limited and the next message works', async () => {
    const chat = await ready();
    await chat.send('mock:ratelimit-daily');
    const items = chat.active!.model.items;
    expect(items.at(-1)).toMatchObject({ kind: 'turn_end', stopReason: 'rate_limited' });
    expect(items.some((i) => i.kind === 'notice' && i.title.includes('daily credit budget'))).toBe(
      true
    );
    expect(chat.active!.rateWait).toBeNull();
    await chat.send('hello');
    expect(chat.active!.model.items.at(-1)).toMatchObject({
      kind: 'turn_end',
      stopReason: 'end_turn'
    });
  });
});
