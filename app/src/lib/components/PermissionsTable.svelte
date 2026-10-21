<script lang="ts">
  import type { Catalog, PermissionValue } from '../host/modes';
  import { globProblem } from '../modes/glob';

  let {
    permissions = $bindable(),
    catalog,
    errors = {}
  }: {
    permissions: Record<string, PermissionValue>;
    catalog: Catalog | null;
    /** Messages by tool or glob. */
    errors?: Record<string, string>;
  } = $props();

  let pattern = $state('');
  let value = $state<PermissionValue>('ask');

  const names = $derived([
    ...(catalog?.builtinTools.map((t) => t.name) ?? []),
    'mise__*',
    'mcp__*'
  ]);
  const rows = $derived(Object.entries(permissions));
  const problem = $derived(
    pattern.trim()
      ? globProblem(pattern.trim()) && `Invalid pattern: ${globProblem(pattern.trim())}`
      : null
  );
  const exists = $derived(pattern.trim() in permissions);

  function defaultFor(tool: string): string | null {
    const t = catalog?.builtinTools.find((x) => x.name === tool);
    return t ? (t.readOnly ? 'allow' : 'ask') : null;
  }

  function add() {
    const p = pattern.trim();
    if (!p || problem) return;
    permissions = { ...permissions, [p]: value };
    pattern = '';
  }

  function set(p: string, v: PermissionValue) {
    permissions = { ...permissions, [p]: v };
  }

  function remove(p: string) {
    const next = { ...permissions };
    delete next[p];
    permissions = next;
  }
</script>

{#if rows.length}
  <table aria-label="Permissions">
    <thead>
      <tr><th>Tool or glob</th><th>Permission</th><th><span class="sr-only">Actions</span></th></tr>
    </thead>
    <tbody>
      {#each rows as [p, v] (p)}
        <tr>
          <td>
            <code>{p}</code>
            {#if defaultFor(p)}<span class="muted small">(default {defaultFor(p)})</span>{/if}
            {#if errors[p]}<div class="err small">{errors[p]}</div>{/if}
          </td>
          <td>
            <select
              aria-label={`Permission for ${p}`}
              value={v}
              onchange={(e) => set(p, e.currentTarget.value as PermissionValue)}
            >
              {#each ['allow', 'ask', 'deny'] as o (o)}<option value={o}>{o}</option>{/each}
            </select>
          </td>
          <td
            ><button
              type="button"
              class="ghost"
              aria-label={`Remove rule ${p}`}
              onclick={() => remove(p)}>&times;</button
            ></td
          >
        </tr>
      {/each}
    </tbody>
  </table>
{:else}
  <p class="muted small">No rules: tools use their defaults (read-only tools allow, others ask).</p>
{/if}
<div class="add">
  <input
    type="text"
    aria-label="Tool or glob"
    placeholder="Tool name or glob, e.g. Bash or mise__*"
    list="perm-names"
    bind:value={pattern}
    spellcheck="false"
    onkeydown={(e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        add();
      }
    }}
  />
  <datalist id="perm-names"
    >{#each names as n (n)}<option value={n}></option>{/each}</datalist
  >
  <select aria-label="New rule permission" bind:value>
    {#each ['allow', 'ask', 'deny'] as o (o)}<option value={o}>{o}</option>{/each}
  </select>
  <button type="button" disabled={!pattern.trim() || !!problem} onclick={add}>
    {exists ? 'Replace rule' : 'Add rule'}
  </button>
</div>
{#if problem}<p class="err small" role="status">{problem}</p>{/if}

<style>
  table {
    border-collapse: collapse;
    width: 100%;
    margin-bottom: 6px;
  }
  th {
    text-align: left;
    font-weight: 600;
    font-size: 12px;
    color: var(--muted);
  }
  th,
  td {
    padding: 3px 6px 3px 0;
    border-bottom: 1px solid var(--border);
  }
  .add {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .add input {
    flex: 1;
    min-width: 200px;
  }
  .small {
    font-size: 12px;
  }
  .err {
    color: var(--err);
    margin: 0;
  }
</style>
