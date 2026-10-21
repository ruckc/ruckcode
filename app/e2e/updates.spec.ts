import { menu } from './helpers';
import { expect, test, type Page } from '@playwright/test';

const calls = (page: Page) =>
  page.evaluate(
    () => (window as unknown as { __ruckcodeUpdateCalls?: string[] }).__ruckcodeUpdateCalls ?? []
  );

const open = (page: Page, update: string, extra = '') =>
  page.goto(`/?mock=1&delay=2&update=${update}&updatedelay=60${extra}`);

test('no update UI without an updater (plain mock mode)', async ({ page }) => {
  await page.goto('/?mock=1&delay=2');
  await expect(page.getByRole('button', { name: 'Connect' })).toBeVisible();
  await expect(page.getByTestId('update-banner')).toHaveCount(0);
  await expect(page.getByTestId('update-downloading')).toHaveCount(0);
  expect(await calls(page)).toEqual([]);
});

test('downloads in the background, then offers Restart now / Later', async ({ page }) => {
  await open(page, '1');
  // subtle progress line first
  await expect(page.getByTestId('update-downloading')).toContainText('Downloading update v9.9.9');
  const banner = page.getByTestId('update-banner');
  await expect(banner).toContainText('Update v9.9.9 ready.');
  await banner.getByRole('button', { name: 'Release notes' }).click();
  await expect(page.getByText('Faster startup')).toBeVisible();

  await banner.getByRole('button', { name: 'Later' }).click();
  await expect(banner).toHaveCount(0);
  expect(await calls(page)).toEqual(['check', 'download']);
});

test('Restart now installs and relaunches', async ({ page }) => {
  await open(page, '1');
  await page.getByRole('button', { name: 'Restart now' }).click();
  await expect.poll(() => calls(page)).toEqual(['check', 'download', 'install', 'restart']);
});

test('never restarts under a running agent turn', async ({ page }) => {
  await open(page, '1');
  await page.getByRole('button', { name: 'Connect' }).click();
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeEnabled();
  await page.getByRole('textbox', { name: 'Prompt' }).fill('mock:slow');
  await page.getByRole('textbox', { name: 'Prompt' }).press('Enter');
  await expect(page.getByRole('button', { name: /Cancel the running turn/ })).toBeVisible();

  const banner = page.getByTestId('update-banner');
  await expect(banner).toContainText('would interrupt it');
  await expect(banner.getByRole('button', { name: 'Restart now' })).toHaveCount(0);
  // forcing takes two deliberate clicks
  await banner.getByRole('button', { name: 'Restart anyway…' }).click();
  await expect(
    banner.getByRole('button', { name: 'Interrupt the agent and restart' })
  ).toBeVisible();
  expect(await calls(page)).not.toContain('install');

  // once the turn ends, a plain Restart now is offered again
  await page.getByRole('textbox', { name: 'Prompt' }).press('Escape');
  await expect(banner.getByRole('button', { name: 'Restart now' })).toBeVisible();
});

test('install types that cannot self-update link to the download page', async ({ page }) => {
  await open(page, 'manual');
  const banner = page.getByTestId('update-banner');
  await expect(banner).toContainText('Linux package');
  await expect(banner.getByRole('link')).toHaveAttribute(
    'href',
    'https://ruckc.github.io/ruckcode/'
  );
  await expect(banner.getByRole('button', { name: 'Restart now' })).toHaveCount(0);
  expect(await calls(page)).toEqual(['check']);
});

test('a failed download shows the error and can be retried', async ({ page }) => {
  await open(page, 'faildownload');
  await expect(page.getByTestId('update-banner')).toContainText('signature verification failed');
  await page.getByRole('button', { name: 'Retry' }).click();
  await expect.poll(async () => (await calls(page)).filter((c) => c === 'download').length).toBe(2);
});

test('Settings: the automatic-download toggle persists, and downloads then wait for approval', async ({
  page
}) => {
  await open(page, 'uptodate');
  await menu(page, 'Settings');
  await page.getByRole('tab', { name: 'Updates' }).click();
  const box = page.getByRole('checkbox', { name: 'Automatically download updates' });
  await expect(box).toBeChecked();
  await expect(page.getByTestId('update-status')).toContainText('up to date');
  await box.uncheck();

  // reload with a newer version available: asks first instead of downloading
  await open(page, '1');
  await expect(page.getByTestId('update-banner')).toContainText('Update v9.9.9 is available.');
  await expect(page.getByTestId('update-banner')).toContainText('Automatic downloads are off');
  expect(await calls(page)).toEqual(['check']);
  await page.getByRole('button', { name: 'Download' }).click();
  await expect(page.getByTestId('update-banner')).toContainText('Update v9.9.9 ready.');
});

test('Settings reports an unconfigured build', async ({ page }) => {
  await open(page, 'unconfigured');
  await expect(page.getByTestId('update-banner')).toHaveCount(0);
  await menu(page, 'Settings');
  await page.getByRole('tab', { name: 'Updates' }).click();
  await expect(page.getByTestId('update-status')).toContainText('Updates are not configured');
});
