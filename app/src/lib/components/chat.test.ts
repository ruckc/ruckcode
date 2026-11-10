// @vitest-environment jsdom
import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { ToolItem } from '../state/reducer';
import ChatView from './ChatView.svelte';
import Composer from './Composer.svelte';
import { Markdown } from '@ruckc/truss-markdown';
import DiffPanel from './DiffPanel.svelte';
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
    expect(screen.queryByLabelText('Output')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: /Run tests/ }));
    expect(screen.getByText('src/a.rs:4')).toBeInTheDocument();
    const out = screen.getByLabelText('Output');
    expect(out.textContent).toContain('line 11');
    expect(out.textContent).not.toContain('line 12');
    await fireEvent.click(screen.getByRole('button', { name: /Show all \(18 more lines\)/ }));
    expect(out.textContent).toContain('line 29');
    expect(screen.queryByLabelText('Input')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Show input' }));
    expect(screen.getByLabelText('Input').textContent).toContain('"command": "cargo test"');
  });

  it('shows running and failed calls open, with status text', () => {
    const { unmount } = render(ToolCard, { tool: tool({ status: 'in_progress' }) });
    expect(screen.getAllByText('Running').length).toBeGreaterThan(0);
    expect(screen.getByLabelText('Output')).toBeInTheDocument();
    unmount();
    render(ToolCard, { tool: tool({ status: 'failed' }) });
    expect(screen.getAllByText('Failed').length).toBeGreaterThan(0);
  });

  it('keeps the status and kind on the card for tests and styling', () => {
    render(ToolCard, { tool: tool({ status: 'failed' }) });
    const card = screen.getByTestId('tool-card');
    expect(card).toHaveAttribute('data-status', 'failed');
    expect(card).toHaveAttribute('data-kind', 'execute');
  });

  it('renders diffs open', () => {
    render(ToolCard, {
      tool: tool({
        output: '',
        diffs: [{ path: 'a.rs', oldText: 'a\nb\n', newText: 'a\nc\n' }]
      })
    });
    expect(screen.getByRole('group', { name: 'Diff: a.rs' })).toBeInTheDocument();
    expect(screen.getByTestId('summary')).toHaveTextContent('+1');
  });
});

describe('DiffPanel', () => {
  it('shows counts and a new-file tag', () => {
    render(DiffPanel, { diff: { path: 'n.txt', oldText: null, newText: 'x\ny\n' } });
    expect(screen.getByText('new file')).toBeInTheDocument();
    expect(screen.getByTestId('summary')).toHaveTextContent('+2');
  });

  it('renders stored unified-diff text', () => {
    render(DiffPanel, {
      patch: { path: 'a.rs', text: '--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-a\n+b\n' }
    });
    expect(screen.getByRole('group', { name: 'Diff: a.rs' })).toBeInTheDocument();
    expect(screen.queryByText('new file')).toBeNull();
  });
});

describe('Markdown (truss-markdown)', () => {
  it('renders markdown, shows script tags as text and offers a working copy button', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    render(Markdown, { text: '**hi** <script>x()</script>\n\n```ts\nconst a = 1;\n```' });
    expect(document.querySelector('strong')?.textContent).toBe('hi');
    expect(document.querySelector('script')).toBeNull();
    expect(document.body.textContent).toContain('<script>x()</script>');
    await fireEvent.click(await screen.findByRole('button', { name: 'Copy code' }));
    expect(writeText).toHaveBeenCalledWith('const a = 1;');
    await vi.waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Copied'));
  });

  it('never makes a link out of a javascript: URL', () => {
    render(Markdown, { text: '[x](javascript:alert(1)) [ok](https://example.com)' });
    const links = [...document.querySelectorAll('a')];
    expect(links.map((a) => a.getAttribute('href'))).toEqual(['https://example.com']);
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
    expect(screen.getByRole('button', { name: /Show reasoning/ })).toBeInTheDocument();
    expect(screen.getByTestId('assistant-msg')).toHaveTextContent('answer');
    const notice = document.querySelector('[data-severity="warning"]');
    expect(notice).toHaveAttribute('role', 'status');
    expect(notice).toHaveTextContent('Careful');
    expect(notice).toHaveTextContent('details');
    expect(screen.getByTestId('turn-end')).toHaveTextContent('Cancelled');
    expect(screen.getByTestId('turn-end')).toHaveTextContent('cancelled');
  });

  it('shows the rate-limit countdown with a working cancel button', async () => {
    const oncancel = vi.fn();
    render(ChatView, {
      items: [{ kind: 'user', text: 'go' }],
      running: true,
      loading: false,
      rateWait: {
        reason: 'Rate limited (tokens/minute)',
        resumeAtMs: Date.now() + 51_000,
        attempt: 1,
        maxAttempts: 6
      },
      oncancel
    });
    expect(screen.getByTestId('rate-wait')).toHaveTextContent(
      /Rate limited \(tokens\/minute\) — retrying in (50|51)s \(retry 1\/6\)/
    );
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel the wait' }));
    expect(oncancel).toHaveBeenCalled();
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
