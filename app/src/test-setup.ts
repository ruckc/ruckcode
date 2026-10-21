import { afterEach } from 'vitest';

// Only component tests (jsdom) need DOM matchers and cleanup.
if (typeof document !== 'undefined') {
  await import('@testing-library/jest-dom/vitest');
  const { cleanup } = await import('@testing-library/svelte');
  afterEach(() => cleanup());
}
