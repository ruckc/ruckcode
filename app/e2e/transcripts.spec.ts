import { expect, test, type Page } from '@playwright/test';

const BIG = 'Refactor step 1';

async function openView(page: Page, extra = '') {
  await page.goto(`/?mock=1&delay=2&transcriptdelay=10${extra}`);
  await page.getByRole('button', { name: 'Transcripts', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Transcripts' })).toBeVisible();
  await expect(page.getByTestId('session')).toHaveCount(5);
}

const rows = (page: Page) => page.getByTestId('session');
const detail = (page: Page) => page.getByRole('region', { name: 'Transcript detail' });

test('lists transcripts, sorts and filters them', async ({ page }) => {
  await openView(page);
  await expect(rows(page).first()).toContainText('Rename the helper');
  await expect(rows(page).first()).toContainText('1 bad line');

  await page.getByLabel('Sort by').selectOption('tokens');
  await expect(rows(page).first()).toContainText(BIG);
  await page.getByLabel('Sort by').selectOption('created');
  await expect(rows(page).first()).toContainText('Rename the helper');
  await page.getByRole('button', { name: 'Sort descending' }).click();
  await expect(rows(page).first()).toContainText('Fix the login bug');

  await page.getByLabel('Filter by mode', { exact: true }).selectOption('plan');
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first()).toContainText('Plan the database migration');
  await page.getByLabel('Filter by mode', { exact: true }).selectOption('');
  await page.getByLabel('Filter by model').selectOption('mock-small');
  await expect(rows(page)).toHaveCount(3);
});

test('search finds text deep inside a large transcript and jumps to the record', async ({
  page
}) => {
  await openView(page);
  await page.getByLabel('Search transcripts').fill('ZEBRA');
  const hit = page.getByTestId('hit');
  await expect(hit).toHaveCount(1);
  await expect(hit).toContainText('assistant message');
  await expect(hit).toContainText('zebra refactor landed');
  await hit.click();

  // The record is far beyond the first page: it was loaded and highlighted.
  const focused = page.locator('.item.focused');
  await expect(focused).toContainText('zebra refactor landed');
  await expect(focused).toBeInViewport();
  await expect(page.getByTestId('loaded')).not.toContainText(/^200 of/);

  // Clearing the search brings the list back.
  await page.getByLabel('Search transcripts').fill('');
  await expect(rows(page)).toHaveCount(5);
  await page.getByLabel('Search transcripts').fill('nothing-matches-this');
  await expect(page.getByText('No matches.')).toBeVisible();
});

test('opens a transcript and shows every kind of record', async ({ page }) => {
  await openView(page);
  await rows(page).filter({ hasText: 'Fix the login bug' }).click();
  const tl = detail(page).getByRole('list', { name: 'Timeline' });
  await expect(tl).toBeVisible();

  await expect(tl.getByLabel('MCP servers')).toContainText('spawn github-mcp');
  await expect(tl.getByTestId('tl-user').first()).toContainText('Fix the login bug in src/auth.rs');
  await tl.getByText('Thinking', { exact: true }).first().click();
  await expect(tl.getByText('The failing path is probably the token refresh.')).toBeVisible();

  const edit = tl.getByRole('region', { name: 'Tool Edit src/auth.rs' });
  await expect(edit.getByRole('group', { name: 'Diff of src/auth.rs' })).toContainText(
    'verify_signature(token)'
  );
  await expect(edit.getByTestId('permission')).toHaveText('approved by you');
  await expect(edit).toContainText('14 ms');
  await expect(tl.getByRole('region', { name: 'Hook post_tool' }).nth(1)).toContainText(
    'failed (exit 101)'
  );
  await expect(tl.getByTestId('tl-mode')).toContainText('explore');
  await expect(tl.getByRole('region', { name: 'Compaction' })).toContainText('6 messages');
  await expect(tl.getByLabel('Turn summary').first()).toContainText('4 requests');
  await expect(tl.getByTestId('tl-resumed')).toBeVisible();
  await expect(page.getByTestId('loaded')).toContainText('of 25 records loaded');
});

test('large transcripts load page by page', async ({ page }) => {
  await openView(page);
  await rows(page).filter({ hasText: BIG }).click();
  const loaded = page.getByTestId('loaded');
  await expect(loaded).toContainText(/^200 of 1,996 records loaded/);
  await detail(page)
    .getByRole('button', { name: /Load more/ })
    .click();
  await expect(loaded).toContainText(/^400 of/);
  // Scrolling to the end loads further pages without clicking.
  const scroller = detail(page).getByRole('tabpanel');
  await scroller.evaluate((el) => el.scrollTo(0, el.scrollHeight));
  await expect(loaded).not.toContainText(/^400 of/);
});

test('metrics show request latency, TTFT, throughput and charts', async ({ page }) => {
  await openView(page);
  await rows(page).filter({ hasText: 'Fix the login bug' }).click();
  await detail(page).getByRole('tab', { name: 'Metrics' }).click();
  const m = page.getByTestId('metrics');
  await expect(m.getByTestId('m-requests')).toHaveText('5');
  await expect(m.getByRole('img', { name: /Bar chart of 5 request durations/ })).toBeVisible();
  await expect(m.getByTestId('chart-in')).toBeVisible();
  await expect(m.getByTestId('chart-out')).toBeVisible();
  await expect(m.locator('rect.bar')).toHaveCount(5);
  await expect(m.getByRole('table', { name: 'Per-turn totals' })).toContainText('end turn');
  await m.getByText(/Per-request table/).click();
  await expect(m.getByRole('table', { name: 'Per-request metrics' }).getByRole('row')).toHaveCount(
    6
  );
});

test('a corrupt transcript opens with a notice about the skipped line', async ({ page }) => {
  await openView(page);
  await rows(page).filter({ hasText: 'Rename the helper' }).click();
  await expect(page.getByTestId('skipped')).toContainText('1 unreadable line was skipped');
  await expect(detail(page).getByTestId('tl-assistant')).toBeVisible();
});

test('copies the transcript as markdown', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await openView(page);
  await rows(page).filter({ hasText: 'Fix the login bug' }).click();
  await detail(page).getByRole('button', { name: 'Copy as markdown' }).click();
  await expect(detail(page).getByRole('button', { name: 'Copied' })).toBeVisible();
  const md = await page.evaluate(() => navigator.clipboard.readText());
  expect(md).toContain('# Session 20260301-101500-a1b2c3');
  expect(md).toContain('### Tool: Edit src/auth.rs');
  expect(md).toContain('```diff');
});

