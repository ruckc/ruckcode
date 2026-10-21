// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { FakeTranscripts } from '../host/fake-transcripts';
import TranscriptsView from './TranscriptsView.svelte';

const target = { project: '/demo/project' };

beforeEach(() => {
  window.history.replaceState({}, '', '/?mock=1');
});

function view(extra: Record<string, unknown> = {}) {
  const api = new FakeTranscripts({ delayMs: 0 });
  const onresume = vi.fn();
  render(TranscriptsView, { props: { api, target, onresume, debounceMs: 0, ...extra } });
  return { api, onresume };
}

async function openAuth() {
  await screen.findAllByTestId('session');
  await fireEvent.click(screen.getByRole('button', { name: /Fix the login bug/ }));
  await screen.findByRole('list', { name: 'Timeline' });
}

describe('TranscriptsView', () => {
  it('lists transcripts with summaries and warns about unreadable lines', async () => {
    view();
    const rows = await screen.findAllByTestId('session');
    expect(rows).toHaveLength(5);
    expect(screen.getByText('1 bad line')).toBeInTheDocument();
    expect(screen.getByTestId('count')).toHaveTextContent('5 of 5');
  });

  it('sorts and filters', async () => {
    view();
    await screen.findAllByTestId('session');
    const first = () => screen.getAllByTestId('session')[0].textContent ?? '';
    expect(first()).toContain('Rename the helper');
    await fireEvent.change(screen.getByLabelText('Sort by'), { target: { value: 'tokens' } });
    expect(first()).toContain('Refactor step 1');
    await fireEvent.change(screen.getByLabelText('Filter by mode'), { target: { value: 'plan' } });
    expect(screen.getAllByTestId('session')).toHaveLength(1);
    expect(screen.getByTestId('count')).toHaveTextContent('1 of 5');
  });

  it('shows search hits and opens the transcript at the record', async () => {
    view();
    await screen.findAllByTestId('session');
    await fireEvent.input(screen.getByLabelText('Search transcripts'), {
      target: { value: 'zebra' }
    });
    const hit = await screen.findByTestId('hit');
    expect(hit).toHaveTextContent('assistant message');
    await fireEvent.click(hit);
    await screen.findByRole('list', { name: 'Timeline' });
    await waitFor(() => expect(document.querySelector('.item.focused')).toBeTruthy());
    expect(document.querySelector('.item.focused')).toHaveTextContent('zebra refactor');
  });

  it('renders the timeline: messages, tool cards with diffs and permission, hooks and more', async () => {
    view();
    await openAuth();
    const tl = screen.getByRole('list', { name: 'Timeline' });
    expect(within(tl).getByText('Session started')).toBeInTheDocument();
    expect(within(tl).getByText('MCP servers')).toBeInTheDocument();
    expect(within(tl).getByText(/failed: spawn github-mcp/)).toBeInTheDocument();
    expect(within(tl).getAllByText('Thinking', { selector: 'summary' })).toHaveLength(2);
    expect(within(tl).getByText(/Fix the login bug in src\/auth.rs/)).toBeInTheDocument();
    // The edit card starts open: diff, permission, output.
    const edit = within(tl).getByRole('region', { name: 'Tool Edit src/auth.rs' });
    expect(within(edit).getByRole('group', { name: 'Diff of src/auth.rs' })).toBeInTheDocument();
    expect(within(edit).getByTestId('permission')).toHaveTextContent('approved by you');
    expect(within(edit).getByLabelText('Tool output')).toHaveTextContent('Edited src/auth.rs');
    // The denied Bash call is an error card (open), the quiet Read card starts closed.
    const bash = within(tl).getByRole('region', { name: /Tool Run sed/ });
    expect(within(bash).getByTestId('permission')).toHaveTextContent('denied by policy');
    const read = within(tl).getByRole('region', { name: 'Tool Read src/auth.rs' });
    expect(within(read).queryByLabelText('Tool output')).toBeNull();
    await fireEvent.click(within(read).getByRole('button', { name: /Read src\/auth.rs/ }));
    expect(within(read).getByLabelText('Tool output')).toBeInTheDocument();
    await fireEvent.click(within(read).getByRole('button', { name: 'Show input' }));
    expect(within(read).getByLabelText('Tool input')).toHaveTextContent('"file_path"');
    // Hooks, mode and model changes, compaction, turn ends, resume marker.
    expect(within(tl).getAllByRole('region', { name: 'Hook post_tool' })).toHaveLength(2);
    expect(
      within(tl).getByText(/failed \(exit 101\)/, { selector: '.status' })
    ).toBeInTheDocument();
    expect(within(tl).getAllByText(/unused variable/).length).toBeGreaterThan(0);
    expect(within(tl).getByText('Mode', { selector: '.chip' })).toBeInTheDocument();
    expect(within(tl).getByText('mock-small', { selector: 'code' })).toBeInTheDocument();
    expect(within(tl).getByRole('region', { name: 'Compaction' })).toBeInTheDocument();
    expect(within(tl).getAllByLabelText('Turn summary')).toHaveLength(2);
    expect(within(tl).getByText('Session resumed')).toBeInTheDocument();
    // Per-request metrics under each assistant message.
    expect(within(tl).getAllByLabelText('Request metrics')[0]).toHaveTextContent('TTFT');
  });

  it('pages a large transcript with Load more', async () => {
    view();
    await screen.findAllByTestId('session');
    await fireEvent.click(screen.getByRole('button', { name: /Refactor step 1/ }));
    await screen.findByRole('list', { name: 'Timeline' });
    expect(screen.getByTestId('loaded')).toHaveTextContent(/^200 of 1,9\d\d records loaded/);
    await fireEvent.click(screen.getByRole('button', { name: /Load more/ }));
    await waitFor(() => expect(screen.getByTestId('loaded')).toHaveTextContent(/^400 of/));
  });

  it('shows metrics with charts and accessible tables', async () => {
    view();
    await openAuth();
    await fireEvent.click(screen.getByRole('tab', { name: 'Metrics' }));
    const m = await screen.findByTestId('metrics');
    expect(within(m).getByTestId('m-requests')).toHaveTextContent('5');
    expect(within(m).getByRole('img', { name: /Bar chart of 5 request durations/ })).toBeVisible();
    expect(within(m).getByRole('img', { name: /cumulative input tokens/i })).toBeInTheDocument();
    expect(within(m).getByRole('img', { name: /cumulative output tokens/i })).toBeInTheDocument();
    expect(within(m).getByRole('table', { name: 'Per-turn totals' })).toBeInTheDocument();
    expect(within(m).getAllByRole('row').length).toBeGreaterThan(5);
  });

  it('loads every record for metrics of a large transcript', async () => {
    view();
    await screen.findAllByTestId('session');
    await fireEvent.click(screen.getByRole('button', { name: /Refactor step 1/ }));
    await screen.findByRole('list', { name: 'Timeline' });
    await fireEvent.click(screen.getByRole('tab', { name: 'Metrics' }));
    await waitFor(() => expect(screen.getByTestId('m-requests')).toHaveTextContent('665'), {
      timeout: 5000
    });
  });

  it('resumes the session and copies markdown', async () => {
    const { onresume } = view();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    await openAuth();
    await fireEvent.click(screen.getByRole('button', { name: 'Resume session' }));
    expect(onresume).toHaveBeenCalledWith('20260301-101500-a1b2c3');
    await fireEvent.click(screen.getByRole('button', { name: 'Copy as markdown' }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(writeText.mock.calls[0][0]).toContain('# Session 20260301-101500-a1b2c3');
    await screen.findByRole('button', { name: 'Copied' });
  });

  it('shows the skipped-lines notice for the corrupt transcript', async () => {
    view();
    await screen.findAllByTestId('session');
    await fireEvent.click(screen.getByRole('button', { name: /Rename the helper/ }));
    expect(await screen.findByTestId('skipped')).toHaveTextContent('1 unreadable line was skipped');
  });
});
