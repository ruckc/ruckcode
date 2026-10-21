<script lang="ts">
  import { STEPS, STEP_LABEL, choiceValue, type SetupWizard } from '../state/wizard.svelte';
  import DiffText from './DiffText.svelte';

  let {
    wizard,
    onreconnect,
    onclose
  }: { wizard: SetupWizard; onreconnect: () => void; onclose: () => void } = $props();

  const w = $derived(wizard);
  const steps = STEPS;

  function submit(e: SubmitEvent) {
    e.preventDefault();
    void w.next();
  }

  const missingNames = $derived(w.state.keys.missing.map((m) => m.name));
</script>

<div class="wizard">
  <ol class="steps" aria-label="Setup steps">
    {#each steps as s, i (s)}
      <li
        class:current={w.step === s}
        class:past={i < w.stepIndex || w.step === 'done'}
        aria-current={w.step === s ? 'step' : undefined}
      >
        {#if i < w.stepIndex}
          <button type="button" class="ghost" onclick={() => w.goto(s)}
            >{i + 1}. {STEP_LABEL[s]}</button
          >
        {:else}
          <span>{i + 1}. {STEP_LABEL[s]}</span>
        {/if}
      </li>
    {/each}
  </ol>

  {#if w.step === 'done'}
    <section aria-labelledby="done-title" data-step="done">
      <h3 id="done-title">Saved</h3>
      {#if w.report}
        <ul class="report">
          <li>Profile settings: <code>{w.report.configPath}</code></li>
          {#if w.report.credentialsPath}<li>
              API key: <code>{w.report.credentialsPath}</code> (private)
            </li>{/if}
          {#if w.report.projectConfigPath}
            <li>Key reference: <code>{w.report.projectConfigPath}</code> (this project only)</li>
          {/if}
          {#if w.report.modesPath}
            <li>Per-mode models ({w.report.modesUpdated}): <code>{w.report.modesPath}</code></li>
          {/if}
        </ul>
      {/if}
      <p>The running agent keeps the old settings until it reconnects.</p>
      <div class="actions">
        <button type="button" class="primary" data-autofocus onclick={onreconnect}>
          Reconnect the agent now
        </button>
        <button type="button" onclick={onclose}>Later</button>
      </div>
    </section>
  {:else}
    <form onsubmit={submit} data-step={w.step}>
      {#if w.step === 'key'}
        <fieldset>
          <legend>How should ruckcode authenticate?</legend>
          <label class="choice">
            <input
              type="radio"
              name="auth"
              value="new"
              checked={w.authKind === 'new'}
              onchange={() => w.setAuth('new')}
            />
            <span>Paste an API key</span>
            <span class="muted">Saved to your private per-user credentials file</span>
          </label>
          <label class="choice" class:off={w.keyNames.length === 0}>
            <input
              type="radio"
              name="auth"
              value="existing"
              checked={w.authKind === 'existing'}
              disabled={w.keyNames.length === 0}
              onchange={() => w.setAuth('existing')}
            />
            <span>Use a saved key</span>
            <span class="muted"
              >{w.keyNames.length ? 'From your API keys' : 'No saved keys yet'}</span
            >
          </label>
          <label class="choice">
            <input
              type="radio"
              name="auth"
              value="none"
              checked={w.authKind === 'none'}
              onchange={() => w.setAuth('none')}
            />
            <span>No key</span>
            <span class="muted">For local servers without authentication</span>
          </label>
        </fieldset>
        {#if w.authKind === 'new'}
          <label class="field">
            <span>API key</span>
            <input
              type="password"
              data-autofocus
              bind:value={w.newKeySecret}
              oninput={() => w.keyEdited()}
              autocomplete="off"
              spellcheck="false"
              aria-describedby="secret-note"
            />
          </label>
          <p id="secret-note" class="muted">
            Kept in memory until you save, then stored in <code>credentials.toml</code> (private file)
            in your user config folder; never in the project or in config files. It is never shown again.
          </p>
          <label class="field">
            <span>Description (optional)</span>
            <input type="text" bind:value={w.newKeyDescription} />
          </label>
        {:else if w.authKind === 'existing'}
          <label class="field">
            <span>Saved key</span>
            <select bind:value={w.existingKey} onchange={() => w.setAuth('existing')}>
              {#each w.state.keys.keys as k (k.name)}
                <option value={k.name}
                  >{k.name} ({k.masked}){k.description ? ` - ${k.description}` : ''}</option
                >
              {/each}
            </select>
          </label>
        {/if}
        {#if missingNames.length}
          <p class="warn">Some profiles reference missing keys: {missingNames.join(', ')}.</p>
        {/if}
      {:else if w.step === 'endpoint'}
        <label class="field">
          <span>Endpoint URL</span>
          <input
            type="text"
            data-autofocus
            value={w.endpoint}
            oninput={(e) => w.setEndpoint(e.currentTarget.value)}
            placeholder="https://"
            spellcheck="false"
            autocomplete="off"
            aria-required="true"
            aria-invalid={w.endpointError ? 'true' : undefined}
            aria-describedby="endpoint-msg"
          />
        </label>
        <p id="endpoint-msg" class="problem" role="status" data-testid="endpoint-error">
          {w.endpointError ?? ''}
        </p>
        <p class="muted">
          Anthropic or any OpenAI-compatible server. The next step checks which API answers.
        </p>
      {:else if w.step === 'check'}
        <p class="muted">Checking <code>{w.endpoint}</code></p>
        {#if w.checkPhase === 'checking'}
          <p class="loading" role="status" data-testid="check-status">
            <span class="spinner" aria-hidden="true"></span> Probing the Anthropic and OpenAI-compatible
            APIs...
          </p>
        {:else if w.checkPhase === 'ok'}
          <p class="ok" role="status" data-testid="check-status">
            &#10003; {w.probe?.message}{w.forced ? ' (flavor chosen manually)' : ''}
          </p>
          <div class="actions">
            <button type="button" onclick={() => w.overrideKind()}>
              Use the {w.kind === 'anthropic' ? 'OpenAI-compatible' : 'Anthropic'} API instead
            </button>
            <button type="button" onclick={() => w.runCheck(w.forced)}>Re-check</button>
          </div>
        {:else if w.checkPhase === 'failed'}
          <div class="errbox" role="alert">
            <strong>Could not verify the endpoint.</strong>
            <span data-testid="check-error">{w.checkError}</span>
            <div class="actions">
              <button type="button" onclick={() => w.runCheck(w.forced)}>Retry</button>
              <button type="button" onclick={() => w.overrideKind()}>
                Try the {w.kind === 'anthropic' ? 'OpenAI-compatible' : 'Anthropic'} API
              </button>
              <button type="button" onclick={() => w.back()}>Change URL</button>
              <button type="button" onclick={() => w.continueManually()}>
                Continue without a model list
              </button>
            </div>
          </div>
        {/if}
      {:else if w.step === 'models'}
        {#if w.fetched.length}
          <div class="toolbar">
            <input
              type="text"
              aria-label="Filter models"
              placeholder="Filter models"
              bind:value={w.filter}
              data-autofocus
            />
            <button type="button" onclick={() => w.toggleAllFiltered()}>
              {w.filtered.length > 0 && w.filtered.every((m) => w.selected.includes(m))
                ? 'Deselect all'
                : 'Select all'}
            </button>
            <span class="muted">{w.selected.length} of {w.available.length} selected</span>
          </div>
          <ul class="models" aria-label="Models">
            {#each w.filtered as m (m)}
              <li>
                <label class="choice">
                  <input
                    type="checkbox"
                    checked={w.selected.includes(m)}
                    onchange={() => w.toggleModel(m)}
                  />
                  <code>{m}</code>
                </label>
                {#if w.selected.includes(m)}
                  <label class="default">
                    <input
                      type="radio"
                      name="default-model"
                      aria-label={`Default model ${m}`}
                      checked={w.defaultModel === m}
                      onchange={() => (w.defaultModel = m)}
                    />
                    default
                  </label>
                {/if}
              </li>
            {:else}
              <li class="muted">No model matches the filter.</li>
            {/each}
          </ul>
        {:else}
          <p class="muted">No model list is available: type the model ids you want to use.</p>
        {/if}
        <div class="manual">
          <label class="field">
            <span>Add model ids manually</span>
            <span class="row">
              <input
                type="text"
                bind:value={w.manualText}
                placeholder="model-a, model-b"
                spellcheck="false"
                onkeydown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    e.stopPropagation();
                    w.addManual();
                  }
                }}
              />
              <button type="button" onclick={() => w.addManual()}>Add</button>
            </span>
          </label>
          {#if !w.fetched.length && w.selected.length}
            <ul class="models" aria-label="Chosen models">
              {#each w.selected as m (m)}
                <li>
                  <code>{m}</code>
                  <label class="default">
                    <input
                      type="radio"
                      name="default-model"
                      aria-label={`Default model ${m}`}
                      checked={w.defaultModel === m}
                      onchange={() => (w.defaultModel = m)}
                    />
                    default
                  </label>
                  <button type="button" class="ghost" onclick={() => w.toggleModel(m)}
                    >Remove</button
                  >
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {:else if w.step === 'profile'}
        <label class="field">
          <span>Profile name</span>
          <input
            type="text"
            data-autofocus
            bind:value={w.profileName}
            oninput={() => (w.profileTouched = true)}
            spellcheck="false"
            autocomplete="off"
          />
        </label>
        {#if w.profileExists}
          <label class="choice warnbox">
            <input type="checkbox" bind:checked={w.overwrite} />
            <span>
              Profile "{w.profileName.trim()}" already exists. Overwrite its provider, endpoint, key
              and models.
            </span>
          </label>
        {/if}
        <label class="choice">
          <input type="checkbox" bind:checked={w.setDefault} />
          <span>Make this the default profile</span>
        </label>
      {:else if w.step === 'modes'}
        {#if w.state.modes.status === 'ready'}
          <p class="muted">
            Pick a model per mode, or keep the profile default. Only changed modes are written to
            .ruckcode/modes.toml.
          </p>
          <table class="modes">
            <thead><tr><th>Mode</th><th>Model</th></tr></thead>
            <tbody>
              {#each w.modeRows as r (r.id)}
                <tr>
                  <td><code>{r.id}</code><br /><span class="muted">{r.description}</span></td>
                  <td>
                    <select aria-label={`Model for mode ${r.id}`} bind:value={w.choices[r.id]}>
                      <option value="">Profile default</option>
                      {#each w.choiceGroups as g (g.profile)}
                        <optgroup label={g.profile}>
                          {#each g.models as m (m)}
                            <option value={choiceValue(g.profile, m)}>{m}</option>
                          {/each}
                        </optgroup>
                      {/each}
                      {#if r.initial && !w.choiceGroups.some( (g) => g.models.some((m) => choiceValue(g.profile, m) === r.initial) )}
                        <option value={r.initial}>{r.initial.replace(/^::/, '')} (current)</option>
                      {/if}
                    </select>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {:else if w.state.modes.status === 'missing'}
          <div class="box">
            <p>
              This project has no <code>.ruckcode/modes.toml</code>, so there are no modes to assign
              models to. Create it now (like <code>rc init</code>, including discovered mise tasks),
              or skip this step.
            </p>
            <button type="button" disabled={w.initializing} onclick={() => w.runInit()}>
              {w.initializing ? 'Creating...' : 'Create modes.toml'}
            </button>
          </div>
        {:else}
          <p class="errbox" role="alert">
            modes.toml cannot be read: {w.state.modes.error}. Fix it, then run setup again to assign
            models to modes.
          </p>
        {/if}
      {:else if w.step === 'review'}
        {#if w.busy && !w.diff}
          <p class="loading" role="status">
            <span class="spinner" aria-hidden="true"></span> Preparing changes...
          </p>
        {:else if w.diffError}
          <p class="errbox" role="alert">{w.diffError}</p>
        {:else}
          <p class="muted">Review what will be written. API keys are never shown.</p>
          <DiffText text={w.diff} />
        {/if}
      {/if}

      {#if w.problem && w.step !== 'review' && w.step !== 'endpoint'}
        <p class="problem" role="status" data-testid="step-problem">{w.problem}</p>
      {/if}
      {#if w.error}<p class="errbox" role="alert">{w.error}</p>{/if}

      <div class="actions nav">
        <button type="button" disabled={w.stepIndex === 0 || w.busy} onclick={() => w.back()}
          >Back</button
        >
        <span class="spacer"></span>
        <button type="submit" class="primary" disabled={!w.canNext}>
          {w.step === 'review'
            ? w.busy
              ? 'Saving...'
              : 'Save'
            : w.step === 'modes'
              ? 'Review'
              : 'Next'}
        </button>
      </div>
    </form>
  {/if}
</div>

<style>
  .wizard {
    display: grid;
    gap: 10px;
  }
  .steps {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 12px;
    color: var(--muted);
  }
  .steps li.current {
    color: var(--text);
    font-weight: 600;
  }
  .steps li.past {
    color: var(--ok);
  }
  .steps button {
    padding: 0;
    font-size: 12px;
    color: inherit;
  }
  form {
    display: grid;
    gap: 10px;
  }
  fieldset {
    border: 1px solid var(--border);
    border-radius: 6px;
    display: grid;
    gap: 4px;
    margin: 0;
  }
  .choice {
    display: flex;
    gap: 8px;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .choice.off {
    opacity: 0.55;
  }
  .field {
    display: grid;
    gap: 4px;
  }
  .field input,
  .field select {
    width: 100%;
  }
  .row {
    display: flex;
    gap: 6px;
  }
  .row input {
    flex: 1;
  }
  .toolbar {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }
  .toolbar input {
    flex: 1;
    min-width: 140px;
  }
  .models {
    list-style: none;
    margin: 0;
    padding: 4px 8px;
    max-height: 240px;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .models li {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    align-items: center;
    padding: 1px 0;
  }
  .default {
    font-size: 12px;
    color: var(--muted);
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .modes {
    width: 100%;
    border-collapse: collapse;
  }
  .modes th,
  .modes td {
    text-align: left;
    padding: 4px 6px;
    border-bottom: 1px solid var(--border);
    vertical-align: top;
  }
  .modes select {
    width: 100%;
  }
  .box,
  .errbox,
  .warnbox {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
    margin: 0;
  }
  .errbox {
    color: var(--err);
    border-color: var(--err);
    display: grid;
    gap: 4px;
  }
  .ok {
    color: var(--ok);
  }
  .warn {
    color: var(--warn);
    margin: 0;
  }
  .problem {
    margin: 0;
    color: var(--warn);
  }
  .nav {
    border-top: 1px solid var(--border);
    padding-top: 10px;
  }
  .spacer {
    flex: 1;
  }
  .report {
    margin: 0 0 8px;
    padding-left: 18px;
  }
  .loading {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .spinner {
    width: 14px;
    height: 14px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    display: inline-block;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
