<script lang="ts">
  import { QuestionForm, type Question } from '@ruckc/truss-chat';
  import { parseRecommended } from '../format';
  import type { PendingQuestion } from '../state/chat.svelte';
  import Modal from './Modal.svelte';

  let { pending, total = 1 }: { pending: PendingQuestion; total?: number } = $props();

  const asked = $derived(pending.request.questions);

  /** The form works on option labels without the "(Recommended)" marker; answers go back with the original label. */
  const view = $derived.by(() => {
    const originals = asked.map(() => new Map<string, string>());
    const questions = asked.map((q, qi): Question => ({
      header: q.header || undefined,
      question: q.question,
      multiSelect: q.multiSelect,
      options: q.options.map((o) => {
        const p = parseRecommended(o.label);
        originals[qi].set(p.label, o.label);
        return {
          label: p.label,
          description: o.description ?? undefined,
          recommended: p.recommended
        };
      })
    }));
    return { questions, originals };
  });

  function submit(answers: { selected: string[]; other?: string }[]) {
    pending.resolve({
      cancelled: false,
      answers: asked.map((q, i) => ({
        question: q.question,
        selected: (answers[i]?.selected ?? []).map((l) => view.originals[i].get(l) ?? l),
        other: answers[i]?.other?.trim() ? answers[i].other!.trim() : null
      }))
    });
  }

  function cancel() {
    pending.resolve({ answers: [], cancelled: true });
  }
</script>

<Modal
  title={`The agent has ${asked.length > 1 ? 'questions' : 'a question'}`}
  size="lg"
  backdropClose={false}
  onclose={cancel}
>
  {#if total > 1}<p class="muted count">1 of {total} pending</p>{/if}
  <QuestionForm
    mode="tabs"
    numberKeys
    submitOnModEnter
    questions={view.questions}
    onsubmit={submit}
    onskip={cancel}
  />
</Modal>

<style>
  .count {
    margin: 0 0 var(--tr-space-2);
    font-size: var(--tr-text-sm);
  }
</style>
