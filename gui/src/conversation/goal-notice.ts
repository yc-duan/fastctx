// A colleague working toward a goal, as the conversation that asked it
// hears it (`world-colleagues.md`): the report it hands back when its goal
// settles, and the questions it asks without stopping. Each is one quiet
// line; opened, the colleague's own words.

import type { JsonValue } from "../world/wire";
import { escape } from "./prose";

/** One question as the colleague's host asked it. */
export interface Asked {
  ask: string;
  options: { label: string; note: string | null }[];
}

/** The questions a host asked: Claude's `question` with labelled options,
 *  or a plain `title` with words for options. */
export function askedOf(questions: JsonValue | undefined): Asked[] {
  if (!Array.isArray(questions)) return [];
  const asked: Asked[] = [];
  for (const one of questions) {
    if (!one || typeof one !== "object" || Array.isArray(one)) continue;
    const ask = typeof one.question === "string" ? one.question : typeof one.title === "string" ? one.title : "";
    const options = Array.isArray(one.options)
      ? one.options.flatMap((option) => {
          if (typeof option === "string") return [{ label: option, note: null }];
          if (option && typeof option === "object" && !Array.isArray(option) && typeof option.label === "string") {
            return [{ label: option.label, note: typeof option.description === "string" && option.description.trim() ? option.description : null }];
          }
          return [];
        })
      : [];
    if (ask.trim()) asked.push({ ask: ask.trim(), options });
  }
  return asked;
}

/** The questions, opened under their line: each, and its choices beneath
 *  it with what each means in a quieter ink. */
export function askedHtml(asked: readonly Asked[]): string {
  return `<div class="gq-asked">${asked
    .map(
      (one) =>
        `<div class="gq-one"><p class="gq-ask">${escape(one.ask)}</p>${
          one.options.length
            ? `<ul class="gq-options">${one.options
                .map((option) => `<li><span class="gq-label">${escape(option.label)}</span>${option.note ? `<span class="gq-note">${escape(option.note)}</span>` : ""}</li>`)
                .join("")}</ul>`
            : ""
        }</div>`,
    )
    .join("")}</div>`;
}
