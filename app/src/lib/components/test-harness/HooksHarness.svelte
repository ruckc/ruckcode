<script lang="ts">
  // Test-only wrapper: holds the bound state and reports it.
  import type { Catalog, Issue, ModeDto } from '../../host/modes';
  import HooksEditor from '../HooksEditor.svelte';
  let {
    initial,
    catalog,
    issues,
    onchange
  }: {
    initial: ModeDto['hooks'];
    catalog: Catalog | null;
    issues: Issue[];
    onchange: (h: ModeDto['hooks']) => void;
  } = $props();
  // svelte-ignore state_referenced_locally
  let hooks = $state(initial);
  $effect(() => onchange($state.snapshot(hooks) as ModeDto['hooks']));
</script>

<HooksEditor bind:hooks {catalog} {issues} base="modes[0].hooks" />
