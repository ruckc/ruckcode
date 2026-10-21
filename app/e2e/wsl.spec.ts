import { expect, test, type Page } from '@playwright/test';
import { send } from './helpers';

const open = (page: Page, platform = 'windows') =>
  page.goto(`/?mock=1&delay=2&platform=${platform}`);

const connects = (page: Page) =>
  page.evaluate(
    () => (window as unknown as { __ruckcodeConnects?: unknown[] }).__ruckcodeConnects ?? []
  );

async function chooseWsl(page: Page) {
  await page.getByRole('radio', { name: 'WSL2' }).click();
  await expect(page.getByRole('combobox', { name: 'WSL distro' })).toHaveValue('Ubuntu-24.04');
}

test('WSL2 host is hidden outside Windows', async ({ page }) => {
  await open(page, 'linux');
  await expect(page.getByRole('radio', { name: 'Local' })).toBeVisible();
  await expect(page.getByRole('radio', { name: 'Custom rc' })).toBeVisible();
  await expect(page.getByRole('radio', { name: 'WSL2' })).toHaveCount(0);
});

test('lists distros, installs the bundled rc automatically, then connects through the WSL host', async ({
  page
}) => {
  await open(page);
  await chooseWsl(page);

  // distro list: default first, WSL1 and Docker Desktop are not selectable
  const select = page.getByRole('combobox', { name: 'WSL distro' });
  await expect(select.getByRole('option')).toHaveCount(4);
  await expect(select.getByRole('option', { name: /Alpine/ })).toHaveAttribute('disabled', '');
  await expect(select.getByRole('option', { name: /docker-desktop/ })).toHaveAttribute(
    'disabled',
    ''
  );

  // rc is missing in the distro: it is installed from the bundled copy without any click
  const connect = page.getByRole('button', { name: 'Connect', exact: true });
  await page.getByRole('textbox', { name: 'Project folder' }).fill('C:\\Users\\me\\proj');
  await expect(page.locator('[data-rc="ok"]')).toBeVisible();
  // once rc works, its status, version, log and reinstall button stay out of the way
  await expect(page.getByRole('status', { name: 'rc status' })).toHaveCount(0);
  await expect(page.getByLabel('Install progress')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Reinstall rc' })).toHaveCount(0);
  // path translation is shown
  await expect(page.getByLabel('Linux path')).toContainText('/mnt/c/Users/me/proj');
  await expect(connect).toBeEnabled();

  await connect.click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  expect((await connects(page)).at(-1)).toMatchObject({
    host: 'wsl',
    distro: 'Ubuntu-24.04',
    cwd: '/mnt/c/Users/me/proj'
  });
  // the connected project shows its host
  await expect(page.getByRole('navigation')).toContainText('WSL: Ubuntu-24.04');
  await send(page, 'hello wsl');
  await expect(page.getByTestId('assistant-msg')).toContainText('You said: hello wsl.');
});

test('connect is blocked while the bundled rc is being installed', async ({ page }) => {
  await page.goto('/?mock=1&delay=120&platform=windows');
  await page.getByRole('radio', { name: 'WSL2' }).click();
  await page.getByRole('textbox', { name: 'Project folder' }).fill('/home/dev/app');
  await expect(page.getByRole('status', { name: 'rc status' })).toHaveAttribute(
    'data-state',
    'installing'
  );
  await expect(page.getByText('Installing rc into Ubuntu-24.04…')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Connect', exact: true })).toBeDisabled();
  await expect(page.locator('[data-rc="ok"]')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Connect', exact: true })).toBeEnabled();
});

test('a distro with an older rc is updated automatically', async ({ page }) => {
  await open(page);
  await chooseWsl(page);
  await page.getByRole('combobox', { name: 'WSL distro' }).selectOption('Debian');
  await expect(page.locator('[data-rc="ok"]')).toBeVisible();
  await page.getByRole('textbox', { name: 'Project folder' }).fill('/home/dev/app');
  await expect(page.getByRole('button', { name: 'Connect', exact: true })).toBeEnabled();
});

test('translates UNC and Linux paths and rejects another distro', async ({ page }) => {
  await open(page);
  await chooseWsl(page);
  const input = page.getByRole('textbox', { name: 'Project folder' });
  const linux = page.getByLabel('Linux path');
  await input.fill('\\\\wsl.localhost\\Ubuntu-24.04\\home\\dev\\app');
  await expect(linux).toContainText('/home/dev/app');
  await input.fill('/srv/code/');
  await expect(linux).toContainText('/srv/code');
  await input.fill('\\\\wsl$\\Debian\\home\\dev');
  await expect(linux).toContainText('belongs to distro `Debian`');
  await expect(linux).toHaveAttribute('data-ok', 'false');
});

test('Browse returns a \\\\wsl.localhost path that is shown as a Linux path', async ({ page }) => {
  await open(page);
  await chooseWsl(page);
  await page.getByRole('button', { name: 'Browse…' }).click();
  await expect(page.getByRole('textbox', { name: 'Project folder' })).toHaveValue(
    '/home/dev/picked'
  );
});

test('the host choice is remembered with the recent project', async ({ page }) => {
  await open(page);
  await chooseWsl(page);
  await expect(page.locator('[data-rc="ok"]')).toBeVisible();
  await page.getByRole('textbox', { name: 'Project folder' }).fill('/home/dev/app');
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Disconnect' }).click();

  await page.reload();
  const recent = page.getByRole('button', { name: /app.*WSL: Ubuntu-24.04/ });
  await expect(recent).toBeVisible();
  await recent.click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  expect((await connects(page)).at(-1)).toMatchObject({
    host: 'wsl',
    distro: 'Ubuntu-24.04',
    cwd: '/home/dev/app'
  });
});

test('a WSL project in another distro is refused before connecting', async ({ page }) => {
  await open(page);
  await chooseWsl(page);
  await expect(page.locator('[data-rc="ok"]')).toBeVisible();
  await page.getByRole('textbox', { name: 'Project folder' }).fill('\\\\wsl$\\Debian\\home\\dev');
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.getByRole('alert').filter({ hasText: 'belongs to distro' })).toBeVisible();
  expect(await connects(page)).toHaveLength(0);
});

test('an agent that exits shows its exit code and stderr tail immediately', async ({ page }) => {
  await open(page);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  await send(page, 'mock:exit 3 config file is invalid');
  await expect(page.getByRole('alert')).toContainText('exited with code 3: config file is invalid');
  await expect(page.getByRole('button', { name: 'Reconnect' })).toBeVisible();
});