test('resume opens the session in chat', async ({ page }) => {
  await openView(page);
  await rows(page).filter({ hasText: 'Plan the database migration' }).click();
  await detail(page).getByRole('button', { name: 'Resume session' }).click();
  // Not connected yet: the app connects to the project, then loads the session.
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeEnabled();
  await expect(page.getByRole('heading', { name: 'Transcripts' })).toHaveCount(0);
  await expect(page.getByText('Plan the database migration from SQLite').first()).toBeVisible();
  await expect(page.getByText('Resumed from the transcript.')).toBeVisible();
});

test('view toggle returns to the chat', async ({ page }) => {
  await openView(page);
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Transcripts' })).toHaveCount(0);
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
});

test('works for WSL projects: the distro is passed to the backend', async ({ page }) => {
  await page.goto('/?mock=1&delay=2&transcriptdelay=10&platform=windows');
  await page.getByRole('radio', { name: 'WSL2' }).click();
  await expect(page.locator('[data-rc="ok"]')).toBeVisible();
  await page
    .getByRole('textbox', { name: 'Project folder' })
    .fill('\\\\wsl.localhost\\Ubuntu-24.04\\home\\dev\\app');
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Transcripts', exact: true }).click();
  await expect(rows(page)).toHaveCount(5);
  await rows(page).filter({ hasText: 'Fix the login bug' }).click();
  await expect(detail(page).getByRole('list', { name: 'Timeline' })).toBeVisible();
  await page.getByLabel('Search transcripts').fill('clippy');
  await expect(page.getByTestId('hit').first()).toBeVisible();

  const calls = await page.evaluate(
    () =>
      (
        window as unknown as {
          __ruckcodeTranscriptCalls: { op: string; project: string; distro: string | null }[];
        }
      ).__ruckcodeTranscriptCalls
  );
  expect(calls.length).toBeGreaterThanOrEqual(3);
  for (const c of calls) {
    expect(c).toMatchObject({ project: '/home/dev/app', distro: 'Ubuntu-24.04' });
  }
  expect(calls.map((c) => c.op)).toEqual(expect.arrayContaining(['list', 'read', 'search']));
});
