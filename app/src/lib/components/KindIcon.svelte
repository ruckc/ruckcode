<script lang="ts">
  import { Icon } from '@ruckc/truss-ui';
  import type { IconName } from '@ruckc/truss-ui';

  let { kind }: { kind: string } = $props();

  // One truss glyph per ACP tool kind.
  const icons: Record<string, IconName> = {
    read: 'file',
    edit: 'edit',
    delete: 'trash',
    move: 'arrow-right',
    search: 'search',
    execute: 'terminal',
    think: 'sparkles',
    fetch: 'download',
    switch_mode: 'refresh',
    other: 'zap'
  };
</script>

<Icon name={icons[kind] ?? icons.other} />
