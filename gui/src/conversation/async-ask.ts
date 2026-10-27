// Questions an agent asks without stopping its work (Codex's
// `request_user_input_async`; `world-gui.md`, 对话). They come as an agent's
// message carrying `questions`; the user answers whenever they like, or not
// at all, and the answer goes in as the user's own words, written as Codex's
// own client writes them: a `<send_user_message_question_reply>` envelope
// whose ids are `["request_user_input_async", item id, question index]`.
// The conversation shows what was answered, never the envelope.

export interface AsyncQuestion {
  title: string;
  /** The answers offered; a question without them takes free words. */
  options: string[] | null;
  /** What each answer offered means, where the agent said; told when the
   *  pointer rests on it. */
  notes?: (string | null)[];
  /** Several answers may be chosen together. */
  multiple?: boolean;
}

export const REPLY_TAG = "send_user_message_question_reply";

/** The envelope answering the questions of `item`: one element per
 *  question answered, in order; a skipped question is left out. */
export function replyOf(item: string, questions: AsyncQuestion[], answers: (string | null)[]): string {
  const answered = answers.flatMap((answer, index) =>
    answer === null || !answer.trim()
      ? []
      : [{ questionItemId: JSON.stringify(["request_user_input_async", item, index]), question: questions[index]!.title, answer: answer.trim() }],
  );
  return `<${REPLY_TAG}>\n${JSON.stringify(answered)}\n</${REPLY_TAG}>`;
}

/** The answers an envelope carries, and the item they answer; null when the
 *  words are not one. */
export function readReply(text: string): { item: string; answers: { index: number; answer: string }[] } | null {
  const match = new RegExp(`^\\s*<${REPLY_TAG}>\\s*([\\s\\S]*?)\\s*</${REPLY_TAG}>\\s*$`).exec(text);
  if (!match) return null;
  try {
    const list = JSON.parse(match[1]!) as { questionItemId?: unknown; answer?: unknown }[];
    let item: string | null = null;
    const answers: { index: number; answer: string }[] = [];
    for (const one of list) {
      if (typeof one.questionItemId !== "string" || typeof one.answer !== "string") return null;
      const id = JSON.parse(one.questionItemId) as unknown;
      if (!Array.isArray(id) || id[0] !== "request_user_input_async" || typeof id[1] !== "string" || typeof id[2] !== "number") return null;
      item ??= id[1];
      answers.push({ index: id[2], answer: one.answer });
    }
    return item ? { item, answers } : null;
  } catch {
    return null;
  }
}
