<script lang="ts">
  // Test-only wrapper: holds the bound state and reports it.
  import ChipInput from '../ChipInput.svelte';
  let {
    initial,
    onvalues,
    ...rest
  }: {
    initial: string[];
    onvalues: (v: string[]) => void;
    label: string;
    suggestions?: string[];
    preview?: (v: string) => string | null;
    problem?: (v: string) => string | null;
  } = $props();
  // svelte-ignore state_referenced_locally
  let values = $state(initial);
  $effect(() => onvalues($state.snapshot(values)));
</script>

<ChipInput bind:values {...rest} />
