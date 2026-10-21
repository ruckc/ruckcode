<script lang="ts">
  // Test-only wrapper: holds the bound state and reports it.
  import type { Catalog, PermissionValue } from '../../host/modes';
  import PermissionsTable from '../PermissionsTable.svelte';
  let {
    initial,
    catalog,
    onchange
  }: {
    initial: Record<string, PermissionValue>;
    catalog: Catalog | null;
    onchange: (p: Record<string, PermissionValue>) => void;
  } = $props();
  // svelte-ignore state_referenced_locally
  let permissions = $state(initial);
  $effect(() => onchange($state.snapshot(permissions)));
</script>

<PermissionsTable bind:permissions {catalog} />
