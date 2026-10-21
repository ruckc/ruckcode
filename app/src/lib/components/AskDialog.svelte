<script lang="ts">
  import { focusTrap } from '../a11y';
  import { parseRecommended } from '../format';
  import type { PendingQuestion } from '../state/chat.svelte';

  let { pending, total = 1 }: { pending: PendingQuestion; total?: number } = $props();

  const questions = $derived(pending.request.questions);
  let step = $state(0);
  let selected = $state<Record<number, string[]>>({});
  let other = $state<Record<number, string>>({});
  let otherInputs: (HTMLInputElement | undefined)[] = $state([]);

  const q = $derived(questions[step]);
  const last = $derived(step === questions.length - 1);

  function answered(i: number): boolean {
    return (selected[i]?.length ?? 0) > 0 || !!other[i]?.trim();
  }
  const allAnswered = $derived(questions.every((_, i) => answered(i)));

  function toggle(i: number, label: string, multi: boolean) {
    const cur = selected[i] ?? [];
    if (multi) selected[i] = cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label];
    else {
      selected[i] = [label];
      other[i] = ''; // single-select: a choice replaces the free-text answer
    }
  }

  function typeOther(i: number, multi: boolean) {
    if (!multi && other[i]?.trim()) selected[i] = [];
  }

  function submit() {
    pending.resolve({
      cancelled: false,
      answers: questions.map((qq, i) => ({
        question: qq.question,
        selected: selected[i] ?? [],
        other: other[i]?.trim() ? other[i].trim() : null
      }))
    });
  }

  function cancel() {
    pending.resolve({ answers: [], cancelled: true });
  }

  function next() {
    if (!last) step++;
    else if (allAnswered) submit();
  }

  function onkeydown(e: KeyboardEvent) {
    const inText =
      (e.target as HTMLElement).tagName === 'INPUT' &&
      (e.target as HTMLInputElement).type === 'text';
    if (e.key === 'Escape') {
      e.preventDefault();
      cancel();
    } else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && allAnswered) {
      e.preventDefault();
      submit();
    } else if (e.key === 'Enter' && inText) {
      e.preventDefault();
      next();
    } else if (e.altKey && e.key === 'ArrowRight') {
      e.preventDefault();
      step = Math.min(step + 1, questions.length - 1);
    } else if (e.altKey && e.key === 'ArrowLeft') {
      e.preventDefault();
      step = Math.max(step - 1, 0);
    } else if (!inText && !e.ctrlKey && !e.metaKey && !e.altKey && /^[1-9]$/.test(e.key)) {
      const opt = q.options[Number(e.key) - 1];
      if (opt) {
        e.preventDefault();
        toggle(step, opt.label, q.multiSelect);
      }
    } else if (!inText && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
      // Move between options (native radios already do; this also covers checkboxes).
      const items = [
        ...(e.currentTarget as HTMLElement).querySelectorAll<HTMLInputElement>('.opts input')
      ];
      const at = items.indexOf(document.activeElement as HTMLInputElement);
      if (at >= 0 && (q.multiSelect || items[at].type === 'checkbox')) {
        e.preventDefault();
        const n = (at + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
        items[n].focus();
      }
    }
  }

  function tabKey(e: KeyboardEvent, i: number) {
    if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
      e.preventDefault();
      const n = (i + (e.key === 'ArrowRight' ? 1 : -1) + questions.length) % questions.length;
      step = n;
      queueMicrotask(() => document.getElementById(`ask-tab-${n}`)?.focus());
    }
  }
</script>

