<script lang="ts">
  let { kind }: { kind: string } = $props();

  // Small 16x16 stroke icons, one per ACP tool kind.
  const paths: Record<string, string> = {
    read: 'M3 2h7l3 3v9H3z M10 2v3h3 M5.5 8h5 M5.5 11h5',
    edit: 'M3 13l1-3 7-7 2 2-7 7z M10 4l2 2',
    delete: 'M3 4h10 M6 4V2.5h4V4 M4.5 4l.5 9.5h6l.5-9.5',
    move: 'M2 8h10 M9 4l4 4-4 4',
    search: 'M7 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10z M11 11l3.5 3.5',
    execute: 'M2 3h12v10H2z M4.5 6.5L7 8.5l-2.5 2 M8.5 10.5h3',
    think: 'M8 2a4.5 4.5 0 0 0-2 8.5V12h4v-1.5A4.5 4.5 0 0 0 8 2z M6.5 14h3',
    fetch: 'M8 2v8 M5 7l3 3 3-3 M3 13h10',
    switch_mode: 'M2 5h10 M9 2l3 3-3 3 M14 11H4 M7 8l-3 3 3 3',
    other: 'M8 2l1.8 4 4.2.4-3.2 2.8 1 4.1L8 11.2 4.2 13.3l1-4.1L2 6.4 6.2 6z'
  };
  const d = $derived(paths[kind] ?? paths.other);
</script>

<svg
  class="kind"
  viewBox="0 0 16 16"
  width="16"
  height="16"
  fill="none"
  stroke="currentColor"
  stroke-width="1.4"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path {d} />
</svg>

<style>
  .kind {
    flex: none;
    color: var(--muted);
  }
</style>
