<script lang="ts">
  import { Button, Combobox, Select, Table } from '@ruckc/truss-ui';
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

  const PERMS: PermissionValue[] = ['allow', 'ask', 'deny'];
  const permOptions = PERMS.map((o) => ({ value: o, label: o }));

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
  <Table aria-label="Permissions" columns={['Tool or glob', 'Permission', 'Actions']}>
    {#each rows as [p, v] (p)}
      <tr>
        <td>
          <code>{p}</code>
          {#if defaultFor(p)}<span class="muted small">(default {defaultFor(p)})</span>{/if}
          {#if errors[p]}<div class="err small">{errors[p]}</div>{/if}
        </td>
        <td class="perm">
          <Select
            size="sm"
            searchable={false}
            ariaLabel={`Permission for ${p}`}
            options={permOptions}
            value={v}
            onchange={(x) => set(p, x)}
          />
        </td>
        <td>
          <Button
            size="sm"
            variant="minimal"
            intent="neutral"
            icon="cross"
            aria-label={`Remove rule ${p}`}
            onclick={() => remove(p)}
          />
        </td>
      </tr>
    {/each}
  </Table>
{:else}
  <p class="muted small">No rules: tools use their defaults (read-only tools allow, others ask).</p>
{/if}
<div class="add">
  <div class="pattern">
    <Combobox
      allowCustom
      ariaLabel="Tool or glob"
      placeholder="Tool name or glob, e.g. Bash or mise__*"
      options={names.map((n) => ({ value: n, label: n }))}
      bind:text={pattern}
      spellcheck="false"
      onkeydown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          add();
        }
      }}
    />
  </div>
  <div class="perm">
    <Select
      size="sm"
      searchable={false}
      ariaLabel="New rule permission"
      options={permOptions}
      {value}
      onchange={(x) => (value = x)}
    />
  </div>
  <Button size="sm" variant="outlined" disabled={!pattern.trim() || !!problem} onclick={add}>
    {exists ? 'Replace rule' : 'Add rule'}
  </Button>
</div>
{#if problem}<p class="err small" role="status">{problem}</p>{/if}

<style>
  .add {
    display: flex;
    gap: var(--tr-space-2);
    flex-wrap: wrap;
    align-items: center;
  }
  .pattern {
    flex: 1;
    min-width: 200px;
  }
  .perm {
    min-width: 110px;
  }
  .add .perm {
    width: 120px;
  }
  .small {
    font-size: var(--tr-text-sm);
  }
  .err {
    color: var(--tr-danger);
    margin: 0;
  }
</style>
