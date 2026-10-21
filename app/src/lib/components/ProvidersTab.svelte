<script lang="ts">
  import type { SetupState } from '../host/setup';

  let { info, onadd }: { info: SetupState; onadd: () => void } = $props();
</script>

{#if info.profiles.length}
  <table class="profiles" aria-label="Providers">
    <thead>
      <tr><th>Profile</th><th>API</th><th>Endpoint</th><th>Model</th><th>Key</th></tr>
    </thead>
    <tbody>
      {#each info.profiles as p (p.name)}
        <tr>
          <td>
            {p.name}{#if p.name === info.defaultProfile}<span class="tag">default</span>{/if}
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
  <button type="button" class="primary" onclick={onadd}>Add provider…</button>
  <span class="muted">Manage keys on the API keys tab and per-mode models in Modes.</span>
</p>

<style>
  .profiles {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }
  th,
  td {
    text-align: left;
    padding: 4px 6px;
    border-bottom: 1px solid var(--border);
  }
  .mono {
    font-family: var(--mono, monospace);
    word-break: break-all;
  }
  .tag {
    margin-left: 6px;
    font-size: 11px;
    padding: 0 5px;
    border-radius: 4px;
    background: var(--code-bg);
  }
</style>
