<script lang="ts">
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
  {#if error}<p class="errbox" role="alert">{error}</p>{/if}
  {#if notice}<p class="ok" role="status">{notice}</p>{/if}

  {#if keys.length}
    <table aria-label="API keys">
      <thead>
        <tr
          ><th>Name</th><th>Description</th><th>Created</th><th>Key</th><th>Used by</th><th
          ></th></tr
        >
      </thead>
      <tbody>
        {#each keys as k (k.name)}
          <tr>
            <td><code>{k.name}</code></td>
            <td>{k.description ?? ''}</td>
            <td>{k.createdAt ? k.createdAt.slice(0, 10) : ''}</td>
            <td><code>{k.masked}</code></td>
            <td>{k.usedBy.length ? k.usedBy.join(', ') : 'unused'}</td>
            <td class="rowactions">
              <button type="button" disabled={busy} onclick={() => openUse(k.name)}
                >Use for profile</button
              >
              {#if confirmRemove === k.name}
                <button type="button" class="danger" disabled={busy} onclick={() => remove(k.name)}
                  >Confirm remove</button
                >
                <button type="button" class="ghost" onclick={() => (confirmRemove = null)}
                  >Cancel</button
                >
              {:else}
                <button
                  type="button"
                  class="danger"
                  aria-label={`Remove ${k.name}`}
                  disabled={busy}
                  onclick={() => (confirmRemove = k.name)}>Remove</button
                >
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <p class="muted">
      No saved keys. Add one below; secrets are stored in a private file and never shown.
    </p>
  {/if}

  {#each info.keys.missing as m (m.name + m.usedBy)}
    <p class="warn">{m.usedBy} references the missing key "{m.name}". Add a key with that name.</p>
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
      <label class="field">
        <span>Profile</span>
        <input type="text" list="profile-names" bind:value={useProfile} data-autofocus />
        <datalist id="profile-names">
          {#each info.profiles as p (p.name)}<option value={p.name}></option>{/each}
        </datalist>
      </label>
      <fieldset>
        <legend>Scope</legend>
        <label class="choice">
          <input type="radio" name="use-scope" value="user" bind:group={useScope} /> My profile (all projects)
        </label>
        <label class="choice">
          <input type="radio" name="use-scope" value="project" bind:group={useScope} /> This project only
        </label>
      </fieldset>
      <div class="actions">
        <button type="submit" class="primary" disabled={!useProfile.trim()}>Apply</button>
        <button type="button" onclick={() => (useFor = null)}>Cancel</button>
      </div>
    </form>
  {/if}

  <form class="box" aria-label="Add key" onsubmit={add}>
    <strong>Add a key</strong>
    <label class="field">
      <span>Name</span>
      <input type="text" bind:value={name} spellcheck="false" autocomplete="off" />
    </label>
    <label class="field">
      <span>API key</span>
      <input type="password" bind:value={secret} autocomplete="off" spellcheck="false" />
    </label>
    <label class="field">
      <span>Description (optional)</span>
      <input type="text" bind:value={description} />
    </label>
    {#if name && !nameOk}<p class="warn">Name: letters, digits, ".", "-" or "_".</p>{/if}
    {#if replacing}<p class="warn">A key named "{name.trim()}" exists and will be replaced.</p>{/if}
    <div class="actions">
      <button type="submit" class="primary" disabled={busy || !nameOk || !secret.trim()}
        >Save key</button
      >
    </div>
  </form>
</div>

<style>
  .keys {
    display: grid;
    gap: 10px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th,
  td {
    text-align: left;
    padding: 4px 6px;
    border-bottom: 1px solid var(--border);
    vertical-align: top;
  }
  .rowactions {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .box {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
    display: grid;
    gap: 6px;
  }
  .field {
    display: grid;
    gap: 2px;
  }
  .field input {
    width: 100%;
  }
  fieldset {
    border: 1px solid var(--border);
    border-radius: 6px;
    margin: 0;
  }
  .errbox {
    color: var(--err);
    margin: 0;
  }
  .ok {
    color: var(--ok);
    margin: 0;
  }
  .warn {
    color: var(--warn);
    margin: 0;
  }
  @media (max-width: 640px) {
    th:nth-child(3),
    td:nth-child(3) {
      display: none;
    }
  }
</style>
