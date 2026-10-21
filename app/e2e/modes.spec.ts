import { expect, test, type Page } from '@playwright/test';

const goto = (page: Page, extra = '') =>
  page.goto(`/?mock=1&delay=2&modesdelay=5&setupdelay=5${extra}`);

async function openModes(page: Page, extra = '') {
  await goto(page, extra);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  await page.getByRole('group', { name: 'View' }).getByRole('button', { name: 'Modes' }).click();
}

const editor = (page: Page) => page.getByRole('region', { name: 'Mode editor' });
const list = (page: Page) => editor(page).getByRole('list', { name: 'Modes list' });
const pick = (page: Page, id: string) =>
  list(page)
    .getByRole('button', { name: new RegExp(`\\b${id}\\b`) })
    .first();
const reviewBtn = (page: Page) => editor(page).getByRole('button', { name: /Review & save/ });

async function saveAll(page: Page) {
  await reviewBtn(page).click();
  await editor(page).getByRole('button', { name: 'Save modes.toml' }).click();
  await expect(editor(page).getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
}

test('edits a prompt and the tools, reviews the diff and saves', async ({ page }) => {
  await openModes(page);
  await expect(list(page).getByRole('button')).toHaveCount(3 + 6); // 3 modes, up/down buttons
  await pick(page, 'plan').click();
  const prompt = editor(page).getByRole('textbox', { name: 'Prompt' });
  await expect(prompt).toHaveValue(/plan phase/);
  await prompt.fill('Plan very carefully.\nThen ask.');
  await expect(editor(page).getByTestId('prompt-count')).toContainText('30 characters');
  await editor(page)
    .getByRole('checkbox', { name: /^Write/ })
    .uncheck();
  await expect(reviewBtn(page)).toBeEnabled();
  await expect(editor(page)).toContainText('Unsaved changes');

  await reviewBtn(page).click();
  const diff = editor(page).getByLabel('Changes to modes.toml');
  await expect(diff).toContainText('+Plan very carefully.');
  await expect(diff).toContainText('-tools = ["Read", "Glob", "Grep", "Write"');
  await expect(diff).toContainText('+tools = ["Read", "Glob", "Grep", "AskUserQuestion"');
  await editor(page).getByRole('button', { name: 'Save modes.toml' }).click();
  await expect(editor(page)).toContainText('apply to new sessions');
  await expect(editor(page)).not.toContainText('Unsaved changes');

  // persisted: leave and come back
  await page.getByRole('group', { name: 'View' }).getByRole('button', { name: 'Chat' }).click();
  await page.getByRole('group', { name: 'View' }).getByRole('button', { name: 'Modes' }).click();
  await pick(page, 'plan').click();
  await expect(editor(page).getByRole('textbox', { name: 'Prompt' })).toHaveValue(
    'Plan very carefully.\nThen ask.'
  );
  await expect(editor(page).getByRole('checkbox', { name: /^Write/ })).not.toBeChecked();
});

test('adds a mode from a lifecycle template', async ({ page }) => {
  await openModes(page);
  await editor(page).getByLabel('Add mode from template').selectOption({ label: 'Review' });
  await editor(page).getByRole('button', { name: 'Add mode', exact: true }).click();
  await expect(editor(page).getByRole('heading', { level: 2 })).toContainText('Review');
  await expect(editor(page).getByRole('textbox', { name: 'Prompt' })).toHaveValue(/review phase/);
  // references to modes this file does not have were dropped
  await expect(editor(page).getByLabel('next: build')).not.toBeChecked();
  await expect(list(page)).toContainText('new');
  await editor(page).getByLabel('next: build').check();
  await reviewBtn(page).click();
  const diff = editor(page).getByLabel('Changes to modes.toml');
  await expect(diff).toContainText('+[modes.review]');
  await expect(diff).toContainText('+next = ["build"]');
  await editor(page).getByRole('button', { name: 'Save modes.toml' }).click();
  await expect(pick(page, 'review')).toBeVisible();
});

test('renames a mode and updates every reference', async ({ page }) => {
  await openModes(page);
  await pick(page, 'build').click();
  await editor(page).getByRole('textbox', { name: 'Id', exact: true }).fill('implement');
  await expect(editor(page)).toContainText('Renaming also updates 3 references');
  await editor(page).getByRole('button', { name: 'Rename' }).click();
  await expect(list(page)).toContainText('implement');

  // the other modes now point at the new id
  await pick(page, 'plan').click();
  await expect(editor(page).getByLabel('next: implement')).toBeChecked();
  await pick(page, 'verify').click();
  await expect(editor(page).getByLabel('next: implement')).toBeChecked();
  await expect(editor(page).getByLabel('auto-advance: implement')).toBeChecked();

  await reviewBtn(page).click();
  const diff = editor(page).getByLabel('Changes to modes.toml');
  await expect(diff).toContainText('+[modes.implement]');
  await expect(diff).toContainText('-[modes.build]');
  await expect(diff).toContainText('+next = ["implement"]');
  await editor(page).getByRole('button', { name: 'Save modes.toml' }).click();
  await expect(editor(page)).toContainText('Saved');
});

test('a hook with an invalid regex blocks saving until it is fixed', async ({ page }) => {
  await openModes(page);
  await pick(page, 'build').click();
  await editor(page).locator('summary', { hasText: 'pre_tool_use' }).click();
  await editor(page).getByRole('button', { name: 'Add pre_tool_use hook' }).click();
  await editor(page)
    .getByRole('textbox', { name: 'Command for pre_tool_use hook 1' })
    .fill('echo checking');
  const matcher = editor(page).getByRole('textbox', { name: 'Matcher for pre_tool_use hook 1' });
  await matcher.fill('Bash|(Edit');
  await expect(editor(page)).toContainText('Invalid regex');
  await expect(matcher).toHaveAttribute('aria-invalid', 'true');
  await expect(editor(page).getByTestId('error-count')).toContainText('1 error');
  await expect(reviewBtn(page)).toBeDisabled();

  await matcher.fill('Bash|Edit');
  await expect(editor(page)).not.toContainText('Invalid regex');
  await expect(reviewBtn(page)).toBeEnabled();
  await editor(page).getByLabel('On failure for pre_tool_use hook 1').selectOption('block');
  await reviewBtn(page).click();
  const diff = editor(page).getByLabel('Changes to modes.toml');
  await expect(diff).toContainText('+[[modes.build.hooks.pre_tool_use]]');
  await expect(diff).toContainText('+matcher = "Bash|Edit"');
  await expect(diff).toContainText('+on_failure = "block"');
  await editor(page).getByRole('button', { name: 'Save modes.toml' }).click();
  await expect(editor(page)).toContainText('Saved');
});

test('edits the permissions table', async ({ page }) => {
  await openModes(page);
  await pick(page, 'verify').click();
  const table = editor(page).getByRole('table', { name: 'Permissions' });
  await expect(table).toContainText('Bash');
  await editor(page).getByLabel('Permission for Bash').selectOption('deny');
  await editor(page).getByLabel('Tool or glob').fill('mise__*');
  await editor(page).getByLabel('New rule permission').selectOption('allow');
  await editor(page).getByRole('button', { name: 'Add rule' }).click();
  await expect(table).toContainText('mise__*');
  await reviewBtn(page).click();
  const diff = editor(page).getByLabel('Changes to modes.toml');
  await expect(diff).toContainText('-Bash = "ask"');
  await expect(diff).toContainText('+Bash = "deny"');
  await expect(diff).toContainText('+"mise__*" = "allow"');
  await editor(page).getByRole('button', { name: 'Back to editing' }).click();
  await editor(page).getByRole('button', { name: 'Remove rule Bash' }).click();
  await expect(table).not.toContainText('Bash');
});

test('shows the flow diagram with auto-advance edges styled differently', async ({ page }) => {
  await openModes(page);
  const flow = editor(page).getByTestId('flow');
  await expect(flow).toBeVisible();
  await expect(flow.getByRole('button', { name: 'Mode Plan (default)' })).toBeVisible();
  await expect(flow.locator('[data-edge="plan>build"]')).toHaveAttribute('data-kind', 'next');
  await expect(flow.locator('[data-edge="verify>build"]')).toHaveAttribute('data-kind', 'auto');
  // clicking a node opens its form
  await flow.getByRole('button', { name: 'Mode Build' }).click();
  await expect(editor(page).getByRole('heading', { level: 2 })).toContainText('Build');
  // changing the transitions redraws it
  await editor(page).getByLabel('next: plan').check();
  await editor(page).getByRole('button', { name: 'Overview & flow' }).click();
  await expect(flow.locator('[data-edge="build>plan"]')).toBeVisible();
});

test('reorders modes with the move buttons and warns before leaving with unsaved changes', async ({
  page
}) => {
  await openModes(page);
  await editor(page).getByRole('button', { name: 'Move verify up' }).click();
  await expect(list(page).getByRole('listitem').nth(1)).toContainText('verify');
  await expect(editor(page)).toContainText('Unsaved changes');

  await page.getByRole('group', { name: 'View' }).getByRole('button', { name: 'Chat' }).click();
  const dlg = page.getByRole('alertdialog', { name: 'Unsaved mode changes' });
  await expect(dlg).toBeVisible();
  await dlg.getByRole('button', { name: 'Keep editing' }).click();
  await expect(dlg).toHaveCount(0);
  await expect(editor(page)).toBeVisible();

  await page.getByRole('group', { name: 'View' }).getByRole('button', { name: 'Chat' }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Discard changes' }).click();
  await expect(editor(page)).toHaveCount(0);
  await page.getByRole('group', { name: 'View' }).getByRole('button', { name: 'Modes' }).click();
  await expect(list(page).getByRole('listitem').nth(1)).toContainText('build');
});

test('without a modes.toml it offers rc init (and a blank file)', async ({ page }) => {
  await goto(page, '&setup=fresh');
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  // The first-run wizard opens asynchronously; wait for it before dismissing it.
  const wizard = page.getByRole('dialog', { name: /settings/i });
  await expect(wizard).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(wizard).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  await page.getByRole('group', { name: 'View' }).getByRole('button', { name: 'Modes' }).click();
  await expect(editor(page)).toContainText('No modes.toml');
  await editor(page)
    .getByLabel(/Choose which modes/)
    .check();
  await editor(page)
    .getByRole('list', { name: 'Modes to create' })
    .getByLabel(/design/)
    .uncheck();
  await editor(page).getByRole('button', { name: 'Create modes.toml' }).click();
  await expect(editor(page).getByRole('list', { name: 'Modes list' })).toBeVisible();
});

test('the editor works against a WSL host', async ({ page }) => {
  await page.goto('/?mock=1&delay=2&platform=windows&setupdelay=5&modesdelay=5');
  await page.getByRole('radio', { name: 'WSL2' }).click();
  await expect(page.getByText('rc 0.1.0 (bundled)')).toBeVisible();
  await page.getByRole('textbox', { name: 'Project folder' }).fill('/home/dev/proj');
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  await page.getByRole('group', { name: 'View' }).getByRole('button', { name: 'Modes' }).click();
  await expect(editor(page)).toContainText('/home/dev/proj/.ruckcode/modes.toml');
  await pick(page, 'plan').click();
  await editor(page).getByLabel('Description').fill('Think first');
  await saveAll(page);
  const calls = await page.evaluate(
    () =>
      (window as unknown as { __ruckcodeModesCalls: { op: string; distro: string | null }[] })
        .__ruckcodeModesCalls
  );
  expect(calls.map((c) => c.op)).toEqual(expect.arrayContaining(['get', 'preview', 'apply']));
  expect(calls.every((c) => c.distro === 'Ubuntu-24.04')).toBe(true);
  await editor(page).getByRole('button', { name: 'Reconnect the agent now' }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
});
