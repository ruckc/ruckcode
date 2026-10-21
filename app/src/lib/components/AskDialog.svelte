<script lang="ts">
  import type { PendingQuestion } from '../state/chat.svelte';

  let { pending }: { pending: PendingQuestion } = $props();

  const questions = $derived(pending.request.questions);
  let selected = $state<Record<number, string[]>>({});
  let other = $state<Record<number, string>>({});

  function toggle(i: number, label: string, multi: boolean) {
    const cur = selected[i] ?? [];
    if (multi) selected[i] = cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label];
    else selected[i] = [label];
  }

  function submit() {
    pending.resolve({
      cancelled: false,
      answers: questions.map((q, i) => ({
        question: q.question,
        selected: selected[i] ?? [],
        other: other[i]?.trim() ? other[i].trim() : null
      }))
    });
  }
</script>

<div class="backdrop" role="presentation">
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="ask-title">
    <h2 id="ask-title">The agent has a question</h2>
    {#each questions as q, i (i)}
      <fieldset>
        <legend>{q.header ? `${q.header}: ` : ''}{q.question}</legend>
        {#each q.options as o (o.label)}
          <label>
            <input
              type={q.multiSelect ? 'checkbox' : 'radio'}
              name={`q${i}`}
              checked={(selected[i] ?? []).includes(o.label)}
              onchange={() => toggle(i, o.label, q.multiSelect)}
            />
            {o.label}
            {#if o.description}<span class="muted">{o.description}</span>{/if}
          </label>
        {/each}
        <input type="text" placeholder="Other..." bind:value={other[i]} />
      </fieldset>
    {/each}
    <div class="actions">
      <button class="primary" onclick={submit}>Submit</button>
      <button onclick={() => pending.resolve({ answers: [], cancelled: true })}>Cancel</button>
    </div>
  </div>
</div>
