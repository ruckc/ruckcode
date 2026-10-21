import { connected, menu } from './helpers';
import { expect, test, type Page } from '@playwright/test';

const fresh = (page: Page, extra = '') =>
  page.goto(`/?mock=1&delay=2&setup=fresh&setupdelay=40${extra}`);
const normal = (page: Page, extra = '') => page.goto(`/?mock=1&delay=2&setupdelay=40${extra}`);

const settings = (page: Page) => page.getByRole('dialog', { name: 'Settings' });
const dialog = (page: Page) => page.getByRole('dialog', { name: /Set up ruckcode|Add a provider/ });
const next = (page: Page) => dialog(page).getByRole('button', { name: 'Next', exact: true });

/** Settings > Providers > Add provider opens the wizard in its own dialog. */
async function openWizard(page: Page) {
  await menu(page, 'Settings');
  await settings(page).getByRole('button', { name: 'Add provider…' }).click();
  await expect(settings(page)).toHaveCount(0);
  await expect(dialog(page)).toBeVisible();
}

async function openSettings(page: Page, tab?: string) {
  await menu(page, 'Settings');
  await expect(settings(page)).toBeVisible();
  if (tab) await settings(page).getByRole('tab', { name: tab }).click();
}

test('first run: paste a key, pick the URL, check compatibility, then save a profile', async ({
  page
}) => {
  await fresh(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page)).toContainText('No model provider is configured');

  // 1. API key (masked); there is no env-var option and no provider step.
  await expect(dialog(page)).not.toContainText('nvironment variable');
  await expect(next(page)).toBeDisabled();
  const secret = dialog(page).getByRole('textbox', { name: 'API key' });
  await expect(secret).toHaveAttribute('type', 'password');
  await secret.fill('sk-or-v1-very-secret-4242');
  await expect(next(page)).toBeEnabled();
  await next(page).click();

  // 2. Endpoint URL: empty, required, validated inline; no suggestions.
  const url = dialog(page).getByLabel('Endpoint URL');
  await expect(url).toHaveValue('');
  await expect(next(page)).toBeDisabled();
  await url.fill('openrouter.ai/api/v1');
  await expect(dialog(page).getByTestId('endpoint-error')).toContainText('http://');
  await expect(next(page)).toBeDisabled();
  for (const label of ['Anthropic', 'OpenAI', 'OpenRouter', 'Ollama', 'LM Studio', 'vLLM'])
    await expect(dialog(page).getByRole('button', { name: label, exact: true })).toHaveCount(0);
  await url.fill('https://openrouter.ai/api/v1');
  await next(page).click();

  // 3. Compatibility check detects the API flavor.
  await expect(dialog(page).getByTestId('check-status')).toContainText(
    'OpenAI-compatible API ✓ (5 models)'
  );
  await next(page).click();

  // 4. Models: filter + select + default; 5. profile name derived from the host.
  await expect(dialog(page).getByRole('list', { name: 'Models' })).toBeVisible();
  await expect(next(page)).toBeDisabled();
  await dialog(page).getByLabel('Filter models').fill('claude');
  await dialog(page).getByRole('button', { name: 'Select all' }).click();
  await expect(dialog(page)).toContainText('1 of 5 selected');
  await expect(dialog(page).getByLabel('Default model anthropic/claude-sonnet-5-5')).toBeChecked();
  await next(page).click();
  await expect(dialog(page).getByLabel('Profile name')).toHaveValue('openrouter');
  await next(page).click();

  // 6. modes: no modes.toml yet -> create it, then assign a model to one mode
  await expect(dialog(page)).toContainText('has no .ruckcode/modes.toml');
  await dialog(page).getByRole('button', { name: 'Create modes.toml' }).click();
  await dialog(page)
    .getByLabel('Model for mode plan')
    .selectOption({ label: 'anthropic/claude-sonnet-5-5' });
  await dialog(page).getByRole('button', { name: 'Review' }).click();

  const diff = dialog(page).getByLabel('Changes to be written');
  await expect(diff).toContainText('[profiles.openrouter]');
  await expect(diff).toContainText('api_key_name = "openrouter"');
  await expect(diff).toContainText('default_profile = "openrouter"');
  await expect(diff).toContainText('[modes.plan]');
  await expect(diff).not.toContainText('very-secret');
  await dialog(page).getByRole('button', { name: 'Save' }).click();

  await expect(dialog(page)).toContainText('Saved');
  expect(await page.content()).not.toContain('very-secret');
  await dialog(page).getByRole('button', { name: 'Reconnect the agent now' }).click();
  await expect(dialog(page)).toHaveCount(0);
  await connected(page);
});

