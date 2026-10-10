// The card of questions asked without waiting (`async-ask.ts`): drawn from
// the questions and what was answered, read back when the user answers.

import type { AsyncQuestion } from "./async-ask";
import { escape, plain } from "./prose";

export interface QuestionWords {
  answer: string;
  skip: string;
  /** "Answered", over what was answered. */
  answered: string;
  change: string;
  other: string;
  sending: string;
  failed(why: string): string;
}

export const zhQuestions: QuestionWords = {
  answer: "回答",
  skip: "跳过",
  answered: "已回答",
  change: "改答",
  other: "其他回答…",
  sending: "发送中",
  failed: () => "发送失败",
};

export const enQuestions: QuestionWords = {
  answer: "Answer",
  skip: "Skip",
  answered: "Answered",
  change: "Change",
  other: "Something else…",
  sending: "Sending",
  failed: () => "Couldn't send",
};

/** The card: each question, its options to choose one of and room for
 *  words of the user's own, nothing chosen before the user chooses; or,
 *  once answered, what was answered and a way to answer again. */
export function drawQuestions(key: string, questions: AsyncQuestion[], answers: (string | null)[], words: QuestionWords, editing: boolean): string {
  const done = answers.some((answer) => answer !== null);
  if (done && !editing) {
    return `<div class="question-card answered" data-question-card="${key}"><div class="question-said">${questions
      .map((question, index) =>
        answers[index] === null
          ? ""
          : `<p class="question-pair"><span class="question-title">${plain(question.title)}</span><span class="question-answer">${escape(answers[index]!)}</span></p>`,
      )
      .join("")}</div><div class="question-foot"><span class="question-state">${escape(words.answered)}</span><button class="text-button quiet-button" type="button" data-question-change="${key}">${escape(words.change)}</button></div></div>`;
  }
  const body = questions
    .map((question, index) => {
      const options = (question.options ?? [])
        .map((option) => `<button class="ask-option" type="button" data-question-pick="${index}" aria-pressed="false">${escape(option)}</button>`)
        .join("");
      return `<fieldset class="question-one" data-index="${index}"><legend class="question-title">${plain(question.title)}</legend>${options ? `<div class="ask-options">${options}</div>` : ""}<input class="ask-input question-free" type="text" data-question-free="${index}" placeholder="${escape(question.options?.length ? words.other : "")}" autocomplete="off"></fieldset>`;
    })
    .join("");
  return `<div class="question-card" data-question-card="${key}">${body}<div class="question-foot"><span class="question-state" aria-live="polite"></span><button class="ask-button" type="button" data-question-skip="${key}">${escape(words.skip)}</button><button class="ask-button ask-primary" type="button" data-question-send="${key}">${escape(words.answer)}</button></div></div>`;
}

/** What the card holds now: per question, the option chosen or the words
 *  written, whichever was touched last; null where nothing is. */
export function chosenIn(card: HTMLElement, count: number): (string | null)[] {
  return Array.from({ length: count }, (_, index) => {
    const free = card.querySelector<HTMLInputElement>(`[data-question-free="${index}"]`)?.value.trim();
    if (free) return free;
    const picked = card.querySelector<HTMLElement>(`[data-question-pick="${index}"][aria-pressed="true"]`);
    return picked?.textContent ?? null;
  });
}
