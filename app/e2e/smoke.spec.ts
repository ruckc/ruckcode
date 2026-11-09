import { expect, test, type Page } from '@playwright/test';
import { menu, openApp, picker, send } from './helpers';

// Every screen renders in both colour schemes, without console errors. The detailed behaviour of
// each screen is covered by the feature specs; this guards against a screen failing to mount.
for (const scheme of ['light', 'dark'] as const) {
  test.describe(`smoke (${scheme})`, () => {
    test.use({ colorScheme: scheme });

    let errors: string[] = [];
    test.beforeEach(({ page }) => {
      errors = [];
      page.on('pageerror', (e) => errors.push(e.message));
      page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
    });
    test.afterEach(() => expect(errors).toEqual([]));

    const themed = async (page: Page) => {
      // truss-ui follows the OS scheme: its surface token differs between light and dark.
      const bg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
      expect(bg).toBe(scheme === 'dark' ? 'rgb(17, 19, 24)' : 'rgb(251, 251, 254)');
    };

    test('project picker, chat, markdown and tool cards', async ({ page }) => {
      await page.goto('/?mock=1&delay=2');
      await expect(page.getByRole('button', { name: 'Connect' })).toBeVisible();
      await themed(page);
      await page.getByRole('button', { name: 'Connect' }).click();
      await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeEnabled();
      await send(page, 'mock:md');
      await expect(page.getByRole('log', { name: 'Conversation' })).toContainText('Heading');
      await send(page, 'mock:bash');
      await expect(page.getByTestId('tool-card')).toBeVisible();
    });

    test('permission and question dialogs', async ({ page }) => {
      await openApp(page);
      await send(page, 'mock:write src/a.txt body');
      await expect(page.getByRole('alertdialog')).toBeVisible();
      await page.keyboard.press('n');
      await send(page, 'mock:askmulti');
      await expect(page.getByRole('dialog', { name: /questions/ })).toBeVisible();
      await page.keyboard.press('Escape');
      await expect(page.getByRole('dialog', { name: /questions/ })).toHaveCount(0);
    });

    test('rate limit banner, plan panel and sessions picker', async ({ page }) => {
      await openApp(page);
      await send(page, 'mock:ratelimit');
      await expect(page.getByTestId('rate-wait')).toBeVisible();
      await page.getByRole('button', { name: 'Cancel the wait' }).click();
      await picker(page);
      await page
        .getByRole('list', { name: 'Sessions' })
        .getByRole('button', { name: /Fix the login bug/ })
        .click();
      await expect(page.getByRole('complementary', { name: 'Plan' })).toContainText(
        'Plan complete'
      );
    });

    test('resume dialog', async ({ page }) => {
      await page.goto('/?mock=1&delay=2&unfinished=1');
      await page.getByRole('button', { name: 'Connect' }).click();
      await expect(page.getByRole('dialog', { name: 'Pick up where you left off?' })).toBeVisible();
    });

    test('settings tabs, modes editor and transcripts', async ({ page }) => {
      await openApp(page);
      await menu(page, 'Settings');
      const settings = page.getByRole('dialog', { name: 'Settings' });
      for (const tab of ['Providers', 'API keys', 'Modes', 'Project']) {
        await settings.getByRole('tab', { name: tab }).click();
        await expect(settings.getByRole('tabpanel')).toBeVisible();
      }
      await settings.getByRole('button', { name: 'Close dialog' }).click();
      await menu(page, 'Transcripts');
      await expect(page.getByRole('button', { name: 'Back to chat' })).toBeVisible();
    });

    test('first-run setup wizard', async ({ page }) => {
      await page.goto('/?mock=1&delay=2&setup=fresh');
      await page.getByRole('button', { name: 'Connect', exact: true }).click();
      await expect(page.getByRole('dialog', { name: 'Set up ruckcode' })).toBeVisible();
    });
  });
}
