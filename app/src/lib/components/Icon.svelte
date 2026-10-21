<script lang="ts">
  export type IconName = 'more' | 'folder' | 'refresh' | 'power' | 'close' | 'back' | 'plus';
  let { name }: { name: IconName } = $props();
</script>

<svg
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#if name === 'more'}
    <circle cx="5" cy="12" r="1" /><circle cx="12" cy="12" r="1" /><circle cx="19" cy="12" r="1" />
  {:else if name === 'folder'}
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  {:else if name === 'refresh'}
    <path d="M21 12a9 9 0 1 1-2.64-6.36" /><path d="M21 3v6h-6" />
  {:else if name === 'power'}
    <path d="M12 2v10" /><path d="M18.36 6.64a9 9 0 1 1-12.73 0" />
  {:else if name === 'close'}
    <path d="M18 6 6 18" /><path d="m6 6 12 12" />
  {:else if name === 'back'}
    <path d="M19 12H5" /><path d="m12 19-7-7 7-7" />
  {:else if name === 'plus'}
    <path d="M12 5v14" /><path d="M5 12h14" />
  {/if}
</svg>
