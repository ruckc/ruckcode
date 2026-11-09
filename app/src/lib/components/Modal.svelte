<script lang="ts">
  import { onDestroy, type Snippet } from 'svelte';
  import { Dialog } from '@ruckc/truss-ui';
  import type { HTMLDialogAttributes } from 'svelte/elements';

  /**
   * A truss-ui `Dialog` that is open for as long as it is mounted, hands focus back to whatever had
   * it when it goes away, and can ignore clicks on the backdrop.
   *
   * TODO(truss-ui#135): `Dialog` should offer `closeOnBackdrop` / `closable` props (and `Escape` opt-out)
   * so this wrapper's capture-phase guard and focus restore are not needed.
   */
  let {
    title,
    size = 'md',
    width,
    alert = false,
    backdropClose = true,
    onclose,
    children,
    footer,
    ...rest
  }: {
    title: string;
    size?: 'sm' | 'md' | 'lg' | 'xl' | 'full';
    /** Any CSS width; overrides `size`. */
    width?: string;
    /** `role="alertdialog"`: a prompt that needs an answer. */
    alert?: boolean;
    /** A click outside the dialog closes it. Off for prompts where a stray click must not answer. */
    backdropClose?: boolean;
    /** Esc, the close button (and a backdrop click when `backdropClose`) ask to close. */
    onclose?: () => void;
    children: Snippet;
    footer?: Snippet;
  } & Omit<
    HTMLDialogAttributes,
    'open' | 'title' | 'children' | 'role' | 'onclose' | 'onclick'
  > = $props();

  let open = $state(true);
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  onDestroy(() => {
    if (previous?.isConnected) previous.focus();
  });

  /**
   * Esc and the close button close the native dialog; `onclose` decides whether that is final. When
   * the owner keeps this modal mounted (e.g. "keep editing" on unsaved changes), it opens again.
   */
  function closed() {
    // Reopened before the owner reacts, so anything it opens meanwhile (a confirmation) stacks above.
    open = true;
    onclose?.();
  }

  /** Runs before truss-ui's own handler on the dialog element, which closes on a backdrop click. */
  function guard(e: MouseEvent) {
    if (!backdropClose && e.target === e.currentTarget) e.stopImmediatePropagation();
  }
</script>

<Dialog
  bind:open
  {title}
  {size}
  {width}
  {alert}
  onclose={closed}
  onclickcapture={guard}
  {...rest}
  {children}
  {footer}
/>
