import { expect, type Page } from '@playwright/test';

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
  await page.getByRole('button', { name: 'Close settings' }).click();
}

export async function send(page: Page, text: string): Promise<void> {
  await page.getByRole('textbox', { name: 'Prompt' }).fill(text);
  await page.getByRole('textbox', { name: 'Prompt' }).press('Enter');
}
