<script lang="ts">
  import { Button, Checkbox, Input, Kbd, RadioGroup, Tabs } from '@ruckc/truss-ui';
  import { parseRecommended } from '../format';
  import type { PendingQuestion } from '../state/chat.svelte';
  import Modal from './Modal.svelte';

  let { pending, total = 1 }: { pending: PendingQuestion; total?: number } = $props();

  // TODO(truss-ui#138): truss-chat's QuestionForm is a linear wizard; this keeps ruckcode's tabbed,
  // keyboard-driven flow (number keys, Alt+Arrow, Ctrl+Enter, "Recommended") on truss-ui controls.
  const questions = $derived(pending.request.questions);
  let step = $state(0);
  let selected = $state<Record<number, string[]>>({});
  let other = $state<Record<number, string>>({});

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

  const tabs = $derived(
    questions.map((qq, i) => ({
      id: String(i),
      label: `${qq.header || `Q${i + 1}`}${answered(i) ? ' ✓' : ''}`
    }))
  );
  /** "(Recommended)" moves from the label to a marker the option shows. */
  const recommended = (label: string) => {
    const p = parseRecommended(label);
    return p.recommended ? `${p.label} — Recommended` : p.label;
  };
</script>

{#snippet content()}
  {#key step}
    <fieldset class="q">
      <legend>{q.question}</legend>
      <p class="muted">
        {q.multiSelect ? 'Choose any number.' : 'Choose one.'} Number keys pick options.
      </p>
      <div class="opts">
        {#if q.multiSelect}
          {#each q.options as o, oi (o.label)}
            <Checkbox
              label={`${oi + 1}. ${recommended(o.label)}`}
              description={o.description ?? undefined}
              checked={(selected[step] ?? []).includes(o.label)}
              autofocus={oi === 0}
              oncheckedchange={() => toggle(step, o.label, true)}
            />
          {/each}
        {:else}
          <RadioGroup
            aria-label={q.question}
            options={q.options.map((o, oi) => ({
              value: o.label,
              label: `${oi + 1}. ${recommended(o.label)}`,
              description: o.description ?? undefined
            }))}
            value={selected[step]?.[0]}
            onchange={(e) => toggle(step, (e.target as HTMLInputElement).value, false)}
          />
        {/if}
      </div>
      <Input
        aria-label="Other answer"
        placeholder="Other…"
        value={other[step] ?? ''}
        oninput={(e) => {
          other[step] = e.currentTarget.value;
          typeOther(step, q.multiSelect);
        }}
      />
    </fieldset>
  {/key}
{/snippet}

<Modal
  title={`The agent has ${questions.length > 1 ? 'questions' : 'a question'}`}
  size="lg"
  backdropClose={false}
  {onkeydown}
  onclose={cancel}
>
  {#if total > 1}<p class="muted count">1 of {total} pending</p>{/if}
  {#if questions.length > 1}
    <Tabs
      {tabs}
      label="Questions"
      value={String(step)}
      onchange={(v) => (step = Number(v))}
      panel={content}
    />
  {:else}
    {@render content()}
  {/if}

  {#snippet footer()}
    {#if questions.length > 1}
      <Button variant="outlined" disabled={step === 0} onclick={() => step--}>Back</Button>
      {#if !last}<Button variant="outlined" onclick={next}>Next</Button>{/if}
    {/if}
    <Button disabled={!allAnswered} onclick={submit}>Submit <Kbd keys="ctrl+enter" /></Button>
    <Button variant="minimal" intent="neutral" onclick={cancel}>Cancel <Kbd keys="Esc" /></Button>
  {/snippet}
</Modal>

<style>
  .q {
    border: 0;
    margin: 0;
    padding: var(--tr-space-3) 0 0;
    display: grid;
    gap: var(--tr-space-2);
  }
  legend {
    font-weight: 600;
    padding: 0;
  }
  .q p,
  .count {
    margin: 0;
    font-size: var(--tr-text-sm);
  }
  .opts {
    display: grid;
    gap: var(--tr-space-2);
  }
</style>
