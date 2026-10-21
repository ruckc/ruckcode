import { z } from 'zod';

/** Method name of ruckcode's AskUserQuestion extension (agent -> client). */
export const ASK_USER_QUESTION = '_ruckcode/ask_user_question';

export const askQuestionSchema = z.object({
  question: z.string(),
  header: z.string().optional().nullable(),
  multiSelect: z.boolean().optional().default(false),
  options: z
    .array(z.object({ label: z.string(), description: z.string().optional().nullable() }))
    .default([])
});

export const askUserQuestionRequestSchema = z.object({
  sessionId: z.string().optional(),
  questions: z.array(askQuestionSchema)
});

export type AskQuestion = z.infer<typeof askQuestionSchema>;
export type AskUserQuestionRequest = z.infer<typeof askUserQuestionRequestSchema>;

export interface AskAnswer {
  question: string;
  selected: string[];
  other: string | null;
}

export interface AskUserQuestionResult {
  answers: AskAnswer[];
  cancelled: boolean;
}

/** Capabilities advertised in `initialize`. */
export const CLIENT_CAPABILITIES = {
  session: { notices: {} },
  _meta: { ruckcode: { askUserQuestion: true, notices: true } }
} as const;
