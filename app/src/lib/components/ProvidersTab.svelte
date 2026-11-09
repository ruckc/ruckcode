<script lang="ts">
  import { Button, Tag } from '@ruckc/truss-ui';
  import type { SetupState } from '../host/setup';

  let { info, onadd }: { info: SetupState; onadd: () => void } = $props();
</script>

{#if info.profiles.length}
  <table class="data" aria-label="Providers">
    <thead>
      <tr><th>Profile</th><th>API</th><th>Endpoint</th><th>Model</th><th>Key</th></tr>
    </thead>
    <tbody>
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
    </tbody>
  </table>
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
