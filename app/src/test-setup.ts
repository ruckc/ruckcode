import { afterEach } from 'vitest';

// Only component tests (jsdom) need DOM matchers and cleanup.
if (typeof document !== 'undefined') {
  await import('@testing-library/jest-dom/vitest');
  const { cleanup } = await import('@testing-library/svelte');
  afterEach(() => cleanup());

  // jsdom has <dialog> but not showModal()/close().
  const dlg = HTMLDialogElement.prototype as unknown as Record<string, unknown>;
  dlg.showModal ??= function (this: HTMLDialogElement) {
    this.setAttribute('open', '');
  };
  dlg.close ??= function (this: HTMLDialogElement) {
    if (!this.hasAttribute('open')) return;
    this.removeAttribute('open');
    this.dispatchEvent(new Event('close'));
  };

  // TODO(truss-ui#146): drop these stubs once truss-ui guards or ships a test setup.
  // jsdom lacks what truss-ui's popovers, Select and DiffView use.
  const proto = HTMLElement.prototype as unknown as Record<string, unknown>;
  proto.showPopover ??= function () {};
  proto.hidePopover ??= function () {};
  proto.scrollIntoView ??= function () {};
  const g = globalThis as unknown as Record<string, unknown>;
  g.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  g.matchMedia ??= (q: string) => ({
    matches: false,
    media: q,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    onchange: null,
    dispatchEvent: () => false
  });
}
