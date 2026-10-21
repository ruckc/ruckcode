import { expect, test } from '@playwright/test';
import { openApp, send } from './helpers';

test('send a prompt and see the streamed reply, plan and usage', async ({ page }) => {
  await openApp(page);
  await send(page, 'hello there');
  await expect(page.getByTestId('user-msg')).toHaveText('hello there');
  await expect(page.getByTestId('assistant-msg')).toContainText('You said: hello there.');
  await expect(page.getByTestId('turn-end')).toContainText('end_turn');
  await expect(page.getByRole('complementary', { name: 'Plan' })).toContainText('Double-check');
  await expect(page.getByRole('meter', { name: 'Context window usage' })).toBeVisible();
  // the session is now named after its first prompt in the sidebar
  await expect(page.getByRole('list', { name: 'Sessions' })).toContainText('hello there');
});

test('markdown is rendered, sanitized and code blocks can be copied', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await openApp(page);
  await send(page, 'mock:md');
  const msg = page.getByTestId('assistant-msg');
  await expect(msg.locator('h1')).toHaveText('Heading');
  await expect(msg.locator('strong')).toHaveText('bold');
  await expect(msg.locator('script')).toHaveCount(0);
  await msg.getByRole('button', { name: 'Copy code' }).click();
  await expect(msg.getByRole('button', { name: 'Copy code' })).toHaveText('Copied');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain(
    'const x: number = 1;'
  );
});

test('approving a write shows the diff, runs the tool and completes the card', async ({ page }) => {
  await openApp(page);
  await send(page, 'mock:write src/a.txt new body');
  const dialog = page.getByRole('alertdialog');
  await expect(dialog).toContainText('Write src/a.txt');
  await expect(dialog.getByRole('group', { name: 'Diff of src/a.txt' })).toBeVisible();
  await page.keyboard.press('y');
  await expect(dialog).toBeHidden();
  const card = page.getByTestId('tool-card');
  await expect(card).toHaveAttribute('data-status', 'completed');
  await expect(card).toContainText('+1 -1');
  await expect(page.getByTestId('assistant-msg')).toContainText('Wrote src/a.txt.');
});

test('rejecting a write fails the tool', async ({ page }) => {
  await openApp(page);
  await send(page, 'mock:write src/a.txt x');
  await page.getByRole('button', { name: /^Reject/ }).click();
  await expect(page.getByTestId('tool-card')).toHaveAttribute('data-status', 'failed');
  await expect(page.getByTestId('assistant-msg')).toContainText('Did not write');
});

test('answer a multi-question ask with multi-select and Other', async ({ page }) => {
  await openApp(page);
  await send(page, 'mock:askmulti');
  const dialog = page.getByRole('dialog', { name: /questions/ });
  await expect(dialog.getByText('Recommended')).toBeVisible();
  await dialog.getByLabel(/sqlite/).check();
  await dialog.getByRole('tab', { name: /Features/ }).click();
  await dialog.getByLabel(/auth/).check();
  await dialog.getByLabel(/search/).check();
  await dialog.getByLabel('Other answer').fill('tracing');
  await dialog.getByRole('tab', { name: /Q3/ }).click();
  await dialog.getByLabel(/No/).check();
  await dialog.getByRole('button', { name: /Submit/ }).click();
  await expect(page.getByTestId('assistant-msg')).toContainText(
    'Answers: sqlite (Recommended) | auth, search, other: tracing | No.'
  );
});

test('cancelling a question with Escape reports it to the agent', async ({ page }) => {
  await openApp(page);
  await send(page, 'mock:ask');
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('assistant-msg')).toContainText('Question cancelled.');
});

test('switch mode manually and by approving an agent proposal', async ({ page }) => {
  await openApp(page);
  const mode = page.getByLabel('Mode', { exact: true });
  await expect(mode).toHaveValue('implement');
  await mode.selectOption('plan');
  await expect(mode).toHaveValue('plan');
  await send(page, 'mock:mode explore');
  const dialog = page.getByRole('alertdialog');
  await expect(dialog).toContainText('Mode switch proposed');
  await expect(dialog).toContainText('switch to mode explore');
  await dialog.getByRole('button', { name: /Switch to explore/ }).click();
  await expect(mode).toHaveValue('explore');
  await expect(page.getByTestId('assistant-msg')).toContainText('Switched to explore.');
});

