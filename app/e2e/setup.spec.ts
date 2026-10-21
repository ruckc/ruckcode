import { expect, test, type Page } from '@playwright/test';

const fresh = (page: Page, extra = '') =>
  page.goto(`/?mock=1&delay=2&setup=fresh&setupdelay=40${extra}`);
const normal = (page: Page, extra = '') => page.goto(`/?mock=1&delay=2&setupdelay=40${extra}`);

const dialog = (page: Page) => page.getByRole('dialog', { name: 'Settings' });
const next = (page: Page) => dialog(page).getByRole('button', { name: 'Next', exact: true });

async function openSettings(page: Page, tab?: string) {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(dialog(page)).toBeVisible();
  if (tab) await dialog(page).getByRole('tab', { name: tab }).click();
}

test('first run opens the wizard, which saves a profile and reconnects the agent', async ({
  page
}) => {
  await fresh(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page)).toContainText('No model provider is configured');

  // provider -> endpoint -> auth (env var is set) -> profile
  await dialog(page)
    .getByLabel(/OpenRouter/)
    .check();
  await next(page).click();
  await expect(dialog(page).getByLabel('Endpoint URL')).toHaveValue('https://openrouter.ai/api/v1');
  await next(page).click();
  await expect(dialog(page).getByTestId('env-status')).toContainText('OPENROUTER_API_KEY is set');
  await next(page).click();
  await expect(dialog(page).getByLabel('Profile name')).toHaveValue('openrouter');
  await next(page).click();

  // models: spinner, then filter + select + default
  await expect(dialog(page).getByRole('list', { name: 'Models' })).toBeVisible();
  await expect(next(page)).toBeDisabled();
  await dialog(page).getByLabel('Filter models').fill('claude');
  await dialog(page).getByRole('button', { name: 'Select all' }).click();
  await expect(dialog(page)).toContainText('1 of 5 selected');
  await expect(dialog(page).getByLabel('Default model anthropic/claude-sonnet-5-5')).toBeChecked();
  await next(page).click();

  // modes: no modes.toml yet -> create it, then assign a model to one mode
  await expect(dialog(page)).toContainText('has no .ruckcode/modes.toml');
  await dialog(page).getByRole('button', { name: 'Create modes.toml' }).click();
  await dialog(page)
    .getByLabel('Model for mode plan')
    .selectOption({ label: 'anthropic/claude-sonnet-5-5' });
  await dialog(page).getByRole('button', { name: 'Review' }).click();

  const diff = dialog(page).getByLabel('Changes to be written');
  await expect(diff).toContainText('[profiles.openrouter]');
  await expect(diff).toContainText('default_profile = "openrouter"');
  await expect(diff).toContainText('[modes.plan]');
  await dialog(page).getByRole('button', { name: 'Save' }).click();

  await expect(dialog(page)).toContainText('Saved');
  await dialog(page).getByRole('button', { name: 'Reconnect the agent now' }).click();
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
});

test('model fetch error offers retry and manual entry', async ({ page }) => {
  await normal(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  await openSettings(page);
  await dialog(page).getByLabel(/vLLM/).check();
  await next(page).click();
  await dialog(page).getByLabel('Endpoint URL').fill('http://broken.example/v1');
  await next(page).click();
  await next(page).click(); // auth: none
  await dialog(page).getByLabel('Profile name').fill('vllm');
  await next(page).click();

  await expect(dialog(page).getByTestId('models-error')).toContainText('401');
  await dialog(page).getByRole('button', { name: 'Retry' }).click();
  await expect(dialog(page).getByTestId('models-error')).toContainText('401');

  await dialog(page).getByLabel('Add model ids manually').fill('my-local-model');
  await dialog(page).getByRole('button', { name: 'Add', exact: true }).click();
  await expect(dialog(page).getByLabel('Default model my-local-model')).toBeChecked();
  await next(page).click();
  // modes already exist in the normal mock project; keep defaults
  await dialog(page).getByRole('button', { name: 'Review' }).click();
  await expect(dialog(page).getByLabel('Changes to be written')).toContainText('my-local-model');
  await dialog(page).getByRole('button', { name: 'Save' }).click();
  await expect(dialog(page)).toContainText('Saved');
});

test('add and remove a named key; the secret never appears on the page', async ({ page }) => {
  await normal(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  await openSettings(page, 'API keys');
  const add = dialog(page).getByRole('form', { name: 'Add key' });
  await add.getByLabel('Name').fill('work');
  await add.getByLabel('Description (optional)').fill('Team key');
  const secret = add.getByLabel('API key');
  await expect(secret).toHaveAttribute('type', 'password');
  await secret.fill('sk-live-very-secret-4242');
  await add.getByRole('button', { name: 'Save key' }).click();

  const table = dialog(page).getByRole('table', { name: 'API keys' });
  await expect(table).toContainText('work');
  await expect(table).toContainText('Team key');
  await expect(table).toContainText('…4242');
  await expect(secret).toHaveValue('');
  expect(await page.content()).not.toContain('very-secret');

  await dialog(page).getByRole('button', { name: 'Remove work' }).click();
  await expect(table).toContainText('work'); // not yet: needs confirmation
  await dialog(page).getByRole('button', { name: 'Confirm remove' }).click();
  await expect(dialog(page)).toContainText('No saved keys');
});

test('use a key for a profile in this project only', async ({ page }) => {
  await normal(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  await openSettings(page, 'API keys');
  const add = dialog(page).getByRole('form', { name: 'Add key' });
  await add.getByLabel('Name').fill('client-acme');
  await add.getByLabel('API key').fill('sk-acme-12345678');
  await add.getByRole('button', { name: 'Save key' }).click();
  await expect(dialog(page).getByRole('table', { name: 'API keys' })).toContainText('unused');

  await dialog(page).getByRole('button', { name: 'Use for profile' }).click();
  await dialog(page).getByLabel('This project only').check();
  await dialog(page).getByRole('button', { name: 'Apply' }).click();
  await expect(dialog(page).getByRole('table', { name: 'API keys' })).toContainText(
    'profile `mock` (this project)'
  );
});

test('creates modes.toml from the Project tab', async ({ page }) => {
  await fresh(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(dialog(page)).toBeVisible();
  await page.keyboard.press('Escape'); // skip the wizard: the agent connects anyway
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();

  await openSettings(page, 'Project');
  await expect(dialog(page)).toContainText('No modes.toml');
  await dialog(page).getByRole('button', { name: 'Create modes.toml' }).click();
  await expect(dialog(page)).toContainText('3 mise tasks discovered');
  await expect(dialog(page).getByRole('list', { name: 'Modes' })).toContainText('verify');
});

test('the whole flow works against a WSL host in mock mode', async ({ page }) => {
  await page.goto('/?mock=1&delay=2&platform=windows&setupdelay=20');
  await page.getByRole('radio', { name: 'WSL2' }).click();
  await page.getByRole('button', { name: 'Install rc' }).click();
  await expect(page.getByText('rc 0.1.0 installed')).toBeVisible();
  await page.getByRole('textbox', { name: 'Project folder' }).fill('/home/dev/proj');
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  await openSettings(page, 'API keys');
  await expect(dialog(page)).toContainText('/home/dev/proj (WSL: Ubuntu-24.04)');
  const add = dialog(page).getByRole('form', { name: 'Add key' });
  await add.getByLabel('Name').fill('wslkey');
  await add.getByLabel('API key').fill('sk-wsl-00009999');
  await add.getByRole('button', { name: 'Save key' }).click();
  await expect(dialog(page).getByRole('table', { name: 'API keys' })).toContainText('…9999');
});
