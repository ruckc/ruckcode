// @vitest-environment jsdom
import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import PlanPanel from './PlanPanel.svelte';

const e = (content: string, status: string) => ({ content, status, priority: 'high' }) as never;

describe('PlanPanel', () => {
  it('lists unfinished items with progress', () => {
    render(PlanPanel, { props: { plan: [e('a', 'completed'), e('b', 'pending')] } });
    expect(screen.getByText('b')).toBeTruthy();
    expect(screen.getByText('1/2')).toBeTruthy();
    expect(screen.queryByText(/Plan complete/)).toBeNull();
  });

  it('shows the completion confirmation instead of the list', () => {
    render(PlanPanel, { props: { plan: [], complete: 3 } });
    expect(screen.getByRole('status').textContent).toContain('Plan complete (3/3)');
  });
});
