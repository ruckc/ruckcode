<script lang="ts">
  import { onDestroy, type Snippet } from 'svelte';
  import { Dialog } from '@ruckc/truss-ui';
  import type { HTMLDialogAttributes } from 'svelte/elements';

  /**
   * A truss-ui `Dialog` that is open for as long as it is mounted and hands focus back to whatever had
   * it when it goes away. Esc, the close button and (optionally) a backdrop click do not close it
   * themselves: they call `onclose`, and the owner decides by unmounting.
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
    /** A click outside the dialog asks to close. Off for prompts where a stray click must not answer. */
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

  /** Veto every user-initiated close; the owner unmounts the modal if it wants it gone. */
  function beforeclose() {
    onclose?.();
    return false;
  }
</script>

<Dialog
  bind:open
  {title}
  {size}
  {width}
  {alert}
  closeOnBackdrop={backdropClose}
  {beforeclose}
  {...rest}
  {children}
  {footer}
/>
