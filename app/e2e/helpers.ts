import { expect, type Page } from '@playwright/test';

/** Open the mock app (fast scripted agent) and connect to the demo project. */
export async function openApp(page: Page, delay = 2): Promise<void> {
  await page.goto(`/?mock=1&delay=${delay}`);
  await page.getByRole('button', { name: 'Connect' }).click();
  await expect(page.getByRole('button', { name: 'New session', exact: true })).toBeVisible();
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeEnabled();
}

export async function send(page: Page, text: string): Promise<void> {
  await page.getByRole('textbox', { name: 'Prompt' }).fill(text);
  await page.getByRole('textbox', { name: 'Prompt' }).press('Enter');
}
