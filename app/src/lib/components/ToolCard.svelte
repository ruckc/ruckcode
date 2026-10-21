<script lang="ts">
  import type { ChatItem } from '../state/reducer';

  let { tool }: { tool: Extract<ChatItem, { kind: 'tool' }> } = $props();
</script>

<div class="tool" data-status={tool.status}>
  <div class="head">
    <span class="badge">{tool.status.replace('_', ' ')}</span>
    <span class="title">{tool.title}</span>
    <span class="muted">{tool.toolKind}</span>
  </div>
  {#if tool.output}<pre>{tool.output}</pre>{/if}
</div>
