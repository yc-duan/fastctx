// Where a fork came from, as a card at the head of the fork (`world-gui.md`,
// 分叉): the conversation it was made from by its family's mark and name,
// and the last words between the user and the agent it carries on from, the
// user's words as they stand in the conversation and the answer under them
// fading out. Pressed, it goes back to that conversation.

import { glyph } from "./glyphs";
import { escape } from "./prose";

/** The conversation a fork was made from, and where. */
export interface ForkFrom {
  name: string;
  agent: "claude" | "codex";
  /** The user's last words before the fork, and the answer to them. */
  said: string | null;
  answer: string | null;
}

/** Markdown's own marks taken out of words shown short. */
function plain(text: string): string {
  return text
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}(#{1,6}|>|[-*+]|\d+[.)])\s+/gm, "")
    .replace(/(\*\*|__|\*|_|~~)(?=\S)([\s\S]*?\S)\1/g, "$2")
    .replace(/\s+/g, " ")
    .trim();
}

/** The card; `title` is what pointing at it says. */
export function forkCard(from: ForkFrom, title: string): string {
  const said = from.said ? plain(from.said) : "";
  const answer = from.answer ? plain(from.answer) : "";
  return `<button class="fk-card" type="button" title="${escape(title)}"><span class="fk-who">${glyph(from.agent, 14)}<span class="fk-name">${escape(from.name)}</span></span>${said ? `<span class="fk-said">${escape(said)}</span>` : ""}${answer ? `<span class="fk-answer">${escape(answer)}</span>` : ""}</button>`;
}
