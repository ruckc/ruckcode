<script lang="ts">
  import { Button, Tag, Table } from '@ruckc/truss-ui';
  import type { SetupState } from '../host/setup';

  let { info, onadd }: { info: SetupState; onadd: () => void } = $props();
</script>

{#if info.profiles.length}
  <Table aria-label="Providers" columns={['Profile', 'API', 'Endpoint', 'Model', 'Key']}>
    {#each info.profiles as p (p.name)}
      <tr>
        <td>
          {p.name}{#if p.name === info.defaultProfile}<Tag size="sm" intent="primary">default</Tag
            >{/if}
        </td>
        <td>{p.provider === 'anthropic' ? 'Anthropic' : 'OpenAI-compatible'}</td>
        <td class="mono">{p.baseUrl ?? '(default)'}</td>
        <td class="mono" title={p.models.join(', ')}>
          {p.model}{p.models.length > 1 ? ` +${p.models.length - 1}` : ''}
        </td>
        <td>{p.apiKeyName ?? (p.apiKeyEnv ? `$${p.apiKeyEnv}` : 'none')}</td>
      </tr>
    {/each}
  </Table>
{:else}
  <p class="muted">No providers are configured yet.</p>
{/if}
<p>
  <Button onclick={onadd}>Add provider…</Button>
  <span class="muted">Manage keys on the API keys tab and per-mode models in Modes.</span>
</p>

<style>
  .mono {
    font-family: var(--tr-font-mono);
    word-break: break-all;
  }
</style>
