<script lang="ts">
  import {
    Button,
    Callout,
    Checkbox,
    Input,
    RadioGroup,
    Select,
    Spinner,
    Stepper,
    Table
  } from '@ruckc/truss-ui';
  import { STEPS, STEP_LABEL, choiceValue, type SetupWizard } from '../state/wizard.svelte';
  import { PatchText } from '@ruckc/truss-code/lite';

  let {
    wizard,
    onreconnect,
    onclose
  }: { wizard: SetupWizard; onreconnect: () => void; onclose: () => void } = $props();

  const w = $derived(wizard);
  const steps = STEPS.map((s, i) => ({ id: s, label: `${STEP_LABEL[s]}` || String(i + 1) }));

  function submit(e: SubmitEvent) {
    e.preventDefault();
    void w.next();
  }

  const missingNames = $derived(w.state.keys.missing.map((m) => m.name));
  /** The Stepper's position: every step counts as past once the wizard is done. */
  const position = $derived(w.step === 'done' ? STEPS.length : w.stepIndex);

  const nextLabel = $derived(
    w.step === 'review' ? (w.busy ? 'Saving...' : 'Save') : w.step === 'modes' ? 'Review' : 'Next'
  );
  const otherApi = $derived(w.kind === 'anthropic' ? 'OpenAI-compatible' : 'Anthropic');
  const authOptions = $derived([
    {
      value: 'new',
      label: 'Paste an API key',
      description: 'Saved to your private per-user credentials file'
    },
    {
      value: 'existing',
      label: 'Use a saved key',
      description: w.keyNames.length ? 'From your API keys' : 'No saved keys yet',
      disabled: w.keyNames.length === 0
    },
    { value: 'none', label: 'No key', description: 'For local servers without authentication' }
  ]);
  const everyFilteredSelected = $derived(
    w.filtered.length > 0 && w.filtered.every((m) => w.selected.includes(m))
  );
  const choiceOptions = (initial: string) => [
    { value: '', label: 'Profile default' },
    ...w.choiceGroups.map((g) => ({
      group: g.profile,
      options: g.models.map((m) => ({ value: choiceValue(g.profile, m), label: m }))
    })),
    ...(initial &&
    !w.choiceGroups.some((g) => g.models.some((m) => choiceValue(g.profile, m) === initial))
      ? [{ value: initial, label: `${initial.replace(/^::/, '')} (current)` }]
      : [])
  ];
  const defaultOptions = $derived(
    w.selected.map((m) => ({ value: m, label: m, attrs: { 'aria-label': `Default model ${m}` } }))
  );
</script>

