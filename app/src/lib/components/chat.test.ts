// @vitest-environment jsdom
import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { ToolItem } from '../state/reducer';
import ChatView from './ChatView.svelte';
import Composer from './Composer.svelte';
import DiffView from './DiffView.svelte';
import Markdown from './Markdown.svelte';
import ToolCard from './ToolCard.svelte';

const tool = (over: Partial<ToolItem> = {}): ToolItem => ({
  kind: 'tool',
  id: 't',
  title: 'Run tests',
  toolKind: 'execute',
  status: 'completed',
  rawInput: { command: 'cargo test' },
  locations: [{ path: 'src/a.rs', line: 4 }],
  output: Array.from({ length: 30 }, (_, i) => `line ${i}`).join('\n'),
  diffs: [],
  ...over
});

describe('ToolCard', () => {
  it('starts collapsed when quietly completed, then truncates output and shows raw input', async () => {
    render(ToolCard, { tool: tool() });
    expect(screen.queryByLabelText('Tool output')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: /Run tests/ }));
    expect(screen.getByText('src/a.rs:4')).toBeInTheDocument();
    const out = screen.getByLabelText('Tool output');
    expect(out.textContent).toContain('line 11');
    expect(out.textContent).not.toContain('line 12');
    await fireEvent.click(screen.getByRole('button', { name: /Show all \(18 more lines\)/ }));
    expect(out.textContent).toContain('line 29');
    await fireEvent.click(screen.getByRole('button', { name: 'Show input' }));
    expect(screen.getByLabelText('Tool input').textContent).toContain('"command": "cargo test"');
  });

  it('shows running and failed calls open, with status text', () => {
    const { unmount } = render(ToolCard, { tool: tool({ status: 'in_progress' }) });
    expect(screen.getByText('running')).toBeInTheDocument();
    expect(screen.getByLabelText('Tool output')).toBeInTheDocument();
    unmount();
    render(ToolCard, { tool: tool({ status: 'failed' }) });
    expect(screen.getByText('failed')).toBeInTheDocument();
  });

  it('renders diffs and toggles side by side', async () => {
    render(ToolCard, {
      tool: tool({
        output: '',
        diffs: [{ path: 'a.rs', oldText: 'a\nb\n', newText: 'a\nc\n' }]
      })
    });
    expect(screen.getByRole('group', { name: 'Diff of a.rs' })).toBeInTheDocument();
    expect(document.querySelectorAll('tr.add')).toHaveLength(1);
    await fireEvent.click(screen.getByLabelText('side by side'));
    expect(document.querySelector('table.split')).not.toBeNull();
    expect(document.querySelectorAll('td.add')).toHaveLength(1);
    expect(document.querySelectorAll('td.del')).toHaveLength(1);
  });
});

describe('DiffView', () => {
  it('shows counts and a new-file tag', () => {
    render(DiffView, { diff: { path: 'n.txt', oldText: null, newText: 'x\ny\n' } });
    expect(screen.getByText('new file')).toBeInTheDocument();
    expect(screen.getByText('+2')).toBeInTheDocument();
    expect(screen.getByText('-0')).toBeInTheDocument();
  });
});

describe('Markdown', () => {
  it('renders sanitized markdown and adds a working copy button to code blocks', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    render(Markdown, { text: '**hi** <script>x()</script>\n\n```ts\nconst a = 1;\n```' });
    expect(document.querySelector('strong')?.textContent).toBe('hi');
    expect(document.querySelector('script')).toBeNull();
    const btn = await screen.findByRole('button', { name: 'Copy code' });
    await fireEvent.click(btn);
    expect(writeText).toHaveBeenCalledWith('const a = 1;\n');
    await vi.waitFor(() => expect(btn.textContent).toBe('Copied'));
  });
});

describe('ChatView', () => {
  it('renders each item kind distinctly', () => {
    render(ChatView, {
      items: [
        { kind: 'user', text: 'hello' },
        { kind: 'thought', text: 'hmm' },
        { kind: 'assistant', text: 'answer' },
        { kind: 'notice', severity: 'warning', title: 'Careful', description: 'details' },
        { kind: 'turn_end', stopReason: 'cancelled' }
      ],
      running: false,
      loading: false
    });
    expect(screen.getByTestId('user-msg')).toHaveTextContent('hello');
    expect(screen.getByText('Reasoning')).toBeInTheDocument();
    expect(screen.getByTestId('assistant-msg')).toHaveTextContent('answer');
    expect(screen.getByRole('status')).toHaveTextContent('Careful details');
    expect(screen.getByTestId('turn-end')).toHaveTextContent('Cancelled');
    expect(screen.getByTestId('turn-end')).toHaveTextContent('cancelled');
  });
});

function composer(over: Record<string, unknown> = {}) {
  const props = {
    disabled: false,
    running: false,
    queue: [] as string[],
    history: ['first', 'second'],
    placeholder: 'Message',
    onsend: vi.fn(),
    oncancel: vi.fn(),
    onunqueue: vi.fn(),
    ...over
  };
  render(Composer, props);
  return { props, box: screen.getByLabelText('Prompt') as HTMLTextAreaElement };
}

describe('Composer', () => {
  it('Enter sends, Shift+Enter does not', async () => {
    const { props, box } = composer();
    await fireEvent.input(box, { target: { value: 'hello' } });
    await fireEvent.keyDown(box, { key: 'Enter', shiftKey: true });
    expect(props.onsend).not.toHaveBeenCalled();
    await fireEvent.keyDown(box, { key: 'Enter' });
    expect(props.onsend).toHaveBeenCalledWith('hello');
    expect(box.value).toBe('');
  });

  it('recalls history with Up/Down and restores the draft', async () => {
    const { box } = composer();
    await fireEvent.input(box, { target: { value: 'draft' } });
    box.setSelectionRange(0, 0);
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('second');
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('first');
    await fireEvent.keyDown(box, { key: 'ArrowDown' });
    expect(box.value).toBe('second');
    await fireEvent.keyDown(box, { key: 'ArrowDown' });
    expect(box.value).toBe('draft');
  });

  it('while running: Queue label, Esc and Stop cancel, queued prompts removable', async () => {
    const { props, box } = composer({ running: true, queue: ['next one'] });
    expect(screen.getByRole('button', { name: 'Queue' })).toBeDisabled();
    await fireEvent.input(box, { target: { value: 'more' } });
    expect(screen.getByRole('button', { name: 'Queue' })).toBeEnabled();
    await fireEvent.keyDown(box, { key: 'Escape' });
    await fireEvent.click(screen.getByRole('button', { name: /Cancel the running turn/ }));
    expect(props.oncancel).toHaveBeenCalledTimes(2);
    await fireEvent.click(screen.getByRole('button', { name: 'Remove queued prompt 1' }));
    expect(props.onunqueue).toHaveBeenCalledWith(0);
  });

  it('is inert when disabled', () => {
    const { box } = composer({ disabled: true });
    expect(box).toBeDisabled();
  });
});
