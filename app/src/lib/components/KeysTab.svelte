<script lang="ts">
  import { Button, Callout, Combobox, Input, RadioGroup, Table } from '@ruckc/truss-ui';
  import { KEY_NAME_RE, type SetupApi, type SetupState, type SetupTarget } from '../host/setup';

  let {
    api,
    target,
    info,
    onchange
  }: {
    api: SetupApi;
    target: SetupTarget;
    info: SetupState;
    /** Called after any change with the refreshed info. */
    onchange: (s: SetupState) => void;
  } = $props();

  let name = $state('');
  let secret = $state('');
  let description = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let confirmRemove = $state<string | null>(null);
  let useFor = $state<string | null>(null);
  let useProfile = $state('');
  let useScope = $state<'user' | 'project'>('user');

  const keys = $derived(info.keys.keys);
  const nameOk = $derived(KEY_NAME_RE.test(name.trim()));
  const replacing = $derived(keys.some((k) => k.name === name.trim()));

  async function refresh() {
    onchange(await api.state(target));
  }

  async function run(fn: () => Promise<void>) {
    busy = true;
    error = null;
    notice = null;
    try {
      await fn();
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function add(e: SubmitEvent) {
    e.preventDefault();
    if (!nameOk || !secret.trim()) return;
    const n = name.trim();
    const s = secret;
    // The secret leaves component state before the request is awaited.
    secret = '';
    await run(async () => {
      await api.keysAdd(target, n, s, description.trim() || undefined);
      name = '';
      description = '';
      notice = `Saved key "${n}".`;
    });
  }

  async function remove(n: string) {
    confirmRemove = null;
    await run(async () => {
      const r = await api.keysRemove(target, n);
      notice = r.stillUsedBy.length
        ? `Removed "${n}". Still referenced by ${r.stillUsedBy.join(', ')}.`
        : `Removed "${n}".`;
    });
  }

  function openUse(n: string) {
    useFor = n;
    useProfile = info.defaultProfile || info.profiles[0]?.name || '';
    useScope = 'user';
  }

  async function applyUse() {
    const n = useFor;
    if (!n || !useProfile.trim()) return;
    useFor = null;
    await run(async () => {
      await api.keysUse(target, n, useProfile.trim(), useScope === 'project');
      notice = `Profile "${useProfile.trim()}" now uses "${n}" (${useScope === 'project' ? 'this project only' : 'your profile'}).`;
    });
  }
</script>

<div class="keys">
  {#if error}<Callout intent="danger">{error}</Callout>{/if}
  {#if notice}<Callout intent="success" role="status">{notice}</Callout>{/if}
  {#if keys.length}
    <Table
      aria-label="API keys"
      columns={['Name', 'Description', 'Created', 'Key', 'Used by', 'Actions']}
    >
      {#each keys as k (k.name)}
        <tr>
          <td><code>{k.name}</code></td>
          <td>{k.description ?? ''}</td>
          <td>{k.createdAt ? k.createdAt.slice(0, 10) : ''}</td>
          <td><code>{k.masked}</code></td>
          <td>{k.usedBy.length ? k.usedBy.join(', ') : 'unused'}</td>
          <td class="rowactions">
            <Button size="sm" variant="outlined" disabled={busy} onclick={() => openUse(k.name)}>
              Use for profile
            </Button>
            {#if confirmRemove === k.name}
              <Button size="sm" intent="danger" disabled={busy} onclick={() => remove(k.name)}
                >Confirm remove</Button
              >
              <Button
                size="sm"
                variant="minimal"
                intent="neutral"
                onclick={() => (confirmRemove = null)}>Cancel</Button
              >
            {:else}
              <Button
                size="sm"
                variant="outlined"
                intent="danger"
                aria-label={`Remove ${k.name}`}
                disabled={busy}
                onclick={() => (confirmRemove = k.name)}>Remove</Button
              >
            {/if}
          </td>
        </tr>
      {/each}
    </Table>
  {:else}
    <p class="muted">
      No saved keys. Add one below; secrets are stored in a private file and never shown.
    </p>
  {/if}
  {#each info.keys.missing as m (m.name + m.usedBy)}
    <Callout intent="warning">
      {m.usedBy} references the missing key "{m.name}". Add a key with that name.
    </Callout>
  {/each}
  {#if useFor}
    <form
      class="box"
      aria-label={`Use key ${useFor}`}
      onsubmit={(e) => {
        e.preventDefault();
        void applyUse();
      }}
    >
      <strong>Use "{useFor}" for a profile</strong>
      <Combobox
        label="Profile"
        allowCustom
        autofocus
        options={info.profiles.map((p) => ({ value: p.name, label: p.name }))}
        bind:value={useProfile}
      />
      <RadioGroup
        label="Scope"
        options={[
          { value: 'user', label: 'My profile (all projects)' },
          { value: 'project', label: 'This project only' }
        ]}
        bind:value={useScope}
      />
      <div class="actions">
        <Button type="submit" disabled={!useProfile.trim()}>Apply</Button>
        <Button type="button" variant="outlined" onclick={() => (useFor = null)}>Cancel</Button>
      </div>
    </form>
  {/if}
  <form class="box" aria-label="Add key" onsubmit={add}>
    <strong>Add a key</strong>
    <Input label="Name" bind:value={name} spellcheck="false" autocomplete="off" />
    <Input
      label="API key"
      type="password"
      bind:value={secret}
      autocomplete="off"
      spellcheck="false"
    />
    <Input label="Description (optional)" bind:value={description} />
    {#if name && !nameOk}
      <Callout intent="warning">Name: letters, digits, ".", "-" or "_".</Callout>
    {/if}
    {#if replacing}
      <Callout intent="warning">A key named "{name.trim()}" exists and will be replaced.</Callout>
    {/if}
    <div class="actions">
      <Button type="submit" disabled={busy || !nameOk || !secret.trim()}>Save key</Button>
    </div>
  </form>
</div>

<style>
  .keys {
    display: grid;
    gap: var(--tr-space-3);
  }
  .box {
    border: 1px solid var(--tr-outline-variant);
    border-radius: var(--tr-radius-md);
    padding: var(--tr-space-3);
    display: grid;
    gap: var(--tr-space-2);
    max-width: 480px;
  }
  .actions {
    display: flex;
    gap: var(--tr-space-2);
  }
  .rowactions {
    display: flex;
    gap: var(--tr-space-1);
    flex-wrap: wrap;
  }
  p {
    margin: 0;
  }
</style>
