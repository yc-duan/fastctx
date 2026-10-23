// The world's own ids in a plan come alive: a colleague, a sentinel, a job
// or a journal entry the agent wrote by name is drawn as a small chip that
// says how it stands now, read from the world rather than from the agent's
// words, which would go stale.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";

export type LiveKind = "resident" | "sense" | "job" | "entry";

/** How a thing in the world stands now, in the interface's words. */
export interface LiveFact {
  kind: LiveKind;
  /** The resident's host, for its mark. */
  agent?: "claude" | "codex";
  /** What it is called: a colleague's task, a sentinel's sentence. */
  label: string;
  /** Its state, short: 「在干活，2 小时」. */
  state: string;
  /** Whether that state wants the eye: working, fired, failed. */
  lit?: boolean;
}

export type LiveFacts = (id: string) => LiveFact | null;

const pattern = /\b(?:(?:codex|claude)-[0-9a-z]{4}|[js]-[0-9a-z]{6}|\d{8}T\d{6}Z-[A-Za-z0-9][\w.-]*)\b/g;

/** Replaces each id in the text under `root` with its chip. Code keeps its
 *  ids as written. */
export function liven(root: HTMLElement, facts: LiveFacts): void {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) =>
      node.parentElement?.closest("code, pre, .pb-chip, .diagram") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
  });
  const texts: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) texts.push(node as Text);
  for (const text of texts) {
    const value = text.data;
    pattern.lastIndex = 0;
    if (!pattern.test(value)) continue;
    pattern.lastIndex = 0;
    const out = document.createDocumentFragment();
    let at = 0;
    for (const match of value.matchAll(pattern)) {
      const id = match[0];
      const fact = facts(id);
      if (!fact) continue;
      out.append(value.slice(at, match.index));
      out.append(chip(id, fact));
      at = match.index! + id.length;
    }
    if (at === 0) continue;
    out.append(value.slice(at));
    text.replaceWith(out);
  }
}

/** A name as a book marks one, with a rule beneath it, and how it stands
 *  now as an interlinear note after it, in the small type; a colleague
 *  carries its host's mark. */
function chip(id: string, fact: LiveFact): HTMLElement {
  const element = document.createElement("span");
  element.className = `pb-chip pb-${fact.kind}${fact.lit ? " lit" : ""}`;
  element.dataset.id = id;
  element.tabIndex = 0;
  element.title = id;
  const mark = fact.kind === "resident" ? glyph(fact.agent ?? "colleague", 13) : "";
  element.innerHTML = `${mark}<span class="pb-chip-name">${escape(fact.label)}</span><span class="pb-chip-state">${escape(fact.state)}</span>`;
  return element;
}
