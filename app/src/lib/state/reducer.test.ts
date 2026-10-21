import { describe, expect, it } from 'vitest';
import { applyUpdate, emptyModel, pushTurnEnd, viewConfigOptions } from './reducer';

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
    expect(m.plan).toEqual([{ content: 'a', status: 'pending', priority: 'high' }]);
    expect(m.items[0]).toMatchObject({ kind: 'notice', severity: 'warning' });
    expect(m.config).toEqual({ model: 'mock' });
  });
});

describe('applyUpdate: tools, history and options', () => {
  it('keeps rawInput, locations and diffs, and replaces them only when present', () => {
    const m = emptyModel();
    applyUpdate(m, {
      sessionUpdate: 'tool_call',
      toolCallId: 'e1',
      title: 'Edit a.rs',
      kind: 'edit',
      rawInput: { file_path: 'a.rs' },
      locations: [{ path: 'a.rs', line: 3 }]
    });
    expect(m.items[0]).toMatchObject({
      rawInput: { file_path: 'a.rs' },
      locations: [{ path: 'a.rs', line: 3 }],
      diffs: [],
      status: 'pending'
    });
    applyUpdate(m, {
      sessionUpdate: 'tool_call_update',
      toolCallId: 'e1',
      status: 'completed',
      content: [
        { type: 'diff', path: 'a.rs', oldText: 'x', newText: 'y' },
        { type: 'content', content: text('ok') }
      ]
    });
    applyUpdate(m, { sessionUpdate: 'tool_call_update', toolCallId: 'e1', title: 'Edit a.rs!' });
    expect(m.items[0]).toMatchObject({
      title: 'Edit a.rs!',
      output: 'ok',
      diffs: [{ path: 'a.rs', oldText: 'x', newText: 'y' }],
      rawInput: { file_path: 'a.rs' }
    });
  });

  it('records a new-file diff with null oldText and ignores updates for unknown tools', () => {
    const m = emptyModel();
    applyUpdate(m, { sessionUpdate: 'tool_call_update', toolCallId: 'nope', status: 'failed' });
    expect(m.items).toEqual([]);
    applyUpdate(m, {
      sessionUpdate: 'tool_call',
      toolCallId: 'n',
      title: 'Write',
      content: [{ type: 'diff', path: 'n.txt', newText: 'hi' }]
    });
    expect(m.items[0]).toMatchObject({ diffs: [{ oldText: null, newText: 'hi' }] });
  });

  it('merges replayed user chunks and pushes turn ends', () => {
    const m = emptyModel();
    applyUpdate(m, { sessionUpdate: 'user_message_chunk', content: text('hel') });
    applyUpdate(m, { sessionUpdate: 'user_message_chunk', content: text('lo') });
    applyUpdate(m, { sessionUpdate: 'agent_message_chunk', content: text('hi') });
    pushTurnEnd(m, 'cancelled');
    expect(m.items.map((i) => i.kind)).toEqual(['user', 'assistant', 'turn_end']);
    expect(m.items[0]).toMatchObject({ text: 'hello' });
    expect(m.items[2]).toMatchObject({ stopReason: 'cancelled' });
  });

  it('flattens grouped select options and ignores non-select ones', () => {
    const view = viewConfigOptions([
      {
        type: 'select',
        id: 'model',
        name: 'Model',
        category: 'model',
        currentValue: 'b',
        options: [
          { group: 'g', name: 'G', options: [{ value: 'a', name: 'A' }] },
          { value: 'b', name: 'B' }
        ]
      } as never,
      { type: 'boolean', id: 'x', name: 'X', currentValue: true } as never
    ]);
    expect(view).toEqual([
      {
        id: 'model',
        name: 'Model',
        category: 'model',
        currentValue: 'b',
        options: [
          { value: 'a', name: 'A', description: undefined },
          { value: 'b', name: 'B', description: undefined }
        ]
      }
    ]);
  });

  it('tracks the session title and tolerates unknown updates', () => {
    const m = emptyModel();
    applyUpdate(m, { sessionUpdate: 'session_info_update', title: 'T' });
    applyUpdate(m, { sessionUpdate: 'available_commands_update', availableCommands: [] });
    expect(m.title).toBe('T');
    expect(m.items).toEqual([]);
  });
});