test('a local server needs no key: type the Ollama URL', async ({ page }) => {
  await normal(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await connected(page);
  await openWizard(page);
  await dialog(page).getByLabel('No key').check();
  await next(page).click();
  await expect(dialog(page).getByLabel('Endpoint URL')).toHaveValue('');
  await expect(next(page)).toBeDisabled();
  await dialog(page).getByLabel('Endpoint URL').fill('http://localhost:11434/v1');
  await next(page).click();
  await expect(dialog(page).getByTestId('check-status')).toContainText(
    'OpenAI-compatible API ✓ (3 models)'
  );
  await next(page).click();
  await dialog(page).getByLabel('Filter models').fill('llama');
  await dialog(page).getByRole('button', { name: 'Select all' }).click();
  await next(page).click();
  await expect(dialog(page).getByLabel('Profile name')).toHaveValue('ollama');
  await next(page).click();
  await dialog(page).getByRole('button', { name: 'Review' }).click();
  const diff = dialog(page).getByLabel('Changes to be written');
  await expect(diff).toContainText('base_url = "http://localhost:11434/v1"');
  await expect(diff).not.toContainText('api_key');
  await dialog(page).getByRole('button', { name: 'Save' }).click();
  await expect(dialog(page)).toContainText('Saved');
});

test('check errors: invalid key (401), wrong path (404), override, manual model entry', async ({
  page
}) => {
  await normal(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await connected(page);
  await openWizard(page);
  await dialog(page).getByRole('textbox', { name: 'API key' }).fill('bad');
  await next(page).click();
  await dialog(page).getByLabel('Endpoint URL').fill('https://openrouter.ai/api/v1');
  await next(page).click();
  await expect(dialog(page).getByTestId('check-error')).toContainText('rejected the API key');
  await expect(dialog(page).getByTestId('check-error')).toContainText('401');
  await expect(next(page)).toBeDisabled();

  // Fix the key, but use a URL missing its /v1 path: a 404 with a hint.
  await dialog(page).getByRole('button', { name: 'Change URL' }).click();
  await dialog(page).getByRole('button', { name: 'Back' }).click();
  await dialog(page).getByRole('textbox', { name: 'API key' }).fill('sk-or-good-key');
  await next(page).click();
  await dialog(page).getByLabel('Endpoint URL').fill('https://openrouter.ai/api');
  await next(page).click();
  await expect(dialog(page).getByTestId('check-error')).toContainText('404');
  await expect(dialog(page).getByTestId('check-error')).toContainText('/v1');
  await dialog(page)
    .getByRole('button', { name: /Try the Anthropic API/ })
    .click();
  await expect(dialog(page).getByTestId('check-error')).toContainText('404');

  // Unreachable server: continue by typing model ids.
  await dialog(page).getByRole('button', { name: 'Change URL' }).click();
  await dialog(page).getByLabel('Endpoint URL').fill('http://broken.example/v1');
  await next(page).click();
  await expect(dialog(page).getByTestId('check-error')).toContainText('Could not connect');
  await dialog(page).getByRole('button', { name: 'Continue without a model list' }).click();
  await dialog(page).getByLabel('Add model ids manually').fill('my-local-model');
  await dialog(page).getByRole('button', { name: 'Add', exact: true }).click();
  await expect(dialog(page).getByLabel('Default model my-local-model')).toBeChecked();
  await next(page).click();
  await expect(dialog(page).getByLabel('Profile name')).toHaveValue('broken-example');
  await dialog(page).getByLabel('Profile name').fill('mine');
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
  await connected(page);
  await openSettings(page, 'API keys');
  const add = settings(page).getByRole('form', { name: 'Add key' });
  await add.getByLabel('Name').fill('work');
  await add.getByLabel('Description (optional)').fill('Team key');
  const secret = add.getByLabel('API key');
  await expect(secret).toHaveAttribute('type', 'password');
  await secret.fill('sk-live-very-secret-4242');
  await add.getByRole('button', { name: 'Save key' }).click();

  const table = settings(page).getByRole('table', { name: 'API keys' });
  await expect(table).toContainText('work');
  await expect(table).toContainText('Team key');
  await expect(table).toContainText('…4242');
  await expect(secret).toHaveValue('');
  expect(await page.content()).not.toContain('very-secret');

  await settings(page).getByRole('button', { name: 'Remove work' }).click();
  await expect(table).toContainText('work'); // not yet: needs confirmation
  await settings(page).getByRole('button', { name: 'Confirm remove' }).click();
  await expect(settings(page)).toContainText('No saved keys');
});

test('use a key for a profile in this project only', async ({ page }) => {
  await normal(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await connected(page);
  await openSettings(page, 'API keys');
  const add = settings(page).getByRole('form', { name: 'Add key' });
  await add.getByLabel('Name').fill('client-acme');
  await add.getByLabel('API key').fill('sk-acme-12345678');
  await add.getByRole('button', { name: 'Save key' }).click();
  await expect(settings(page).getByRole('table', { name: 'API keys' })).toContainText('unused');

  await settings(page).getByRole('button', { name: 'Use for profile' }).click();
  await settings(page).getByLabel('This project only').check();
  await settings(page).getByRole('button', { name: 'Apply' }).click();
  await expect(settings(page).getByRole('table', { name: 'API keys' })).toContainText(
    'profile `mock` (this project)'
  );
});

test('creates modes.toml from the Project tab', async ({ page }) => {
  await fresh(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(dialog(page)).toBeVisible();
  await page.keyboard.press('Escape'); // skip the wizard: the agent connects anyway
  await expect(dialog(page)).toHaveCount(0);
  await connected(page);

  await openSettings(page, 'Project');
  await expect(settings(page)).toContainText('No modes.toml');
  await settings(page).getByRole('button', { name: 'Create modes.toml' }).click();
  await expect(settings(page)).toContainText('3 mise tasks discovered');
  await expect(settings(page).getByRole('list', { name: 'Modes' })).toContainText('verify');
});

test('the whole flow works against a WSL host in mock mode', async ({ page }) => {
  await page.goto('/?mock=1&delay=2&platform=windows&setupdelay=20');
  await page.getByRole('radio', { name: 'WSL2' }).click();
  await expect(page.locator('[data-rc="ok"]')).toBeVisible();
  await page.getByRole('textbox', { name: 'Project folder' }).fill('/home/dev/proj');
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await connected(page);
  await openSettings(page, 'API keys');
  await expect(settings(page)).toContainText('/home/dev/proj (WSL: Ubuntu-24.04)');
  const add = settings(page).getByRole('form', { name: 'Add key' });
  await add.getByLabel('Name').fill('wslkey');
  await add.getByLabel('API key').fill('sk-wsl-00009999');
  await add.getByRole('button', { name: 'Save key' }).click();
  await expect(settings(page).getByRole('table', { name: 'API keys' })).toContainText('…9999');
});