<div class="wizard">
  <Stepper {steps} clickable bind:current={() => position, (i) => w.goto(STEPS[i])} />
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
        <Button autofocus onclick={onreconnect}>Reconnect the agent now</Button>
        <Button variant="outlined" onclick={onclose}>Later</Button>
      </div>
    </section>
  {:else}
    <form onsubmit={submit} data-step={w.step}>
      {#if w.step === 'key'}
        <RadioGroup
          label="How should ruckcode authenticate?"
          options={authOptions}
          value={w.authKind}
          onchange={(e) => w.setAuth((e.target as HTMLInputElement).value as typeof w.authKind)}
        />
        {#if w.authKind === 'new'}
          <Input
            label="API key"
            type="password"
            autofocus
            bind:value={w.newKeySecret}
            oninput={() => w.keyEdited()}
            autocomplete="off"
            spellcheck="false"
            aria-describedby="secret-note"
          />
          <p id="secret-note" class="muted">
            Kept in memory until you save, then stored in <code>credentials.toml</code> (private file)
            in your user config folder; never in the project or in config files. It is never shown again.
          </p>
          <Input label="Description (optional)" bind:value={w.newKeyDescription} />
        {:else if w.authKind === 'existing'}
          <Select
            label="Saved key"
            bind:value={w.existingKey}
            onchange={() => w.setAuth('existing')}
            options={w.state.keys.keys.map((k) => ({
              value: k.name,
              label: `${k.name} (${k.masked})${k.description ? ` - ${k.description}` : ''}`
            }))}
          />
        {/if}
        {#if missingNames.length}
          <Callout intent="warning">
            Some profiles reference missing keys: {missingNames.join(', ')}.
          </Callout>
        {/if}
      {:else if w.step === 'endpoint'}
        <Input
          label="Endpoint URL"
          autofocus
          value={w.endpoint}
          oninput={(e) => w.setEndpoint(e.currentTarget.value)}
          placeholder="https://"
          spellcheck="false"
          autocomplete="off"
          aria-required="true"
          aria-invalid={w.endpointError ? 'true' : undefined}
          aria-describedby="endpoint-msg"
        />
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
            <Spinner size={14} /> Probing the Anthropic and OpenAI-compatible APIs...
          </p>
        {:else if w.checkPhase === 'ok'}
          <p class="ok" role="status" data-testid="check-status">
            &#10003; {w.probe?.message}{w.forced ? ' (flavor chosen manually)' : ''}
          </p>
          <div class="actions">
            <Button type="button" variant="outlined" onclick={() => w.overrideKind()}>
              Use the {otherApi} API instead
            </Button>
            <Button type="button" variant="outlined" onclick={() => w.runCheck(w.forced)}>
              Re-check
            </Button>
          </div>
        {:else if w.checkPhase === 'failed'}
          <Callout intent="danger" title="Could not verify the endpoint.">
            <span data-testid="check-error">{w.checkError}</span>
            {#snippet actions()}
              <Button type="button" variant="outlined" onclick={() => w.runCheck(w.forced)}>
                Retry
              </Button>
              <Button type="button" variant="outlined" onclick={() => w.overrideKind()}>
                Try the {otherApi} API
              </Button>
              <Button type="button" variant="outlined" onclick={() => w.back()}>Change URL</Button>
              <Button type="button" variant="outlined" onclick={() => w.continueManually()}>
                Continue without a model list
              </Button>
            {/snippet}
          </Callout>
        {/if}
      {:else if w.step === 'models'}
        {#if w.fetched.length}
          <div class="toolbar">
            <div class="grow">
              <Input
                aria-label="Filter models"
                placeholder="Filter models"
                bind:value={w.filter}
                autofocus
              />
            </div>
            <Button type="button" variant="outlined" onclick={() => w.toggleAllFiltered()}>
              {everyFilteredSelected ? 'Deselect all' : 'Select all'}
            </Button>
            <span class="muted">{w.selected.length} of {w.available.length} selected</span>
          </div>
          <ul class="models" aria-label="Models">
            {#each w.filtered as m (m)}
              <li>
                <Checkbox
                  label={m}
                  checked={w.selected.includes(m)}
                  oncheckedchange={() => w.toggleModel(m)}
                />
              </li>
            {:else}
              <li class="muted">No model matches the filter.</li>
            {/each}
          </ul>
        {:else}
          <p class="muted">No model list is available: type the model ids you want to use.</p>
        {/if}
        <div class="manual">
          <div class="row">
            <div class="grow">
              <Input
                label="Add model ids manually"
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
            </div>
            <Button type="button" variant="outlined" onclick={() => w.addManual()}>Add</Button>
          </div>
          {#if !w.fetched.length && w.selected.length}
            <ul class="models" aria-label="Chosen models">
              {#each w.selected as m (m)}
                <li>
                  <code>{m}</code>
                  <Button
                    type="button"
                    size="sm"
                    variant="minimal"
                    intent="neutral"
                    aria-label={`Remove ${m}`}
                    onclick={() => w.toggleModel(m)}>Remove</Button
                  >
                </li>
              {/each}
            </ul>
          {/if}
        </div>
        {#if w.selected.length}
          <RadioGroup
            label="Default model"
            options={defaultOptions}
            bind:value={() => w.defaultModel, (v) => (w.defaultModel = v)}
          />
        {/if}
      {:else if w.step === 'profile'}
        <Input
          label="Profile name"
          autofocus
          bind:value={w.profileName}
          oninput={() => (w.profileTouched = true)}
          spellcheck="false"
          autocomplete="off"
        />
        {#if w.profileExists}
          <Callout intent="warning">
            <Checkbox
              bind:checked={w.overwrite}
              label={`Profile "${w.profileName.trim()}" already exists. Overwrite its provider, endpoint, key and models.`}
            />
          </Callout>
        {/if}
        <Checkbox bind:checked={w.setDefault} label="Make this the default profile" />
      {:else if w.step === 'modes'}
        {#if w.state.modes.status === 'ready'}
          <p class="muted">
            Pick a model per mode, or keep the profile default. Only changed modes are written to
            .ruckcode/modes.toml.
          </p>
          <Table columns={['Mode', 'Model']}>
            {#each w.modeRows as r (r.id)}
              <tr>
                <td><code>{r.id}</code><br /><span class="muted">{r.description}</span></td>
                <td>
                  <Select
                    ariaLabel={`Model for mode ${r.id}`}
                    size="sm"
                    bind:value={w.choices[r.id]}
                    options={choiceOptions(r.initial)}
                  />
                </td>
              </tr>
            {/each}
          </Table>
        {:else if w.state.modes.status === 'missing'}
          <div class="box">
            <p>
              This project has no <code>.ruckcode/modes.toml</code>, so there are no modes to assign
              models to. Create it now (like <code>rc init</code>, including discovered mise tasks),
              or skip this step.
            </p>
            <Button
              type="button"
              variant="outlined"
              disabled={w.initializing}
              onclick={() => w.runInit()}
            >
              {w.initializing ? 'Creating...' : 'Create modes.toml'}
            </Button>
          </div>
        {:else}
          <Callout intent="danger">
            modes.toml cannot be read: {w.state.modes.error}. Fix it, then run setup again to assign
            models to modes.
          </Callout>
        {/if}
      {:else if w.step === 'review'}
        {#if w.busy && !w.diff}
          <p class="loading" role="status"><Spinner size={14} /> Preparing changes...</p>
        {:else if w.diffError}
          <Callout intent="danger">{w.diffError}</Callout>
        {:else}
          <p class="muted">Review what will be written. API keys are never shown.</p>
          <PatchText text={w.diff} label="Changes to be written" maxHeight="340px" />
        {/if}
      {/if}
      {#if w.problem && w.step !== 'review' && w.step !== 'endpoint'}
        <p class="problem" role="status" data-testid="step-problem">{w.problem}</p>
      {/if}
      {#if w.error}<Callout intent="danger">{w.error}</Callout>{/if}
      <div class="actions nav">
        <Button
          type="button"
          variant="outlined"
          disabled={w.stepIndex === 0 || w.busy}
          onclick={() => w.back()}>Back</Button
        >
        <span class="spacer"></span>
        <Button type="submit" disabled={!w.canNext}>{nextLabel}</Button>
      </div>
    </form>
  {/if}
</div>

<style>
  .wizard,
  form {
    display: grid;
    gap: var(--tr-space-3);
  }
  .actions,
  .row,
  .toolbar {
    display: flex;
    gap: var(--tr-space-2);
    align-items: center;
    flex-wrap: wrap;
  }
  .row {
    align-items: flex-end;
  }
  .grow {
    flex: 1;
    min-width: 140px;
  }
  .models {
    list-style: none;
    margin: 0;
    padding: var(--tr-space-1) var(--tr-space-2);
    max-height: 240px;
    overflow: auto;
    border: 1px solid var(--tr-outline-variant);
    border-radius: var(--tr-radius-md);
    display: grid;
    gap: var(--tr-space-1);
  }
  .models li {
    display: flex;
    justify-content: space-between;
    gap: var(--tr-space-2);
    align-items: center;
  }
  .box {
    border: 1px solid var(--tr-outline-variant);
    border-radius: var(--tr-radius-md);
    padding: var(--tr-space-2) var(--tr-space-3);
    display: grid;
    gap: var(--tr-space-2);
    justify-items: start;
  }
  .box p,
  .wizard p {
    margin: 0;
  }
  .ok {
    color: var(--tr-success);
  }
  .problem {
    margin: 0;
    color: var(--tr-warning);
  }
  .nav {
    border-top: 1px solid var(--tr-outline-variant);
    padding-top: var(--tr-space-3);
  }
  .spacer {
    flex: 1;
  }
  .report {
    margin: 0 0 var(--tr-space-2);
    padding-left: 18px;
  }
  .loading {
    display: flex;
    gap: var(--tr-space-2);
    align-items: center;
  }
</style>
