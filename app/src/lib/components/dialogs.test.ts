// @vitest-environment jsdom
import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { PendingPermission, PendingQuestion } from '../state/chat.svelte';
import AskDialog from './AskDialog.svelte';
import PermissionDialog from './PermissionDialog.svelte';

function permission(overrides: Record<string, unknown> = {}) {
  const resolve = vi.fn();
  const pending: PendingPermission = {
    sessionId: 's',
    resolve,
    request: {
      sessionId: 's',
      toolCall: {
        toolCallId: 't',
        title: 'Write a.txt',
        kind: 'edit',
        rawInput: { file_path: 'a.txt', content: 'hi' },
        content: [{ type: 'diff', path: 'a.txt', oldText: 'old\n', newText: 'new\n' }],
        ...overrides
      },
      options: [
        { optionId: 'allow', name: 'Allow', kind: 'allow_once' },
        { optionId: 'always', name: 'Always allow', kind: 'allow_always' },
        { optionId: 'reject', name: 'Reject', kind: 'reject_once' }
      ]
    }
  };
  return { pending, resolve };
}

describe('PermissionDialog', () => {
  it('shows title, kind, input summary and a diff preview, and focuses the dialog', () => {
    const { pending } = permission();
    render(PermissionDialog, { pending, total: 3 });
    const dlg = screen.getByRole('alertdialog');
    expect(dlg).toHaveTextContent('Write a.txt');
    expect(dlg).toHaveTextContent('file_path: a.txt');
    expect(dlg).toHaveTextContent('1 of 3 pending');
    expect(screen.getByRole('group', { name: 'Diff of a.txt' })).toBeInTheDocument();
    expect(dlg.contains(document.activeElement)).toBe(true);
  });

  it.each([
    ['y', 'allow'],
    ['a', 'always'],
    ['n', 'reject']
  ])('key %s selects option %s', async (key, optionId) => {
    const { pending, resolve } = permission();
    render(PermissionDialog, { pending });
    await fireEvent.keyDown(window, { key });
    expect(resolve).toHaveBeenCalledWith({ outcome: { outcome: 'selected', optionId } });
  });

  it('Escape cancels the turn and buttons resolve too', async () => {
    const a = permission();
    const view = render(PermissionDialog, { pending: a.pending });
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(a.resolve).toHaveBeenCalledWith({ outcome: { outcome: 'cancelled' } });
    view.unmount();
    const b = permission();
    render(PermissionDialog, { pending: b.pending });
    await fireEvent.click(screen.getByRole('button', { name: /Always allow/ }));
    expect(b.resolve).toHaveBeenCalledWith({
      outcome: { outcome: 'selected', optionId: 'always' }
    });
  });

  it('words a SwitchMode proposal clearly', async () => {
    const { pending, resolve } = permission({
      title: 'Switch mode to explore',
      kind: 'switch_mode',
      rawInput: { mode_id: 'explore', reason: 'done implementing' },
      content: undefined
    });
    render(PermissionDialog, { pending });
    expect(screen.getByRole('alertdialog')).toHaveTextContent('Mode switch proposed');
    expect(screen.getByRole('alertdialog')).toHaveTextContent('switch to mode explore');
    expect(screen.getByRole('alertdialog')).toHaveTextContent('done implementing');
    await fireEvent.click(screen.getByRole('button', { name: /Stay in current mode/ }));
    expect(resolve).toHaveBeenCalledWith({ outcome: { outcome: 'selected', optionId: 'reject' } });
    expect(screen.getByRole('button', { name: /Switch to explore/ })).toBeInTheDocument();
  });
});

function question() {
  const resolve = vi.fn();
  const pending: PendingQuestion = {
    sessionId: 's',
    resolve,
    request: {
      questions: [
        {
          question: 'Which database?',
          header: 'DB',
          multiSelect: false,
          options: [
            { label: 'sqlite (Recommended)', description: 'embedded' },
            { label: 'postgres' }
          ]
        },
        {
          question: 'Which features?',
          header: 'Features',
          multiSelect: true,
          options: [{ label: 'auth' }, { label: 'metrics' }]
        }
      ]
    }
  };
  return { pending, resolve };
}

describe('AskDialog', () => {
  it('walks the tabs, supports multi-select and Other, and submits all answers', async () => {
    const { pending, resolve } = question();
    render(AskDialog, { pending });
    expect(screen.getAllByRole('tab')).toHaveLength(2);
    expect(screen.getByText('Recommended')).toBeInTheDocument();
    const submit = screen.getByRole('button', { name: /Submit/ });
    expect(submit).toBeDisabled();

    await fireEvent.click(screen.getByLabelText(/sqlite/));
    await fireEvent.click(screen.getByRole('tab', { name: /Features/ }));
    await fireEvent.click(screen.getByLabelText(/auth/));
    await fireEvent.click(screen.getByLabelText(/metrics/));
    await fireEvent.input(screen.getByLabelText('Other answer'), { target: { value: ' search ' } });
    expect(submit).toBeEnabled();
    await fireEvent.click(submit);
    expect(resolve).toHaveBeenCalledWith({
      cancelled: false,
      answers: [
        { question: 'Which database?', selected: ['sqlite (Recommended)'], other: null },
        { question: 'Which features?', selected: ['auth', 'metrics'], other: 'search' }
      ]
    });
  });

  it('single-select: typing Other replaces the choice, and a choice clears Other', async () => {
    const { pending, resolve } = question();
    render(AskDialog, { pending });
    await fireEvent.click(screen.getByLabelText(/sqlite/));
    await fireEvent.input(screen.getByLabelText('Other answer'), { target: { value: 'duckdb' } });
    await fireEvent.click(screen.getByRole('tab', { name: /Features/ }));
    await fireEvent.click(screen.getByLabelText(/auth/));
    await fireEvent.click(screen.getByRole('button', { name: /Submit/ }));
    expect(resolve.mock.calls[0][0].answers[0]).toEqual({
      question: 'Which database?',
      selected: [],
      other: 'duckdb'
    });
  });

  it('number keys pick options, Alt+Arrow moves between questions, Escape cancels', async () => {
    const { pending, resolve } = question();
    render(AskDialog, { pending });
    const dlg = screen.getByRole('dialog');
    await fireEvent.keyDown(dlg, { key: '2' });
    expect(screen.getByLabelText(/postgres/)).toBeChecked();
    await fireEvent.keyDown(dlg, { key: 'ArrowRight', altKey: true });
    expect(screen.getByRole('tab', { name: /Features/ })).toHaveAttribute('aria-selected', 'true');
    await fireEvent.keyDown(dlg, { key: '1' });
    await fireEvent.keyDown(dlg, { key: '2' });
    expect(screen.getByLabelText(/auth/)).toBeChecked();
    expect(screen.getByLabelText(/metrics/)).toBeChecked();
    await fireEvent.keyDown(dlg, { key: 'Enter', ctrlKey: true });
    expect(resolve).toHaveBeenCalledWith(expect.objectContaining({ cancelled: false }));
    resolve.mockClear();
    await fireEvent.keyDown(dlg, { key: 'Escape' });
    expect(resolve).toHaveBeenCalledWith({ answers: [], cancelled: true });
  });
});
