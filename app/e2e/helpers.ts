import { expect, type Locator, type Page } from '@playwright/test';

/** An option's accessible name continues with its description, so a plain string matches the start. */
const startsWith = (text: string) =>
  new RegExp(`^${text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(\\s|$)`);

/**
 * Choose an option in a truss-ui Select or Combobox. They are a button (role=combobox) plus a
 * listbox in the browser's top layer (`.tr-popover`), not a native <select>.
 */
export async function choose(trigger: Locator, option: string | RegExp): Promise<void> {
  await trigger.click();
  await trigger
    .page()
    .locator('.tr-popover')
    .getByRole('option', { name: typeof option === 'string' ? startsWith(option) : option })
    .click();
}

/** Open the mock app (fast scripted agent) and connect to the demo project. */
export async function openApp(page: Page, delay = 2): Promise<void> {
  await page.goto(`/?mock=1&delay=${delay}`);
  await page.getByRole('button', { name: 'Connect' }).click();
  await connected(page);
}

/** The agent is connected and a session is ready for input. */
export async function connected(page: Page): Promise<void> {
  // The chat composer (the mode editor also has a "Prompt" field).
  await expect(page.getByRole('textbox', { name: 'Prompt' }).first()).toBeEnabled();
}

/** Pick an item from the top bar's overflow menu. */
export async function menu(page: Page, item: string): Promise<void> {
  await page.getByRole('button', { name: 'More', exact: true }).click();
  await page.getByRole('menuitem', { name: item, exact: true }).click();
}

/** Open the project & session picker. */
export async function picker(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Projects and sessions' }).click();
  await expect(page.getByRole('dialog', { name: 'Open' })).toBeVisible();
}

/** Settings > Modes (the modes.toml editor). */
export async function openModes(page: Page): Promise<void> {
  await menu(page, 'Settings');
  await page.getByRole('dialog', { name: 'Settings' }).getByRole('tab', { name: 'Modes' }).click();
}

export async function closeSettings(page: Page): Promise<void> {
  await page
    .getByRole('dialog', { name: 'Settings' })
    .getByRole('button', { name: 'Close dialog' })
    .click();
}

export async function send(page: Page, text: string): Promise<void> {
  await page.getByRole('textbox', { name: 'Prompt' }).fill(text);
  await page.getByRole('textbox', { name: 'Prompt' }).press('Enter');
}