<div class="backdrop" role="presentation">
  <div
    class="dialog wide"
    role="dialog"
    aria-modal="true"
    aria-labelledby="ask-title"
    tabindex="-1"
    {onkeydown}
    use:focusTrap
  >
    <h2 id="ask-title">
      The agent has {questions.length > 1 ? 'questions' : 'a question'}
      {#if total > 1}<span class="muted count">1 of {total} pending</span>{/if}
    </h2>
    {#if questions.length > 1}
      <div class="tabs" role="tablist" aria-label="Questions">
        {#each questions as qq, i (i)}
          <button
            id={`ask-tab-${i}`}
            role="tab"
            type="button"
            aria-selected={i === step}
            aria-controls="ask-panel"
            tabindex={i === step ? 0 : -1}
            class:done={answered(i)}
            onclick={() => (step = i)}
            onkeydown={(e) => tabKey(e, i)}
          >
            {qq.header || `Q${i + 1}`}
            {#if answered(i)}<span class="check" aria-label="answered">&#10003;</span>{/if}
          </button>
        {/each}
      </div>
    {/if}
    <div
      id="ask-panel"
      role={questions.length > 1 ? 'tabpanel' : undefined}
      aria-labelledby={questions.length > 1 ? `ask-tab-${step}` : undefined}
    >
      {#key step}
        <fieldset>
          <legend>{q.question}</legend>
          <p class="hint muted">
            {q.multiSelect ? 'Choose any number.' : 'Choose one.'} Number keys pick options.
          </p>
          <div class="opts">
            {#each q.options as o, oi (o.label)}
              {@const p = parseRecommended(o.label)}
              <label class="opt" class:picked={(selected[step] ?? []).includes(o.label)}>
                <input
                  type={q.multiSelect ? 'checkbox' : 'radio'}
                  name={`q${step}`}
                  data-autofocus={oi === 0 ? '' : undefined}
                  checked={(selected[step] ?? []).includes(o.label)}
                  onchange={() => toggle(step, o.label, q.multiSelect)}
                />
                <span class="num muted">{oi + 1}</span>
                <span class="text">
                  <span class="label">{p.label}</span>
                  {#if p.recommended}<span class="badge">Recommended</span>{/if}
                  {#if o.description}<span class="muted desc">{o.description}</span>{/if}
                </span>
              </label>
            {/each}
            <label class="opt other">
              <span class="num muted">&#8230;</span>
              <span class="text">
                <span class="label">Other</span>
                <input
                  type="text"
                  placeholder="Other…"
                  aria-label="Other answer"
                  bind:this={otherInputs[step]}
                  bind:value={other[step]}
                  oninput={() => typeOther(step, q.multiSelect)}
                />
              </span>
            </label>
          </div>
        </fieldset>
      {/key}
    </div>
    <div class="actions">
      {#if questions.length > 1}
        <button type="button" disabled={step === 0} onclick={() => step--}>Back</button>
        {#if !last}<button type="button" onclick={next}>Next</button>{/if}
      {/if}
      <button class="primary" type="button" disabled={!allAnswered} onclick={submit}>
        Submit <kbd>Ctrl+Enter</kbd>
      </button>
      <button type="button" class="ghost" onclick={cancel}>Cancel <kbd>Esc</kbd></button>
    </div>
  </div>
</div>

<style>
  .count {
    font-size: 12px;
    font-weight: 400;
    margin-left: 8px;
  }
  .tabs {
    display: flex;
    gap: 4px;
    margin-bottom: 8px;
    border-bottom: 1px solid var(--border);
  }
  .tabs button {
    border: 0;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: none;
  }
  .tabs button[aria-selected='true'] {
    border-bottom-color: var(--accent);
    font-weight: 600;
  }
  .check {
    color: var(--ok);
    margin-left: 4px;
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0;
  }
  legend {
    font-weight: 600;
    padding: 0;
  }
  .hint {
    margin: 2px 0 6px;
    font-size: 12px;
  }
  .opts {
    display: grid;
    gap: 4px;
  }
  .opt {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
  }
  .opt.picked {
    border-color: var(--accent);
    background: var(--code-bg);
  }
  .opt:has(input:focus-visible) {
    outline: 2px solid var(--accent);
  }
  .num {
    width: 1.2em;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .text {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 8px;
    align-items: baseline;
    flex: 1;
  }
  .other .text input {
    flex: 1;
    min-width: 8em;
  }
  .badge {
    font-size: 11px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--accent);
    color: var(--accent-text);
  }
  .desc {
    flex-basis: 100%;
  }
  kbd {
    font: inherit;
    font-size: 11px;
    opacity: 0.8;
    border: 1px solid currentColor;
    border-radius: 3px;
    padding: 0 4px;
    margin-left: 6px;
  }
</style>
