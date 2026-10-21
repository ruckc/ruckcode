import { describe, expect, it } from 'vitest';
import { applyUpdate, emptyModel } from './reducer';

const text = (t: string) => ({ type: 'text' as const, text: t });

describe('applyUpdate', () => {
  it('coalesces streamed chunks of the same kind', () => {
    const m = emptyModel();
    applyUpdate(m, { sessionUpdate: 'agent_message_chunk', content: text('he') });
    applyUpdate(m, { sessionUpdate: 'agent_message_chunk', content: text('llo') });
    applyUpdate(m, { sessionUpdate: 'agent_thought_chunk', content: text('hmm') });
    applyUpdate(m, { sessionUpdate: 'agent_message_chunk', content: text('!') });
    expect(m.items).toEqual([
      { kind: 'assistant', text: 'hello' },
      { kind: 'thought', text: 'hmm' },
      { kind: 'assistant', text: '!' }
    ]);
  });

  it('tracks tool calls; absent content in an update keeps the output', () => {
    const m = emptyModel();
    applyUpdate(m, {
      sessionUpdate: 'tool_call',
      toolCallId: 't1',
      title: 'Bash',
      content: [{ type: 'content', content: text('ls') }]
    });
    applyUpdate(m, { sessionUpdate: 'tool_call_update', toolCallId: 't1', status: 'in_progress' });
    expect(m.items[0]).toMatchObject({ status: 'in_progress', output: 'ls', toolKind: 'other' });
    applyUpdate(m, {
      sessionUpdate: 'tool_call_update',
      toolCallId: 't1',
      status: 'completed',
      content: [{ type: 'content', content: text('done') }]
    });
    expect(m.items[0]).toMatchObject({ status: 'completed', output: 'done' });
  });

  it('handles mode, usage, plan, notice and config updates', () => {
    const m = emptyModel();
    applyUpdate(m, { sessionUpdate: 'current_mode_update', currentModeId: 'design' });
    applyUpdate(m, { sessionUpdate: 'usage_update', used: 5, size: 10 });
    applyUpdate(m, {
      sessionUpdate: 'plan',
      entries: [{ content: 'a', priority: 'high', status: 'pending' }]
    });
    applyUpdate(m, { sessionUpdate: 'notice', severity: 'warning', title: 'careful' });
    applyUpdate(m, {
      sessionUpdate: 'config_option_update',
      configOptions: [
        {
          type: 'select',
          id: 'model',
          name: 'Model',
          currentValue: 'mock',
          options: [{ value: 'mock', name: 'mock' }]
        }
      ]
    });
    expect(m.currentModeId).toBe('design');
    expect(m.usage).toEqual({ used: 5, size: 10 });
    expect(m.plan).toEqual([{ content: 'a', status: 'pending' }]);
    expect(m.items[0]).toMatchObject({ kind: 'notice', severity: 'warning' });
    expect(m.config).toEqual({ model: 'mock' });
  });
});