test('declining a proposed mode switch keeps the mode', async ({ page }) => {
  await openApp(page);
  await send(page, 'mock:mode explore');
  await page.getByRole('button', { name: /Stay in current mode/ }).click();
  await expect(page.getByLabel('Mode', { exact: true })).toHaveValue('implement');
  await expect(page.getByTestId('assistant-msg')).toContainText('Staying in implement.');
});

test('change the model', async ({ page }) => {
  await openApp(page);
  const model = page.getByLabel('Model', { exact: true });
  await expect(model).toHaveValue('mock-large');
  await model.selectOption('mock-small');
  await expect(model).toHaveValue('mock-small');
  await expect(page.getByRole('status').filter({ hasText: 'Model changed' })).toBeVisible();
});

test('open an old session, replaying its history, and close it', async ({ page }) => {
  await openApp(page);
  const sessions = page.getByRole('list', { name: 'Sessions' });
  await sessions.getByRole('button', { name: /Fix the login bug/ }).click();
  await expect(page.getByTestId('user-msg')).toHaveText('Fix the login bug in src/auth.rs');
  await expect(page.getByTestId('assistant-msg')).toContainText('Fixed:');
  await expect(page.getByTestId('tool-card')).toHaveCount(2);
  await expect(page.getByRole('complementary', { name: 'Plan' })).toContainText('Fix it');
  await page.getByRole('button', { name: 'Close session Fix the login bug' }).click();
  await expect(page.getByTestId('user-msg')).toHaveCount(0);
  await expect(sessions.getByRole('button', { name: /Fix the login bug/ })).toBeVisible();
});

test('Esc cancels a running turn, and prompts typed meanwhile are queued', async ({ page }) => {
  await openApp(page);
  await send(page, 'mock:slow');
  await expect(page.getByRole('button', { name: /Cancel the running turn/ })).toBeVisible();
  await send(page, 'queued follow-up');
  await expect(page.getByRole('list', { name: 'Queued prompts' })).toContainText(
    'queued follow-up'
  );
  await page.getByRole('textbox', { name: 'Prompt' }).press('Escape');
  await expect(page.getByTestId('turn-end').first()).toContainText('cancelled');
  // the queued prompt then runs as its own turn
  await expect(page.getByTestId('user-msg').nth(1)).toHaveText('queued follow-up');
  await expect(page.getByTestId('turn-end')).toHaveCount(2);
});

test('tool output is truncated until expanded, and diffs toggle side by side', async ({ page }) => {
  await openApp(page);
  await send(page, 'mock:bash');
  const card = page.getByTestId('tool-card');
  await card.getByRole('button', { name: /Run cargo test/ }).click();
  await expect(card.getByLabel('Tool output')).not.toContainText('case_40');
  await card.getByRole('button', { name: /Show all/ }).click();
  await expect(card.getByLabel('Tool output')).toContainText('case_59');
  await card.getByRole('button', { name: 'Show input' }).click();
  await expect(card.getByLabel('Tool input')).toContainText('cargo test');

  await send(page, 'mock:edit');
  const edit = page.getByTestId('tool-card').nth(1);
  await expect(edit.locator('tr.add').first()).toBeVisible();
  await edit.getByLabel('side by side').check();
  await expect(edit.locator('table.split')).toBeVisible();
});

test('agent crash shows a reconnect button that resumes the session', async ({ page }) => {
  await openApp(page);
  await send(page, 'before crash');
  await expect(page.getByTestId('turn-end')).toBeVisible();
  await send(page, 'mock:crash');
  const alert = page.getByRole('alert');
  await expect(alert).toContainText('Lost connection');
  await alert.getByRole('button', { name: 'Reconnect' }).click();
  await expect(alert).toBeHidden();
  await expect(page.getByTestId('user-msg').first()).toHaveText('before crash');
  await send(page, 'after reconnect');
  await expect(page.getByTestId('assistant-msg').last()).toContainText(
    'You said: after reconnect.'
  );
});

test('recent projects are remembered across reloads', async ({ page }) => {
  await openApp(page);
  await page.getByRole('button', { name: 'Disconnect' }).click();
  await page.reload();
  await expect(page.getByRole('button', { name: /project/ }).first()).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Recent projects' })).toBeVisible();
});
